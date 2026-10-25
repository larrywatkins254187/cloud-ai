#!/usr/bin/env bash
# cloud-ai installer for Ubuntu / Debian (run as root).
#
#   curl -fsSL https://raw.githubusercontent.com/larrywatkins254187/cloud-ai/main/deploy/install.sh \
#     | sudo bash -s -- --domain ai.example.com
#
# Options:
#   --domain <name>    configure nginx (+ Let's Encrypt via certbot when available) for this domain
#   --version <tag>    release to install (default: latest)
#   --build            build from this checkout instead of downloading a release (needs cargo + node)
#   --port <port>      local port the panel listens on (default 8787)
#   --no-browser       skip installing headless Chromium for the live preview
set -euo pipefail

REPO="larrywatkins254187/cloud-ai"
DOMAIN=""
VERSION="latest"
BUILD=0
PORT=8787
BROWSER=1
DATA=/var/lib/cloud-ai
BIN=/usr/local/bin/cloud-ai

while [ $# -gt 0 ]; do
  case "$1" in
    --domain) DOMAIN="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --build) BUILD=1; shift ;;
    --port) PORT="$2"; shift 2 ;;
    --no-browser) BROWSER=0; shift ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }

[ "$(id -u)" = 0 ] || { echo "Please run as root (sudo)." >&2; exit 1; }

say "Installing system packages (git, ssh, rsync, sshpass)"
apt-get update -q
DEBIAN_FRONTEND=noninteractive apt-get install -y -q curl ca-certificates git openssh-client rsync sshpass

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|aarch64) ;;
  arm64) ARCH=aarch64 ;;
  *) echo "unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

if [ "$BUILD" = 1 ]; then
  SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  say "Building from $SRC"
  (cd "$SRC/web" && npm ci && npm run build)
  (cd "$SRC" && cargo build --release)
  install -m 755 "$SRC/target/release/cloud-ai" "$BIN.new"
else
  if [ "$VERSION" = "latest" ]; then
    URL="https://github.com/$REPO/releases/latest/download/cloud-ai-linux-$ARCH"
  else
    URL="https://github.com/$REPO/releases/download/$VERSION/cloud-ai-linux-$ARCH"
  fi
  say "Downloading $URL"
  TMP="$(mktemp -d)"
  curl -fL --retry 3 -o "$TMP/cloud-ai" "$URL"
  if curl -fsL -o "$TMP/cloud-ai.sha256" "$URL.sha256"; then
    (cd "$TMP" && echo "$(cut -d' ' -f1 cloud-ai.sha256)  cloud-ai" | sha256sum -c -)
  fi
  install -m 755 "$TMP/cloud-ai" "$BIN.new"
  rm -rf "$TMP"
fi
mv -f "$BIN.new" "$BIN"
say "Installed $("$BIN" version)"

# The service needs the folders of claude, codex, node and npm on its PATH.
SPATH="/root/.local/bin"
for c in claude codex node npm npx; do
  p="$(command -v "$c" 2>/dev/null || true)"
  if [ -n "$p" ]; then SPATH="$SPATH:$(dirname "$p")"; fi
