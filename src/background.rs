//! Background shell commands and monitors started by Claude Code (`Bash` with
//! `run_in_background`, and the `Monitor` tool). Claude reports them as
//! `local_bash` tasks; their output is saved to `<claude tmp>/…/tasks/<id>.output`.
//! This module finds that file and, for "kill", the processes behind a task.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Task ids are short random tokens; anything else is refused before touching the filesystem.
pub fn valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Where Claude Code keeps task output: `$CLAUDE_CODE_TMPDIR|$TMPDIR|/tmp` + `claude-*/<project>/<session>/tasks/`.
fn tmp_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for k in ["CLAUDE_CODE_TMPDIR", "TMPDIR"] {
        if let Ok(p) = std::env::var(k) {
            if !p.is_empty() {
                v.push(PathBuf::from(p));
            }
        }
    }
    v.push(PathBuf::from("/tmp"));
    v.dedup();
    v
}

/// The output file of a task. `hint` is a path Claude reported (trusted only when it
/// ends in `tasks/<task_id>.output`); otherwise the usual locations are searched.
pub fn output_path(task_id: &str, hint: Option<&str>) -> Option<PathBuf> {
    if !valid_task_id(task_id) {
        return None;
    }
    let name = format!("{task_id}.output");
    if let Some(h) = hint.filter(|h| !h.is_empty()) {
        let p = PathBuf::from(h);
        let ok = p.file_name().map(|f| f == name.as_str()).unwrap_or(false)
            && p.parent().and_then(|d| d.file_name()).map(|d| d == "tasks").unwrap_or(false);
        if ok && p.is_file() {
            return Some(p);
        }
    }
    for root in tmp_roots() {
        let Ok(tops) = std::fs::read_dir(&root) else { continue };
        for top in tops.flatten() {
            if !top.file_name().to_string_lossy().starts_with("claude") {
                continue;
            }
            let Ok(projects) = std::fs::read_dir(top.path()) else { continue };
            for proj in projects.flatten() {
                let Ok(sessions) = std::fs::read_dir(proj.path()) else { continue };
                for s in sessions.flatten() {
                    let p = s.path().join("tasks").join(&name);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

/// The last `max` bytes of a file, starting at a line boundary when cut.
pub fn read_tail(path: &Path, max: u64) -> std::io::Result<(String, u64, bool)> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let start = size.saturating_sub(max);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    f.take(max).read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        if let Some(i) = text.find('\n') {
            text = text[i + 1..].to_string();
        }
    }
    Ok((text, size, start > 0))
}

struct Proc {
    ppid: u32,
    pgid: u32,
}

fn stat(pid: u32) -> Option<Proc> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Fields after the parenthesised command name: state ppid pgrp …
    let rest = &s[s.rfind(')')? + 2..];
    let mut it = rest.split_whitespace();
    let _state = it.next()?;
    let ppid = it.next()?.parse().ok()?;
    let pgid = it.next()?.parse().ok()?;
    Some(Proc { ppid, pgid })
}

fn all_pids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.parse().ok())).collect())
        .unwrap_or_default()
}

fn writes_to(pid: u32, file: &Path) -> bool {
    [1, 2].iter().any(|fd| std::fs::read_link(format!("/proc/{pid}/fd/{fd}")).map(|l| l == file).unwrap_or(false))
}

/// How Claude Code wraps a command: `eval '<command>'` with `'` written as `'"'"'`.
fn eval_pattern(command: &str) -> String {
    format!("eval '{}'", command.replace('\'', "'\"'\"'"))
}

/// Process groups running a task: processes writing to its output file, or
/// descendants of the Claude process whose command line is the task's command.
pub fn task_groups(claude_pid: Option<u32>, command: &str, output: Option<&Path>) -> Vec<u32> {
    let pids = all_pids();
    let procs: HashMap<u32, Proc> = pids.iter().filter_map(|p| stat(*p).map(|s| (*p, s))).collect();
    let me = std::process::id();
    let my_group = procs.get(&me).map(|p| p.pgid);
    let claude_group = claude_pid.and_then(|p| procs.get(&p)).map(|p| p.pgid);
    let under_claude = |mut p: u32| -> bool {
        let Some(root) = claude_pid else { return false };
        for _ in 0..64 {
            match procs.get(&p) {
                Some(s) if s.ppid == root => return true,
                Some(s) if s.ppid > 1 => p = s.ppid,
                _ => return false,
            }
        }
        false
    };
    let pattern = eval_pattern(command);
    let mut groups = HashSet::new();
    for (&pid, p) in &procs {
        if pid == me || Some(pid) == claude_pid {
            continue;
        }
        let hit = output.map(|o| writes_to(pid, o)).unwrap_or(false)
            || (!command.trim().is_empty() && under_claude(pid) && {
                let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                let cmd = String::from_utf8_lossy(&cmd).replace('\0', " ");
                cmd.contains(&pattern)
            });
        if hit {
            groups.insert(p.pgid);
        }
    }
    // Never signal our own group or the Claude process's group.
    groups.into_iter().filter(|g| *g > 1 && Some(*g) != my_group && Some(*g) != claude_group).collect()
}

/// SIGKILL every process group of a task. Returns how many groups were signalled.
pub fn kill_task(claude_pid: Option<u32>, command: &str, output: Option<&Path>) -> usize {
    let groups = task_groups(claude_pid, command, output);
    for g in &groups {
        unsafe {
            libc::kill(-(*g as i32), libc::SIGKILL);
        }
    }
    groups.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn eval_pattern_quotes_like_claude() {
        assert_eq!(eval_pattern("sleep 31; echo a"), "eval 'sleep 31; echo a'");
        assert_eq!(eval_pattern("printf '%s' x"), r#"eval 'printf '"'"'%s'"'"' x'"#);
    }

    #[test]
    fn task_ids_are_checked() {
        assert!(valid_task_id("b604tpwlt"));
        assert!(!valid_task_id("../etc/passwd"));
        assert!(!valid_task_id(""));
        assert!(output_path("../x", None).is_none());
    }

    #[test]
    fn kills_a_process_writing_to_the_output_file() {
        let dir = std::env::temp_dir().join(format!("cai-bg-{}", std::process::id())).join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("tkill1.output");
        let f = std::fs::File::create(&out).unwrap();
        let mut child = std::process::Command::new("sh")
            .args(["-c", "while true; do echo tick; sleep 0.2; done"])
            .stdout(f.try_clone().unwrap())
            .stderr(f)
            .process_group(0)
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(kill_task(None, "", Some(&out)), 1);
        let status = child.wait().unwrap();
        assert!(!status.success());
        let (text, size, cut) = read_tail(&out, 1 << 20).unwrap();
        assert!(text.contains("tick") && size > 0 && !cut);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
