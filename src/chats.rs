use crate::events::Event;
use crate::state::St;
use crate::store::{self, get_chat, get_project, Chat};
use crate::util::now;
use anyhow::{bail, Result};
use rusqlite::params;
use std::path::Path;

pub struct NewChat {
    pub project_id: i64,
    pub user_id: Option<i64>,
    pub title: String,
    pub tool: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    pub worktree: bool,
    pub auto_deploy: bool,
    pub schedule_id: Option<i64>,
    /// The agent may read but not change anything: no worktree, branch or deploy.
    pub read_only: bool,
}

/// Chat title from the first line of a prompt.
pub fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("New chat").trim();
    let mut t: String = first.chars().take(60).collect();
    if first.chars().count() > 60 {
        t.push('…');
    }
    t
}

pub fn validate_tool(tool: &str) -> Result<()> {
    if tool != "claude" && tool != "codex" {
        bail!("tool must be claude or codex");
    }
    Ok(())
}

pub fn validate_mode(mode: &str) -> Result<()> {
    if !["bypass", "edits", "ask", "plan"].contains(&mode) {
        bail!("permission mode must be bypass, edits, ask or plan");
    }
    Ok(())
}

pub fn create_chat(st: &St, mut n: NewChat) -> Result<Chat> {
    validate_tool(&n.tool)?;
    if n.read_only {
        n.worktree = false;
        n.auto_deploy = false;
    }
    validate_mode(&n.permission_mode)?;
    let project = get_project(&st.db, n.project_id)?;
    let title = if n.title.trim().is_empty() { "New chat".to_string() } else { n.title.trim().to_string() };
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO chats(project_id,user_id,title,tool,model,effort,permission_mode,cwd,auto_deploy,schedule_id,created_at,updated_at,read_only)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11,?12)",
            params![
                n.project_id,
                n.user_id,
                title,
                n.tool,
                n.model,
                n.effort,
                n.permission_mode,
                project.path,
                n.auto_deploy as i64,
                n.schedule_id,
                now(),
                n.read_only as i64
            ],
        )?;
        c.last_insert_rowid()
    };
    if n.worktree {
        // Worktrees are created synchronously with std::process to keep this fn sync.
        let branch = format!("cloud-ai/chat-{id}");
        let dir = st.cfg.worktrees_dir().join(project.id.to_string()).join(format!("chat-{id}"));
        let _ = std::fs::create_dir_all(dir.parent().unwrap());
        let out = std::process::Command::new("git")
            .args(["worktree", "add", "-b", &branch, &dir.to_string_lossy(), "HEAD"])
            .current_dir(&project.path)
            .output()?;
        if !out.status.success() {
            st.db.conn().execute("DELETE FROM chats WHERE id=?1", [id])?;
            bail!("could not create git worktree: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        st.db.conn().execute(
            "UPDATE chats SET cwd=?2, worktree_branch=?3 WHERE id=?1",
            params![id, dir.to_string_lossy(), branch],
        )?;
    }
    // Remember where the chat started, so its changed files can be listed later.
    let dir = get_chat(&st.db, id)?.cwd;
    if let Ok(o) = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&dir).output() {
        let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let base = if o.status.success() && !sha.is_empty() {
            Some(sha)
        } else {
            // A repo without commits yet: compare with the empty tree.
            let is_repo = std::process::Command::new("git").args(["rev-parse", "--git-dir"]).current_dir(&dir).output().map(|o| o.status.success()).unwrap_or(false);
            is_repo.then(|| crate::git::EMPTY_TREE.to_string())
        };
        if let Some(b) = base {
            st.db.conn().execute("UPDATE chats SET base_sha=?2 WHERE id=?1", params![id, b])?;
        }
    }
    let chat = get_chat(&st.db, id)?;
    st.emit_project(chat.project_id, Event::ChatCreated { chat: chat.clone() });
    Ok(chat)
}

pub async fn delete_chat(st: &St, chat: &Chat) -> Result<()> {
    crate::runner::drop_session(st, chat.id);
    let running: Vec<i64> = {
        let c = st.db.conn();
        let mut q = c.prepare("SELECT id FROM runs WHERE chat_id=?1 AND status IN ('running','queued')")?;
        let v = q.query_map([chat.id], |r| r.get(0))?.filter_map(Result::ok).collect();
        v
    };
    for r in running {
        let _ = crate::runner::cancel_run(st, r).await;
    }
    if chat.worktree_branch.is_some() {
        remove_worktree(st, chat, false).await.ok();
    }
    st.db.conn().execute("DELETE FROM chats WHERE id=?1", [chat.id])?;
    st.emit_project(chat.project_id, Event::ChatDeleted { chat_id: chat.id, project_id: chat.project_id });
    Ok(())
}

/// Merge a worktree chat's branch back into the project's current branch.
pub async fn merge_worktree(st: &St, chat: &Chat) -> Result<String> {
    let Some(branch) = &chat.worktree_branch else { bail!("this chat does not use a worktree") };
    let project = store::get_project(&st.db, chat.project_id)?;
    let wt = Path::new(&chat.cwd);
    crate::git::commit_all(wt, &format!("cloud-ai: {}", chat.title)).await?;
    let root = Path::new(&project.path);
    let o = crate::git::run(root, &["merge", "--no-ff", "--no-edit", branch]).await?;
    if !o.ok {
        let _ = crate::git::run(root, &["merge", "--abort"]).await;
        bail!("merge failed (conflicts?): {}", o.stderr.trim());
    }
    Ok(o.stdout)
}

pub async fn remove_worktree(st: &St, chat: &Chat, delete_branch: bool) -> Result<()> {
    let project = store::get_project(&st.db, chat.project_id)?;
    let root = Path::new(&project.path);
    crate::git::run(root, &["worktree", "remove", "--force", &chat.cwd]).await?;
    if delete_branch {
        if let Some(b) = &chat.worktree_branch {
            crate::git::run(root, &["branch", "-D", b]).await?;
        }
    }
    Ok(())
}
