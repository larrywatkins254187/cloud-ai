//! Files a chat changed: from git (everything that differs from the commit the
//! chat started at) or, for folders without git, from the agent's edit tool calls.
use crate::git;
use crate::state::St;
use crate::store::Chat;
use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Changed {
    pub path: String,
    /// added | modified | deleted
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
    pub binary: bool,
    /// The chat's own edit tools touched this file.
    pub by_chat: bool,
}

/// One edit the agent made through a tool, read from the chat's messages.
#[derive(Debug, Clone)]
struct ToolEdit {
    path: String,
    /// add | update | delete
    kind: &'static str,
    additions: i64,
    deletions: i64,
    diff: String,
}

fn lines(s: &str) -> i64 {
    if s.is_empty() {
        0
    } else {
        s.lines().count() as i64
    }
}

fn rel(cwd: &str, p: &str) -> String {
    let base = cwd.trim_end_matches('/');
    p.strip_prefix(base).map(|r| r.trim_start_matches('/').to_string()).unwrap_or_else(|| p.to_string())
}

fn minus_plus(old: &str, new: &str) -> String {
    let mut d = String::new();
    for l in old.lines() {
        d.push('-');
        d.push_str(l);
        d.push('\n');
    }
    for l in new.lines() {
        d.push('+');
        d.push_str(l);
        d.push('\n');
    }
    d
}

/// Edits from Claude's Edit/Write/MultiEdit/NotebookEdit and Codex's file changes.
fn tool_edits(st: &St, chat: &Chat) -> Result<Vec<ToolEdit>> {
    let rows: Vec<(String, String)> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT kind, data FROM messages WHERE chat_id=?1 AND kind IN ('tool_use','tool_result') ORDER BY id")?;
        let v = q.query_map([chat.id], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(Result::ok).collect();
        v
    };
    // Calls that failed changed nothing.
    let mut failed = HashSet::new();
    let mut created = HashSet::new();
    for (kind, data) in &rows {
        if kind != "tool_result" {
            continue;
        }
        let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
        let id = v["tool_use_id"].as_str().unwrap_or("").to_string();
        if v["is_error"] == true {
            failed.insert(id.clone());
        }
        if v["output"].as_str().map(|o| o.contains("File created successfully")).unwrap_or(false) {
            created.insert(id);
        }
    }
    let mut out = Vec::new();
    for (kind, data) in &rows {
        if kind != "tool_use" {
            continue;
        }
        let v: Value = serde_json::from_str(data).unwrap_or(Value::Null);
        let id = v["id"].as_str().unwrap_or("");
        if failed.contains(id) {
            continue;
        }
        let input = &v["input"];
        let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
        match v["name"].as_str().unwrap_or("") {
            "Edit" if input["changes"].is_array() => {
                for ch in input["changes"].as_array().unwrap() {
                    let kind = ch["kind"]["type"].as_str().or(ch["kind"].as_str()).unwrap_or("update");
                    let diff = ch["diff"].as_str().unwrap_or("").to_string();
                    let add = diff.lines().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count() as i64;
                    let del = diff.lines().filter(|l| l.starts_with('-') && !l.starts_with("---")).count() as i64;
                    let (add, del) = match kind {
                        "add" if add == 0 => (lines(&diff), 0),
                        "delete" if del == 0 => (0, lines(&diff)),
                        _ => (add, del),
                    };
                    out.push(ToolEdit {
                        path: rel(&chat.cwd, ch["path"].as_str().unwrap_or("")),
                        kind: match kind {
                            "add" => "add",
                            "delete" => "delete",
                            _ => "update",
                        },
                        additions: add,
                        deletions: del,
                        diff,
                    });
                    if let Some(to) = ch["kind"]["move_path"].as_str().filter(|m| !m.is_empty()) {
                        out.push(ToolEdit { path: rel(&chat.cwd, to), kind: "add", additions: 0, deletions: 0, diff: String::new() });
                    }
                }
            }
            "Edit" => {
                let (o, n) = (s("old_string"), s("new_string"));
                out.push(ToolEdit { path: rel(&chat.cwd, &s("file_path")), kind: "update", additions: lines(&n), deletions: lines(&o), diff: minus_plus(&o, &n) });
            }
            "MultiEdit" => {
                let (mut a, mut d, mut diff) = (0, 0, String::new());
                for e in input["edits"].as_array().cloned().unwrap_or_default() {
                    let (o, n) = (e["old_string"].as_str().unwrap_or(""), e["new_string"].as_str().unwrap_or(""));
                    a += lines(n);
                    d += lines(o);
                    diff.push_str(&minus_plus(o, n));
                }
                out.push(ToolEdit { path: rel(&chat.cwd, &s("file_path")), kind: "update", additions: a, deletions: d, diff });
            }
            "Write" => {
                let c = s("content");
                let kind = if created.contains(id) { "add" } else { "update" };
                out.push(ToolEdit { path: rel(&chat.cwd, &s("file_path")), kind, additions: lines(&c), deletions: 0, diff: minus_plus("", &c) });
            }
            "NotebookEdit" => {
                let p = if input["notebook_path"].is_string() { s("notebook_path") } else { s("file_path") };
                let n = s("new_source");
                out.push(ToolEdit { path: rel(&chat.cwd, &p), kind: "update", additions: lines(&n), deletions: 0, diff: minus_plus("", &n) });
            }
            _ => {}
        }
    }
    out.retain(|e| !e.path.is_empty());
    Ok(out)
}

