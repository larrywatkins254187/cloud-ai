//! Thin wrapper around the `git` binary.
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub async fn run(cwd: &Path, args: &[&str]) -> Result<Output> {
    run_timeout(cwd, args, Duration::from_secs(120)).await
}

pub async fn run_timeout(cwd: &Path, args: &[&str], timeout: Duration) -> Result<Output> {
    let fut = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(timeout, fut).await {
        Ok(r) => r?,
        Err(_) => bail!("git {} timed out", args.join(" ")),
    };
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    })
}

/// Run git and fail with its stderr when it exits non-zero.
pub async fn must(cwd: &Path, args: &[&str]) -> Result<String> {
    let o = run(cwd, args).await?;
    if !o.ok {
        bail!("git {}: {}", args.join(" "), o.stderr.trim());
    }
    Ok(o.stdout)
}

pub async fn is_repo(cwd: &Path) -> bool {
    run(cwd, &["rev-parse", "--is-inside-work-tree"]).await.map(|o| o.ok).unwrap_or(false)
}

pub async fn head(cwd: &Path) -> Option<String> {
    must(cwd, &["rev-parse", "HEAD"]).await.ok().map(|s| s.trim().to_string())
}

pub async fn current_branch(cwd: &Path) -> Option<String> {
    must(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await.ok().map(|s| s.trim().to_string())
}

pub async fn is_dirty(cwd: &Path) -> bool {
    must(cwd, &["status", "--porcelain"]).await.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

pub async fn status(cwd: &Path) -> Result<Value> {
    if !is_repo(cwd).await {
        return Ok(json!({"repo": false}));
    }
    let out = must(cwd, &["status", "--porcelain=v1", "--branch", "-uall"]).await?;
    let mut branch = String::new();
    let mut ahead = 0;
    let mut behind = 0;
    let mut files = Vec::new();
    for line in out.lines() {
        if let Some(b) = line.strip_prefix("## ") {
            let (name, rest) = b.split_once("...").unwrap_or((b, ""));
            branch = name.split(' ').next().unwrap_or(name).to_string();
            if let Some(i) = rest.find("ahead ") {
                ahead = rest[i + 6..].split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or(0);
            }
            if let Some(i) = rest.find("behind ") {
                behind = rest[i + 7..].split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or(0);
            }
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let (code, path) = line.split_at(3);
        let path = path.rsplit(" -> ").next().unwrap_or(path).trim_matches('"');
        files.push(json!({"index": &code[0..1], "worktree": &code[1..2], "path": path}));
    }
    let remote = must(cwd, &["remote", "get-url", "origin"]).await.unwrap_or_default();
    Ok(json!({
        "repo": true,
        "branch": branch,
        "ahead": ahead,
        "behind": behind,
        "files": files,
        "remote": remote.trim(),
        "head": head(cwd).await,
    }))
}

pub async fn diff(cwd: &Path, path: Option<&str>) -> Result<String> {
    // Show both staged and unstaged changes against HEAD, plus untracked files.
    let mut args = vec!["diff", "HEAD", "--no-color"];
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let mut out = run(cwd, &args).await?.stdout;
    if out.trim().is_empty() {
        let mut args = vec!["diff", "--no-color"];
        if let Some(p) = path {
            args.push("--");
            args.push(p);
        }
        out = run(cwd, &args).await?.stdout;
    }
    if let Some(p) = path {
        if out.trim().is_empty() {
            // Untracked file: show it as an addition.
            let o = run(cwd, &["diff", "--no-color", "--no-index", "/dev/null", p]).await?;
            out = o.stdout;
        }
    }
    Ok(out)
}

pub async fn log(cwd: &Path, n: usize) -> Result<Value> {
    let out = must(
        cwd,
        &["log", &format!("-{n}"), "--pretty=format:%H%x1f%h%x1f%an%x1f%aI%x1f%s"],
    )
    .await
    .unwrap_or_default();
    let items: Vec<Value> = out
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split('\u{1f}').collect();
            (p.len() == 5).then(|| json!({"sha": p[0], "short": p[1], "author": p[2], "date": p[3], "subject": p[4]}))
        })
        .collect();
    Ok(Value::Array(items))
}

pub async fn branches(cwd: &Path) -> Result<Value> {
    let out = must(cwd, &["branch", "-a", "--format=%(refname:short)%x1f%(HEAD)"]).await?;
    let items: Vec<Value> = out
        .lines()
        .filter_map(|l| {
            let (name, head) = l.split_once('\u{1f}')?;
            Some(json!({"name": name, "current": head == "*"}))
        })
        .collect();
    Ok(Value::Array(items))
}

pub async fn commit_all(cwd: &Path, message: &str) -> Result<bool> {
    if !is_dirty(cwd).await {
        return Ok(false);
    }
    must(cwd, &["add", "-A"]).await?;
    // Use the repo's own identity; fall back to a cloud-ai identity when none is configured.
    let o = run(cwd, &["commit", "-m", message]).await?;
    if !o.ok {
        let o2 = run(cwd, &["-c", "user.name=cloud-ai", "-c", "user.email=cloud-ai@localhost", "commit", "-m", message]).await?;
        if !o2.ok {
            bail!("git commit: {}", o2.stderr.trim());
        }
    }
    Ok(true)
}