done
for d in /root/.nvm/versions/node/*/bin /root/.npm-global/bin /root/.bun/bin; do
  [ -d "$d" ] && SPATH="$SPATH:$d"
done
SPATH="$SPATH:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
SPATH="$(echo "$SPATH" | tr ':' '\n' | awk '!seen[$0]++' | paste -sd: -)"

PATH="$SPATH:$PATH"
command -v claude >/dev/null || warn "Claude Code not found. Install it with: npm install -g @anthropic-ai/claude-code"
command -v codex >/dev/null || warn "Codex CLI not found. Install it with: npm install -g @openai/codex"

if [ "$BROWSER" = 1 ]; then
  if command -v npx >/dev/null; then
    say "Installing headless Chromium for the live preview (about 200 MB on disk)"
    HOME=/root npx -y playwright@latest install --with-deps --only-shell chromium || warn "Chromium install failed; the Preview tab will not work until a browser is installed."
  else
    warn "npx not found; skipping Chromium (needed for the Preview tab)."
  fi
fi

mkdir -p "$DATA"
chmod 700 "$DATA"

PUBLIC_URL=""
[ -n "$DOMAIN" ] && PUBLIC_URL="https://$DOMAIN"

say "Writing /etc/systemd/system/cloud-ai.service"
cat > /etc/systemd/system/cloud-ai.service <<EOF
[Unit]
Description=cloud-ai — web panel for Claude Code and Codex
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
Environment=HOME=/root
Environment=CLOUD_AI_DATA=$DATA
Environment=CLOUD_AI_BIND=127.0.0.1:$PORT
Environment=CLOUD_AI_PUBLIC_URL=$PUBLIC_URL
Environment=PATH=$SPATH
ExecStart=$BIN serve
Restart=always
RestartSec=3
KillMode=mixed
TimeoutStopSec=20
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable cloud-ai >/dev/null
systemctl restart cloud-ai

if [ -n "$DOMAIN" ]; then
  command -v nginx >/dev/null || { say "Installing nginx"; DEBIAN_FRONTEND=noninteractive apt-get install -y -q nginx; }
  CONF=/etc/nginx/sites-available/cloud-ai
  CERT=/etc/letsencrypt/live/$DOMAIN
  # Only listen on IPv6 when the server has it, or nginx refuses to start.
  V6_80=""; V6_443=""
  if [ -f /proc/net/if_inet6 ]; then V6_80="    listen [::]:80;"; V6_443="    listen [::]:443 ssl;"; fi
  LOCATION="
    client_max_body_size 200m;

    location / {
        proxy_pass http://127.0.0.1:$PORT;
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection \$cloud_ai_connection;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
        proxy_buffering off;
    }"
  MAP='map $http_upgrade $cloud_ai_connection {
    default upgrade;
    ""      close;
}'
  write_http() {
    cat > "$CONF" <<EOF
$MAP

server {
    listen 80;
$V6_80
    server_name $DOMAIN;
$LOCATION
}
EOF
  }
  write_https() {
    local opts=""
    [ -f /etc/letsencrypt/options-ssl-nginx.conf ] && opts="    include /etc/letsencrypt/options-ssl-nginx.conf;"
    cat > "$CONF" <<EOF
$MAP

server {
    listen 80;
$V6_80
    server_name $DOMAIN;
    return 301 https://\$host\$request_uri;
}

server {
    listen 443 ssl;
$V6_443
    server_name $DOMAIN;
    ssl_certificate $CERT/fullchain.pem;
    ssl_certificate_key $CERT/privkey.pem;
$opts
$LOCATION
}
EOF
  }
  if [ -f "$CERT/fullchain.pem" ]; then
    write_https
  else
    write_http
  fi
  ln -sf "$CONF" /etc/nginx/sites-enabled/cloud-ai
  nginx -t && systemctl reload nginx
  if [ ! -f "$CERT/fullchain.pem" ]; then
    if command -v certbot >/dev/null; then
      say "Requesting a Let's Encrypt certificate for $DOMAIN"
      if certbot certonly --nginx -d "$DOMAIN" --non-interactive --agree-tos --register-unsafely-without-email; then
        write_https
        nginx -t && systemctl reload nginx
      else
        warn "certbot failed — the panel is reachable over plain HTTP for now. Fix DNS for $DOMAIN and run this script again."
      fi
    else
      warn "certbot is not installed; install it (apt install certbot python3-certbot-nginx) and run this script again for HTTPS."
    fi
  fi
fi

# Show the one-time setup token for the first admin.
for _ in $(seq 1 30); do
  [ -s "$DATA/setup-token" ] && break
  sleep 0.5
done
echo
say "cloud-ai is running."
if [ -n "$DOMAIN" ]; then echo "    Open:  https://$DOMAIN"; else echo "    Open:  http://127.0.0.1:$PORT (put nginx in front of it, or use --domain)"; fi
if [ -s "$DATA/setup-token" ]; then
  echo "    Setup token for the first admin: $(cat "$DATA/setup-token")"
fi
echo "    Logs:  journalctl -u cloud-ai -f"
echo "    Then:  log in, scan the 2FA QR code, and connect Claude Code / Codex under Admin → Claude & Codex."