/// Files changed by the chat, built from its tool calls only.
fn from_tools(chat: &Chat, edits: &[ToolEdit]) -> Vec<Changed> {
    let mut by: BTreeMap<String, Changed> = BTreeMap::new();
    for e in edits {
        let f = by.entry(e.path.clone()).or_insert_with(|| Changed {
            path: e.path.clone(),
            status: if e.kind == "add" { "added".into() } else { "modified".into() },
            additions: 0,
            deletions: 0,
            binary: false,
            by_chat: true,
        });
        f.additions += e.additions;
        f.deletions += e.deletions;
        if e.kind == "delete" {
            f.status = "deleted".into();
        } else if f.status == "deleted" {
            f.status = "modified".into();
        }
    }
    // A shell command may have removed a file the tools wrote.
    for f in by.values_mut() {
        if f.status != "deleted" && !Path::new(&chat.cwd).join(&f.path).exists() {
            f.status = "deleted".into();
        }
    }
    by.into_values().collect()
}

fn count_file_lines(p: &Path) -> (i64, bool) {
    match std::fs::metadata(p) {
        Ok(m) if m.len() > 4 * 1024 * 1024 => (0, true),
        Ok(_) => match std::fs::read(p) {
            Ok(b) if b.contains(&0) => (0, true),
            Ok(b) => (lines(&String::from_utf8_lossy(&b)), false),
            Err(_) => (0, false),
        },
        Err(_) => (0, false),
    }
}

/// Everything in the chat's folder that differs from the commit it started at.
async fn from_git(cwd: &Path, base: &str) -> Result<Vec<Changed>> {
    let mut by: BTreeMap<String, Changed> = BTreeMap::new();
    let ns = git::must(cwd, &["-c", "core.quotepath=off", "diff", "--no-renames", "--name-status", base]).await?;
    for l in ns.lines() {
        let mut it = l.splitn(2, '\t');
        let (Some(st), Some(path)) = (it.next(), it.next()) else { continue };
        let status = match st.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            _ => "modified",
        };
        by.insert(path.to_string(), Changed { path: path.to_string(), status: status.into(), additions: 0, deletions: 0, binary: false, by_chat: false });
    }
    let num = git::must(cwd, &["-c", "core.quotepath=off", "diff", "--no-renames", "--numstat", base]).await?;
    for l in num.lines() {
        let p: Vec<&str> = l.splitn(3, '\t').collect();
        if p.len() < 3 {
            continue;
        }
        if let Some(f) = by.get_mut(p[2]) {
            if p[0] == "-" {
                f.binary = true;
            } else {
                f.additions = p[0].parse().unwrap_or(0);
                f.deletions = p[1].parse().unwrap_or(0);
            }
        }
    }
    let untracked = git::must(cwd, &["-c", "core.quotepath=off", "ls-files", "--others", "--exclude-standard"]).await?;
    for path in untracked.lines().filter(|l| !l.is_empty()).take(2000) {
        let (n, binary) = count_file_lines(&cwd.join(path));
        by.insert(path.to_string(), Changed { path: path.to_string(), status: "added".into(), additions: n, deletions: 0, binary, by_chat: false });
    }
    Ok(by.into_values().collect())
}

