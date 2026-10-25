//! Deploys: run the project's deploy command locally, or ship the code to a
//! remote server over SSH (git pull or rsync) and run it there. Every deploy
//! records the commit it shipped so a rollback can redeploy an older one.
use crate::events::Event;
use crate::git;
use crate::state::St;
use crate::store::{self, get_project, Project, Server};
use crate::util::{now, shell_quote, tail};
use anyhow::{anyhow, bail, Result};
use rusqlite::params;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Default)]
pub struct DeployState {
    running: Mutex<HashSet<i64>>,
}

struct Log<'a> {
    st: &'a St,
    deploy_id: i64,
    project_id: i64,
}

impl Log<'_> {
    fn line(&self, s: &str) {
        let line = format!("{s}\n");
        let _ = self.st.db.conn().execute("UPDATE deploys SET log=log||?2 WHERE id=?1", params![self.deploy_id, line]);
        self.st.emit_project(
            self.project_id,
            Event::DeployLog { deploy_id: self.deploy_id, project_id: self.project_id, line: s.to_string() },
        );
    }
}

/// Start a deploy (or a rollback when `rollback_to` is set). Returns the deploy
/// id, or None when one is already running for the project.
pub fn start(st: &St, project_id: i64, by: &str, rollback_to: Option<String>) -> Option<i64> {
    if !st.deploys.running.lock().unwrap().insert(project_id) {
        return None;
    }
    let kind = if rollback_to.is_some() { "rollback" } else { "deploy" };
    let id = {
        let c = st.db.conn();
        let r = c.execute(
            "INSERT INTO deploys(project_id,triggered_by,kind,status,created_at) VALUES(?1,?2,?3,'running',?4)",
            params![project_id, by, kind, now()],
        );
        if r.is_err() {
            st.deploys.running.lock().unwrap().remove(&project_id);
            return None;
        }
        c.last_insert_rowid()
    };
    emit(st, id);
    let st2 = st.clone();
    tokio::spawn(async move {
        let log = Log { st: &st2, deploy_id: id, project_id };
        let res = run(&st2, id, project_id, rollback_to.clone(), &log).await;
        let (status, health) = match res {
            Ok(h) => ("success", h),
            Err(e) => {
                log.line(&format!("ERROR: {e:#}"));
                ("failed", None)
            }
        };
        let _ = st2.db.conn().execute(
            "UPDATE deploys SET status=?2, health=?3, finished_at=?4 WHERE id=?1",
            params![id, status, health, now()],
        );
        st2.deploys.running.lock().unwrap().remove(&project_id);
        emit(&st2, id);
        // Automatic rollback when the health check fails after a normal deploy.
        if status == "failed" && rollback_to.is_none() {
            if let Ok(d) = store::get_deploy(&st2.db, id) {
                let p = get_project(&st2.db, project_id);
                let health_failed = d.log.contains("HEALTH CHECK FAILED");
                if let (Ok(p), Some(prev)) = (p, d.prev_sha) {
                    if p.auto_rollback && health_failed {
                        st2.notice("warn", format!("{}: health check failed, rolling back to {}", p.name, &prev[..prev.len().min(8)]));
                        start(&st2, project_id, "auto-rollback", Some(prev));
                    }
                }
            }
        }
    });
    Some(id)
}

fn emit(st: &St, id: i64) {
    if let Ok(d) = store::get_deploy(&st.db, id) {
        st.emit_project(d.project_id, Event::DeployUpdated { deploy: d });
    }
}

fn last_success_sha(st: &St, project_id: i64, exclude: i64) -> Option<String> {
    st.db
        .conn()
        .query_row(
            "SELECT commit_sha FROM deploys WHERE project_id=?1 AND status='success' AND id<>?2 AND commit_sha IS NOT NULL ORDER BY id DESC LIMIT 1",
            params![project_id, exclude],
            |r| r.get(0),
        )
        .ok()
}

