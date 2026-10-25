mod agents;
mod api;
mod auth;
mod browser;
mod catalog;
mod chats;
mod config;
mod crypto;
mod db;
mod deploy;
mod devserver;
mod events;
mod extensions;
mod git;
mod runner;
mod scheduler;
mod settings;
mod state;
mod store;
mod sysmon;
mod telegram;
mod terminal;
mod updater;
mod util;
mod web;
mod ws;

use anyhow::{bail, Context, Result};
use axum::extract::DefaultBodyLimit;
use config::Config;
use rusqlite::params;
use state::AppState;
use std::io::{BufRead, Write};
use std::sync::Arc;

const HELP: &str = "cloud-ai — web panel for Claude Code and Codex CLI

USAGE:
  cloud-ai [serve]                  run the server (default)
  cloud-ai create-admin <username>  create an admin user (asks for a password)
  cloud-ai set-password <username>  set a user's password
  cloud-ai reset-2fa <username>     make a user enrol 2FA again on next login
  cloud-ai version

OPTIONS / ENV:
  --data <dir>    CLOUD_AI_DATA   data directory (default /var/lib/cloud-ai)
  --bind <addr>   CLOUD_AI_BIND   listen address (default 127.0.0.1:8787)
";

fn read_password() -> Result<String> {
    eprint!("Password (min 8 chars): ");
    std::io::stderr().flush()?;
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s)?;
    let pw = s.trim_end_matches(['\n', '\r']).to_string();
    if pw.chars().count() < 8 {
        bail!("password too short");
    }
    Ok(pw)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::from_env_and_args(&mut args);
    let cmd = args.first().cloned().unwrap_or_else(|| "serve".into());
    match cmd.as_str() {
        "version" | "--version" | "-V" => {
            println!("cloud-ai {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        "create-admin" | "set-password" | "reset-2fa" => {
            let name = args.get(1).context("username required")?.clone();
            std::fs::create_dir_all(&cfg.data_dir)?;
            let db = db::Db::open(&cfg.db_path())?;
            match cmd.as_str() {
                "create-admin" => {
                    let pw = read_password()?;
                    db.conn().execute(
                        "INSERT INTO users(username,password_hash,role,created_at) VALUES(?1,?2,'admin',?3)",
                        params![name, crypto::hash_password(&pw)?, util::now()],
                    )?;
                    let _ = std::fs::remove_file(cfg.setup_token_path());
                    println!("admin {name} created — log in and scan the 2FA QR code");
                }
                "set-password" => {
                    let pw = read_password()?;
                    let n = db.conn().execute(
                        "UPDATE users SET password_hash=?2 WHERE username=?1",
                        params![name, crypto::hash_password(&pw)?],
                    )?;
                    if n == 0 {
                        bail!("no such user");
                    }
                    db.conn().execute("DELETE FROM sessions WHERE user_id=(SELECT id FROM users WHERE username=?1)", [&name])?;
                    println!("password updated");
                }
                _ => {
                    let n = db
                        .conn()
                        .execute("UPDATE users SET totp_enabled=0, totp_secret=NULL WHERE username=?1", [&name])?;
                    if n == 0 {
                        bail!("no such user");
                    }
                    println!("2FA reset; {name} will enrol again on next login");
                }
            }
            Ok(())
        }
        "serve" => {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
            rt.block_on(serve(cfg))
        }
        other => {
            eprint!("unknown command {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

async fn serve(cfg: Config) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "cloud_ai=info,warn".into()),
        )
        .init();
    for d in [cfg.data_dir.clone(), cfg.uploads_dir(), cfg.worktrees_dir(), cfg.ssh_dir(), cfg.screenshots_dir(), cfg.data_dir.join("tmp")] {
        std::fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    }
    let db = db::Db::open(&cfg.db_path())?;
    let secrets = crypto::Secrets::load_or_create(&cfg.data_dir.join("secret.key"))?;
    let (bus, _) = tokio::sync::broadcast::channel(4096);
    let st = Arc::new(AppState {
        cfg: cfg.clone(),
        db,
        secrets,
        bus,
        http: reqwest::Client::builder().user_agent(concat!("cloud-ai/", env!("CARGO_PKG_VERSION"))).build()?,
        runner: Default::default(),
        codex: Default::default(),
        telegram: Default::default(),
        login_limiter: Default::default(),
        sysmon: Default::default(),
        deploys: Default::default(),
        terminals: Default::default(),
        devservers: Default::default(),
        browser: Default::default(),
        updater: Default::default(),
    });
    auth::ensure_setup_token(&st);
    runner::recover(&st);
    if let Err(e) = deploy::ensure_app_key(&st) {
        tracing::warn!("ssh key: {e:#}");
    }
    deploy::write_ssh_config(&st);
    let _ = st.db.conn().execute("DELETE FROM sessions WHERE expires_at < ?1", [util::now()]);

    runner::spawn(st.clone());
    scheduler::spawn(st.clone());
    catalog::spawn(st.clone());
    telegram::spawn(st.clone());
    sysmon::spawn(st.clone());
    updater::spawn(st.clone());

    let app = api::router()
        .fallback(web::serve)
        .layer(DefaultBodyLimit::max(200 * 1024 * 1024))
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind(&cfg.bind).await.with_context(|| format!("binding {}", cfg.bind))?;
    tracing::info!("cloud-ai {} listening on http://{}", env!("CARGO_PKG_VERSION"), cfg.bind);
    let st2 = st.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            tracing::info!("shutting down");
            shutdown(&st2).await;
        })
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// Stop child processes so nothing keeps running after the panel exits.
async fn shutdown(st: &state::St) {
    let sessions: Vec<_> = st.runner.claude.lock().unwrap().drain().map(|(_, s)| s).collect();
    for s in sessions {
        s.kill();
    }
    st.codex.stop().await;
    devserver::stop_all(st).await;
    browser::stop(st).await;
    terminal::stop_all(st);
}
