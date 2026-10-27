use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use std::collections::HashMap;
use std::sync::Arc;

/// A single SQLite connection shared behind a mutex. Every query in this app is
/// tiny, so one connection in WAL mode is plenty and keeps memory low. The lock
/// is reentrant so a helper that queries while its caller holds the connection
/// cannot deadlock.
#[derive(Clone)]
pub struct Db(Arc<ReentrantMutex<Connection>>, Arc<parking_lot::RwLock<HashMap<String, Option<String>>>>);

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
CREATE TABLE users(
  id INTEGER PRIMARY KEY,
  username TEXT NOT NULL UNIQUE COLLATE NOCASE,
  password_hash TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'user',
  totp_secret TEXT,
  totp_enabled INTEGER NOT NULL DEFAULT 0,
  disabled INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
CREATE TABLE sessions(
  token_hash TEXT PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  stage TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  user_agent TEXT NOT NULL DEFAULT ''
);
CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE servers(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  host TEXT NOT NULL,
  port INTEGER NOT NULL DEFAULT 22,
  username TEXT NOT NULL,
  auth_type TEXT NOT NULL,
  secret_enc TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE projects(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  dev_command TEXT NOT NULL DEFAULT '',
  dev_port INTEGER,
  prod_url TEXT NOT NULL DEFAULT '',
  deploy_target TEXT NOT NULL DEFAULT 'none',
  server_id INTEGER REFERENCES servers(id) ON DELETE SET NULL,
  deploy_method TEXT NOT NULL DEFAULT 'git',
  remote_path TEXT NOT NULL DEFAULT '',
  deploy_command TEXT NOT NULL DEFAULT '',
  rsync_excludes TEXT NOT NULL DEFAULT '',
  auto_commit INTEGER NOT NULL DEFAULT 1,
  health_url TEXT NOT NULL DEFAULT '',
  auto_rollback INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);
CREATE TABLE project_users(
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY(project_id, user_id)
);
CREATE TABLE chats(
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
  title TEXT NOT NULL,
  tool TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '',
  effort TEXT NOT NULL DEFAULT '',
  permission_mode TEXT NOT NULL DEFAULT 'bypass',
  cwd TEXT NOT NULL,
  worktree_branch TEXT,
  cli_session_id TEXT,
  status TEXT NOT NULL DEFAULT 'idle',
  auto_deploy INTEGER NOT NULL DEFAULT 0,
  schedule_id INTEGER,
  tg_topic_id INTEGER,
  cost_usd REAL NOT NULL DEFAULT 0,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  archived INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX chats_project ON chats(project_id, updated_at);
CREATE TABLE runs(
  id INTEGER PRIMARY KEY,
  chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  user_id INTEGER,
  source TEXT NOT NULL,
  schedule_id INTEGER,
  prompt TEXT NOT NULL,
  attachments TEXT NOT NULL DEFAULT '[]',
  chain TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL,
  error TEXT,
  result_text TEXT,
  cost_usd REAL,
  input_tokens INTEGER,
  output_tokens INTEGER,
  created_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT
);
CREATE INDEX runs_status ON runs(status, id);
CREATE INDEX runs_chat ON runs(chat_id, id);
CREATE TABLE messages(
  id INTEGER PRIMARY KEY,
  chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  run_id INTEGER,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX messages_chat ON messages(chat_id, id);
CREATE TABLE requests(
  id INTEGER PRIMARY KEY,
  chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  run_id INTEGER,
  kind TEXT NOT NULL,
  data TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  answer TEXT,
  answered_by TEXT,
  created_at TEXT NOT NULL,
  answered_at TEXT
);
CREATE TABLE attachments(
  id INTEGER PRIMARY KEY,
  user_id INTEGER,
  filename TEXT NOT NULL,
  path TEXT NOT NULL,
  mime TEXT NOT NULL,
  size INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE schedules(
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
  name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  kind TEXT NOT NULL,
  run_at TEXT,
  time_of_day TEXT,
  weekdays TEXT,
  cron TEXT,
  tool TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '',
  effort TEXT NOT NULL DEFAULT '',
  permission_mode TEXT NOT NULL DEFAULT 'bypass',
  mode TEXT NOT NULL DEFAULT 'new',
  chat_id INTEGER,
  prompts TEXT NOT NULL,
  use_worktree INTEGER NOT NULL DEFAULT 0,
  auto_deploy INTEGER NOT NULL DEFAULT 0,
  last_run_at TEXT,
  next_run_at TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE models(
  tool TEXT NOT NULL,
  id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  efforts TEXT NOT NULL DEFAULT '[]',
  default_effort TEXT,
  is_default INTEGER NOT NULL DEFAULT 0,
  sort INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(tool, id)
);
CREATE TABLE deploys(
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  triggered_by TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'deploy',
  status TEXT NOT NULL,
  commit_sha TEXT,
  prev_sha TEXT,
  log TEXT NOT NULL DEFAULT '',
  health TEXT,
  created_at TEXT NOT NULL,
  finished_at TEXT
);
CREATE INDEX deploys_project ON deploys(project_id, id);
CREATE TABLE events(
  id INTEGER PRIMARY KEY,
  level TEXT NOT NULL,
  message TEXT NOT NULL,
  created_at TEXT NOT NULL
);
"#,
    // Telegram forum topic a deploy reports into (NULL = the group's General topic).
    "ALTER TABLE deploys ADD COLUMN tg_thread_id INTEGER;",
    // v3: who sent a prompt from outside the panel (Telegram), plus indexes for hot lookups.
    r#"
ALTER TABLE runs ADD COLUMN author TEXT;
CREATE INDEX IF NOT EXISTS requests_chat ON requests(chat_id, status);
CREATE INDEX IF NOT EXISTS requests_run ON requests(run_id, status);
CREATE INDEX IF NOT EXISTS requests_status ON requests(status);
CREATE INDEX IF NOT EXISTS chats_topic ON chats(tg_topic_id);
CREATE INDEX IF NOT EXISTS chats_updated ON chats(archived, updated_at);
CREATE INDEX IF NOT EXISTS schedules_project ON schedules(project_id);
"#,
    // v4: token usage per run and model, for the Usage page. It keeps its own copy of
    // project/user/chat ids (no cascade) so history survives deleting a chat or project.
    r#"
CREATE TABLE run_usage(
  id INTEGER PRIMARY KEY,
  run_id INTEGER NOT NULL,
  chat_id INTEGER NOT NULL,
  project_id INTEGER NOT NULL,
  user_id INTEGER,
  author TEXT,
  schedule_id INTEGER,
  source TEXT NOT NULL DEFAULT '',
  tool TEXT NOT NULL,
  model TEXT NOT NULL,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  web_search_requests INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL,
  legacy INTEGER NOT NULL DEFAULT 0,
  at TEXT NOT NULL
);
CREATE INDEX run_usage_at ON run_usage(at);
CREATE INDEX run_usage_run ON run_usage(run_id);
INSERT INTO run_usage(run_id,chat_id,project_id,user_id,author,schedule_id,source,tool,model,input_tokens,output_tokens,cost_usd,legacy,at)
  SELECT r.id, r.chat_id, c.project_id, r.user_id, r.author, r.schedule_id, r.source, c.tool,
         CASE WHEN c.model='' THEN 'default' ELSE c.model END,
         COALESCE(r.input_tokens,0), COALESCE(r.output_tokens,0),
         CASE WHEN c.tool='codex' THEN NULL ELSE r.cost_usd END, 1,
         COALESCE(r.finished_at, r.started_at, r.created_at)
  FROM runs r JOIN chats c ON c.id=r.chat_id
  WHERE COALESCE(r.input_tokens,0) + COALESCE(r.output_tokens,0) > 0 OR COALESCE(r.cost_usd,0) > 0;
"#,
];

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        let db = Db(Arc::new(ReentrantMutex::new(conn)), Default::default());
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let c = self.conn();
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = i as i64 + 1;
            if v > version {
                let tx = c.unchecked_transaction()?;
                tx.execute_batch(sql)?;
                tx.execute_batch(&format!("PRAGMA user_version={v}"))?;
                tx.commit()?;
            }
        }
        Ok(())
    }

    pub fn conn(&self) -> ReentrantMutexGuard<'_, Connection> {
        self.0.lock()
    }

    /// Settings are read on hot paths (every event), so they are cached in
    /// memory; all writes go through `set_setting`, which updates the cache.
    pub fn get_setting(&self, key: &str) -> Option<String> {
        if let Some(v) = self.1.read().get(key) {
            return v.clone();
        }
        let v: Option<String> = self
            .conn()
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        self.1.write().insert(key.to_string(), v.clone());
        v
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn().execute(
            "INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        self.1.write().insert(key.to_string(), Some(value.to_string()));
        Ok(())
    }

    pub fn log_event(&self, level: &str, message: &str) {
        let _ = self.conn().execute(
            "INSERT INTO events(level,message,created_at) VALUES(?1,?2,?3)",
            params![level, message, crate::util::now()],
        );
    }
}