pub fn totals(files: &[Changed]) -> Value {
    let c = |s: &str| files.iter().filter(|f| f.status == s).count();
    json!({
        "files": files.len(),
        "added": c("added"),
        "modified": c("modified"),
        "deleted": c("deleted"),
        "additions": files.iter().map(|f| f.additions).sum::<i64>(),
        "deletions": files.iter().map(|f| f.deletions).sum::<i64>(),
    })
}

pub async fn list(st: &St, chat: &Chat) -> Result<Value> {
    let edits = tool_edits(st, chat)?;
    let cwd = Path::new(&chat.cwd);
    let base = chat.base_sha.clone().filter(|_| cwd.exists());
    let git_files = match &base {
        Some(b) if git::is_repo(cwd).await => from_git(cwd, b).await.ok(),
        _ => None,
    };
    let (mode, files) = match git_files {
        Some(mut files) => {
            let touched: HashSet<&str> = edits.iter().map(|e| e.path.as_str()).collect();
            for f in &mut files {
                f.by_chat = touched.contains(f.path.as_str());
            }
            ("git", files)
        }
        None => ("tools", from_tools(chat, &edits)),
    };
    Ok(json!({
        "mode": mode,
        "base": base,
        // Without its own worktree a chat shares the folder, so git also shows other changes.
        "shared": mode == "git" && chat.worktree_branch.is_none(),
        "totals": totals(&files),
        "files": files,
    }))
}

/// The diff of one changed file.
pub async fn diff(st: &St, chat: &Chat, path: &str) -> Result<String> {
    if path.contains("..") || path.starts_with('/') {
        anyhow::bail!("bad path");
    }
    let cwd = Path::new(&chat.cwd);
    if let Some(base) = chat.base_sha.as_deref() {
        if git::is_repo(cwd).await {
            let tracked = git::run(cwd, &["cat-file", "-e", &format!("{base}:{path}")]).await.map(|o| o.ok).unwrap_or(false)
                || git::run(cwd, &["ls-files", "--error-unmatch", "--", path]).await.map(|o| o.ok).unwrap_or(false);
            let o = if tracked {
                git::run(cwd, &["diff", base, "--", path]).await?
            } else {
                git::run(cwd, &["diff", "--no-index", "--", "/dev/null", path]).await?
            };
            return Ok(o.stdout);
        }
    }
    let edits = tool_edits(st, chat)?;
    let mut out = String::new();
    for e in edits.iter().filter(|e| e.path == path) {
        out.push_str(&format!("@@ {} @@\n", e.kind));
        out.push_str(&e.diff);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        assert_eq!(rel("/srv/app", "/srv/app/src/a.rs"), "src/a.rs");
        assert_eq!(rel("/srv/app/", "/srv/app/b"), "b");
        assert_eq!(rel("/srv/app", "/etc/x"), "/etc/x");
    }

    #[test]
    fn totals_count_statuses() {
        let f = |p: &str, s: &str, a, d| Changed { path: p.into(), status: s.into(), additions: a, deletions: d, binary: false, by_chat: true };
        let t = totals(&[f("a", "added", 3, 0), f("b", "modified", 2, 1), f("c", "deleted", 0, 9)]);
        assert_eq!(t["files"], 3);
        assert_eq!(t["added"], 1);
        assert_eq!(t["deleted"], 1);
        assert_eq!(t["additions"], 5);
        assert_eq!(t["deletions"], 10);
    }
}