async fn run(st: &St, id: i64, project_id: i64, rollback_to: Option<String>, log: &Log<'_>) -> Result<Option<String>> {
    let p = get_project(&st.db, project_id)?;
    if p.deploy_target == "none" {
        bail!("deploy is not configured for this project");
    }
    let root = PathBuf::from(&p.path);
    let is_git = git::is_repo(&root).await;
    log.line(&format!("Project: {} ({})", p.name, p.path));

    if is_git && p.auto_commit && rollback_to.is_none() && git::is_dirty(&root).await {
        let msg = format!("cloud-ai: deploy {}", chrono::Utc::now().format("%Y-%m-%d %H:%M"));
        git::commit_all(&root, &msg).await?;
        log.line(&format!("Committed pending changes: {msg}"));
    }
    let sha = match &rollback_to {
        Some(s) => Some(s.clone()),
        None if is_git => git::head(&root).await,
        None => None,
    };
    let prev = last_success_sha(st, project_id, id);
    st.db.conn().execute("UPDATE deploys SET commit_sha=?2, prev_sha=?3 WHERE id=?1", params![id, sha, prev])?;
    emit(st, id);
    if rollback_to.is_some() && !is_git {
        bail!("rollback needs the project to be a git repository");
    }

    match p.deploy_target.as_str() {
        "local" => {
            if let Some(target) = &rollback_to {
                // Keep the current state (including uncommitted work) reachable before moving back.
                if git::is_dirty(&root).await {
                    git::commit_all(&root, "cloud-ai: uncommitted changes before rollback").await?;
                }
                let backup = format!("cloud-ai/backup-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
                git::must(&root, &["branch", &backup]).await?;
                log.line(&format!("Saved current state as branch {backup}"));
                git::must(&root, &["reset", "--hard", target]).await?;
                log.line(&format!("Checked out {target}"));
            }
            if !p.deploy_command.trim().is_empty() {
                stream(log, local_cmd(&root, &p.deploy_command, sha.as_deref())).await?;
            }
        }
        "remote" => {
            let sid = p.server_id.ok_or_else(|| anyhow!("no server selected"))?;
            let server = store::get_server(&st.db, sid)?;
            if p.remote_path.trim().is_empty() {
                bail!("remote path is not set");
            }
            if p.deploy_method == "git" {
                let sha = sha.clone().ok_or_else(|| anyhow!("git deploy needs a git repository"))?;
                if rollback_to.is_none() {
                    let branch = git::current_branch(&root).await.unwrap_or_else(|| "HEAD".into());
                    log.line(&format!("Pushing {branch} to origin…"));
                    let o = git::run_timeout(&root, &["push", "origin", &branch], Duration::from_secs(300)).await?;
                    log.line(o.stderr.trim());
                    if !o.ok {
                        bail!("git push failed");
                    }
                }
                let script = format!(
                    "set -e; cd {}; git fetch --all --prune; git reset --hard {}; export DEPLOY_REF={}; {}",
                    shell_quote(&p.remote_path),
                    shell_quote(&sha),
                    shell_quote(&sha),
                    p.deploy_command
                );
                stream(log, ssh_cmd(st, &server, &script)?).await?;
            } else {
                let mut tmp: Option<PathBuf> = None;
                let src = match &rollback_to {
                    Some(target) => {
                        let dir = st.cfg.data_dir.join("tmp").join(format!("deploy-{id}"));
                        let _ = std::fs::remove_dir_all(&dir);
                        std::fs::create_dir_all(&dir)?;
                        let o = Command::new("bash")
                            .arg("-c")
                            .arg(format!("git archive {} | tar -x -C {}", shell_quote(target), shell_quote(&dir.to_string_lossy())))
                            .current_dir(&root)
                            .output()
                            .await?;
                        if !o.status.success() {
                            bail!("git archive failed: {}", String::from_utf8_lossy(&o.stderr));
                        }
                        tmp = Some(dir.clone());
                        dir
                    }
                    None => root.clone(),
                };
                log.line(&format!("Syncing files to {}:{}…", server.host, p.remote_path));
                let res = stream(log, rsync_cmd(st, &server, &src, &p)?).await;
                if let Some(t) = tmp {
                    let _ = std::fs::remove_dir_all(t);
                }
                res?;
                if !p.deploy_command.trim().is_empty() {
                    let script = format!(
                        "set -e; cd {}; export DEPLOY_REF={}; {}",
                        shell_quote(&p.remote_path),
                        shell_quote(sha.as_deref().unwrap_or("")),
                        p.deploy_command
                    );
                    stream(log, ssh_cmd(st, &server, &script)?).await?;
                }
            }
        }
        t => bail!("unknown deploy target {t}"),
    }

    if p.health_url.trim().is_empty() {
        log.line("Done.");
        return Ok(None);
    }
    log.line(&format!("Health check {} …", p.health_url));
    match health_check(&p).await {
        Ok(h) => {
            log.line(&format!("Healthy: {h}"));
            Ok(Some(h))
        }
        Err(e) => {
            log.line(&format!("HEALTH CHECK FAILED: {e}"));
            bail!("health check failed")
        }
    }
}

async fn health_check(p: &Project) -> Result<String> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
    let mut last = String::new();
    for _ in 0..12 {
        match client.get(&p.health_url).send().await {
            Ok(r) if r.status().is_success() || r.status().is_redirection() => return Ok(format!("HTTP {}", r.status().as_u16())),
            Ok(r) => last = format!("HTTP {}", r.status().as_u16()),
            Err(e) => last = e.to_string(),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    bail!("{last}")
}

fn local_cmd(root: &Path, script: &str, sha: Option<&str>) -> Command {
    let mut c = Command::new("bash");
    c.arg("-lc").arg(script).current_dir(root).env("DEPLOY_REF", sha.unwrap_or(""));
    c
}

async fn stream(log: &Log<'_>, mut cmd: Command) -> Result<()> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null()).kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tx2 = tx.clone();
    tokio::spawn(async move {
        let mut l = BufReader::new(out).lines();
        while let Ok(Some(s)) = l.next_line().await {
            let _ = tx.send(s);
        }
    });
    tokio::spawn(async move {
        let mut l = BufReader::new(err).lines();
        while let Ok(Some(s)) = l.next_line().await {
            let _ = tx2.send(s);
        }
    });
    let wait = async {
        while let Some(line) = rx.recv().await {
            log.line(&line);
        }
        child.wait().await
    };
    let status = tokio::time::timeout(Duration::from_secs(1800), wait)
        .await
        .map_err(|_| anyhow!("timed out after 30 minutes"))??;
    if !status.success() {
        bail!("command exited with {}", status.code().map(|c| c.to_string()).unwrap_or("signal".into()));
    }
    Ok(())
}

// ---------- SSH ----------

pub fn app_key_path(st: &St) -> PathBuf {
    st.cfg.ssh_dir().join("id_ed25519")
}

pub fn custom_key_path(st: &St, server_id: i64) -> PathBuf {
    st.cfg.ssh_dir().join("keys").join(format!("server-{server_id}"))
}

/// Create the panel's own SSH key on first start.
pub fn ensure_app_key(st: &St) -> Result<String> {
    let dir = st.cfg.ssh_dir();
    std::fs::create_dir_all(dir.join("keys"))?;
    let key = app_key_path(st);
    if !key.exists() {
        let o = std::process::Command::new("ssh-keygen")
            .args(["-t", "ed25519", "-N", "", "-C", "cloud-ai", "-f"])
            .arg(&key)
            .output()?;
        if !o.status.success() {
            bail!("ssh-keygen failed: {}", String::from_utf8_lossy(&o.stderr));
        }
    }
    Ok(std::fs::read_to_string(key.with_extension("pub"))?.trim().to_string())
}

fn identity(st: &St, s: &Server) -> Option<PathBuf> {
    match s.auth_type.as_str() {
        "app_key" => Some(app_key_path(st)),
        "key" => Some(custom_key_path(st, s.id)),
        _ => None,
    }
}

fn ssh_base_args(st: &St, s: &Server) -> Vec<String> {
    let mut a = vec![
        "-p".into(),
        s.port.to_string(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", st.cfg.ssh_dir().join("known_hosts").display()),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
    ];
    if let Some(k) = identity(st, s) {
        a.extend(["-i".into(), k.display().to_string(), "-o".into(), "BatchMode=yes".into(), "-o".into(), "IdentitiesOnly=yes".into()]);
    }
    a
}

fn password(st: &St, s: &Server) -> Result<Option<String>> {
    if s.auth_type != "password" {
        return Ok(None);
    }
    let enc: Option<String> = st.db.conn().query_row("SELECT secret_enc FROM servers WHERE id=?1", [s.id], |r| r.get(0))?;
    Ok(Some(st.secrets.decrypt(&enc.ok_or_else(|| anyhow!("password missing"))?)?))
}

/// `ssh user@host bash -lc <script>` (wrapped in sshpass for password servers).
pub fn ssh_cmd(st: &St, s: &Server, script: &str) -> Result<Command> {
    let mut args = ssh_base_args(st, s);
    args.push(format!("{}@{}", s.username, s.host));
    args.push(format!("bash -lc {}", shell_quote(script)));
    Ok(match password(st, s)? {
        Some(pw) => {
            let mut c = Command::new("sshpass");
            c.arg("-e").arg("ssh").args(&args).env("SSHPASS", pw);
            c
        }
        None => {
            let mut c = Command::new("ssh");
            c.args(&args);
            c
        }
    })
}

fn rsync_cmd(st: &St, s: &Server, src: &Path, p: &Project) -> Result<Command> {
    let ssh = format!("ssh {}", ssh_base_args(st, s).iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
    let mut args: Vec<String> = vec!["-az".into(), "--delete".into(), "--info=stats1".into(), "-e".into(), ssh];
    let mut excludes: Vec<String> = p.rsync_excludes.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if excludes.is_empty() {
        excludes = ["node_modules", ".git", ".next", ".env", ".env.local"].iter().map(|s| s.to_string()).collect();
    }
    for e in excludes {
        args.push(format!("--exclude={e}"));
    }
    args.push(format!("{}/", src.display()));
    args.push(format!("{}@{}:{}/", s.username, s.host, p.remote_path.trim_end_matches('/')));
    Ok(match password(st, s)? {
        Some(pw) => {
            let mut c = Command::new("sshpass");
            c.arg("-e").arg("rsync").args(&args).env("SSHPASS", pw);
            c
        }
        None => {
            let mut c = Command::new("rsync");
            c.args(&args);
            c
        }
    })
}

/// Pull a project's files from a server into a local folder (initial import).
pub async fn rsync_from(st: &St, s: &Server, remote_path: &str, dest: &Path) -> Result<String> {
    let ssh = format!("ssh {}", ssh_base_args(st, s).iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" "));
    let args = vec![
        "-az".to_string(),
        "--exclude=node_modules".into(),
        "--exclude=.next".into(),
        "-e".into(),
        ssh,
        format!("{}@{}:{}/", s.username, s.host, remote_path.trim_end_matches('/')),
        format!("{}/", dest.display()),
    ];
    let out = match password(st, s)? {
        Some(pw) => Command::new("sshpass").arg("-e").arg("rsync").args(&args).env("SSHPASS", pw).output().await?,
        None => Command::new("rsync").args(&args).output().await?,
    };
    if !out.status.success() {
        bail!("rsync failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub async fn test_server(st: &St, s: &Server) -> Result<String> {
    let out = tokio::time::timeout(Duration::from_secs(30), ssh_cmd(st, s, "echo ok; uname -a; uptime")?.output())
        .await
        .map_err(|_| anyhow!("timed out"))??;
    if !out.status.success() {
        bail!("{}", tail(String::from_utf8_lossy(&out.stderr).trim(), 1000));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// ssh_config file the agents can use: `ssh -F <data>/ssh/config cai-<name>`.
pub fn write_ssh_config(st: &St) {
    let servers: Vec<Server> = {
        let c = st.db.conn();
        let mut q = match c.prepare(&format!("SELECT {} FROM servers", Server::COLS)) {
            Ok(q) => q,
            Err(_) => return,
        };
        q.query_map([], Server::from_row).map(|it| it.filter_map(Result::ok).collect()).unwrap_or_default()
    };
    let mut out = String::from("# Generated by cloud-ai — do not edit\n");
    for s in servers.iter().filter(|s| s.auth_type != "password") {
        out.push_str(&format!(
            "\nHost {}\n  HostName {}\n  Port {}\n  User {}\n  IdentityFile {}\n  IdentitiesOnly yes\n  StrictHostKeyChecking accept-new\n  UserKnownHostsFile {}\n",
            s.alias(),
            s.host,
            s.port,
            s.username,
            identity(st, s).map(|p| p.display().to_string()).unwrap_or_default(),
            st.cfg.ssh_dir().join("known_hosts").display()
        ));
    }
    let path = st.cfg.ssh_dir().join("config");
    let _ = std::fs::write(&path, out);
    crate::crypto::set_mode_600(&path);
}
