use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use std::path::Path;

/// Symmetric encryption for secrets stored in SQLite (SSH passwords, keys).
/// The key lives in `<data>/secret.key` (0600), outside the database.
#[derive(Clone)]
pub struct Secrets {
    key: [u8; 32],
}

impl Secrets {
    pub fn load_or_create(path: &Path) -> Result<Secrets> {
        let key = if path.exists() {
            let raw = std::fs::read(path)?;
            raw.try_into().map_err(|_| anyhow!("invalid secret.key"))?
        } else {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            std::fs::write(path, k)?;
            set_mode_600(path);
            k
        };
        Ok(Secrets { key })
    }

    pub fn encrypt(&self, plain: &str) -> String {
        let cipher = Aes256Gcm::new_from_slice(&self.key).expect("key length");
        let mut nonce = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = cipher.encrypt(Nonce::from_slice(&nonce), plain.as_bytes()).expect("encrypt");
        let mut out = nonce.to_vec();
        out.extend(ct);
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    pub fn decrypt(&self, enc: &str) -> Result<String> {
        let raw = base64::engine::general_purpose::STANDARD.decode(enc)?;
        if raw.len() < 13 {
            return Err(anyhow!("ciphertext too short"));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).expect("key length");
        let plain = cipher
            .decrypt(Nonce::from_slice(&raw[..12]), &raw[12..])
            .map_err(|_| anyhow!("decrypt failed"))?;
        Ok(String::from_utf8(plain)?)
    }
}

pub fn set_mode_600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

pub fn hash_password(pw: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("hash: {e}"))
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

// ---------- TOTP (RFC 6238, SHA1, 6 digits, 30 s) ----------

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in data {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in s.chars().filter(|c| *c != '=' && !c.is_whitespace()) {
        let v = B32.iter().position(|&x| x as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

pub fn new_totp_secret() -> String {
    let mut b = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut b);
    base32_encode(&b)
}

fn hotp(key: &[u8], counter: u64) -> u32 {
    let mut mac = <Hmac<sha1::Sha1> as Mac>::new_from_slice(key).expect("hmac key");
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0xf) as usize;
    let code = ((h[off] as u32 & 0x7f) << 24)
        | ((h[off + 1] as u32) << 16)
        | ((h[off + 2] as u32) << 8)
        | (h[off + 3] as u32);
    code % 1_000_000
}

pub fn verify_totp(secret_b32: &str, code: &str) -> bool {
    let code = code.trim().replace(' ', "");
    let Ok(code) = code.parse::<u32>() else { return false };
    let Some(key) = base32_decode(secret_b32) else { return false };
    let step = (chrono::Utc::now().timestamp() / 30) as i64;
    (-1..=1).any(|d| hotp(&key, (step + d) as u64) == code)
}

pub fn totp_uri(secret: &str, username: &str, issuer: &str) -> String {
    format!(
        "otpauth://totp/{issuer}:{user}?secret={secret}&issuer={issuer}&algorithm=SHA1&digits=6&period=30",
        issuer = urlencode(issuer),
        user = urlencode(username),
    )
}

pub fn qr_svg(data: &str) -> String {
    match qrcode::QrCode::new(data.as_bytes()) {
        Ok(code) => code
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(220, 220)
            .quiet_zone(true)
            .build(),
        Err(_) => String::new(),
    }
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_roundtrip() {
        let data = b"hello world 12345";
        assert_eq!(base32_decode(&base32_encode(data)).unwrap(), data);
    }

    #[test]
    fn hotp_rfc4226_vectors() {
        let key = b"12345678901234567890";
        assert_eq!(hotp(key, 0), 755224);
        assert_eq!(hotp(key, 1), 287082);
        assert_eq!(hotp(key, 9), 520489);
    }

    #[test]
    fn password_hash_verifies() {
        let h = hash_password("secret").unwrap();
        assert!(verify_password("secret", &h));
        assert!(!verify_password("nope", &h));
    }
}
