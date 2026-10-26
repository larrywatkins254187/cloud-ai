//! Row types and the queries shared by several modules.
use crate::db::Db;
use crate::util::now;
use anyhow::Result;
use rusqlite::{params, OptionalExtension, Row};
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub totp_enabled: bool,
    pub disabled: bool,
    pub created_at: String,
}

impl User {
    pub const COLS: &'static str = "id,username,role,totp_enabled,disabled,created_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<User> {
        Ok(User {
            id: r.get(0)?,
            username: r.get(1)?,
            role: r.get(2)?,
            totp_enabled: r.get::<_, i64>(3)? != 0,
            disabled: r.get::<_, i64>(4)? != 0,
            created_at: r.get(5)?,
        })
    }
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

pub fn get_user(db: &Db, id: i64) -> Result<User> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM users WHERE id=?1", User::COLS), [id], User::from_row)?)
}

pub fn username(db: &Db, id: Option<i64>) -> String {
    id.and_then(|id| get_user(db, id).ok()).map(|u| u.username).unwrap_or_else(|| "system".into())
}

#[derive(Clone, Debug, Serialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub description: String,
    pub dev_command: String,
    pub dev_port: Option<i64>,
    pub prod_url: String,
    pub deploy_target: String,
    pub server_id: Option<i64>,
    pub deploy_method: String,
    pub remote_path: String,
    pub deploy_command: String,
    pub rsync_excludes: String,
    pub auto_commit: bool,
    pub health_url: String,
    pub auto_rollback: bool,
    pub created_at: String,
}

impl Project {
    pub const COLS: &'static str = "id,name,path,description,dev_command,dev_port,prod_url,deploy_target,server_id,deploy_method,remote_path,deploy_command,rsync_excludes,auto_commit,health_url,auto_rollback,created_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Project> {
        Ok(Project {
            id: r.get(0)?,
            name: r.get(1)?,
            path: r.get(2)?,
            description: r.get(3)?,
            dev_command: r.get(4)?,
            dev_port: r.get(5)?,
            prod_url: r.get(6)?,
            deploy_target: r.get(7)?,
            server_id: r.get(8)?,
            deploy_method: r.get(9)?,
            remote_path: r.get(10)?,
            deploy_command: r.get(11)?,
            rsync_excludes: r.get(12)?,
            auto_commit: r.get::<_, i64>(13)? != 0,
            health_url: r.get(14)?,
            auto_rollback: r.get::<_, i64>(15)? != 0,
            created_at: r.get(16)?,
        })
    }
    pub fn deployable(&self) -> bool {
        self.deploy_target != "none" && !self.deploy_command.trim().is_empty()
            || self.deploy_target == "remote" && self.deploy_method == "rsync"
    }
}

pub fn get_project(db: &Db, id: i64) -> Result<Project> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM projects WHERE id=?1", Project::COLS), [id], Project::from_row)?)
}

pub fn list_projects(db: &Db, user: &User) -> Result<Vec<Project>> {
    let c = db.conn();
    let mut out = Vec::new();
    if user.is_admin() {
        let mut st = c.prepare(&format!("SELECT {} FROM projects ORDER BY name COLLATE NOCASE", Project::COLS))?;
        for p in st.query_map([], Project::from_row)? {
            out.push(p?);
        }
    } else {
        let cols = Project::COLS.split(',').map(|c| format!("p.{c}")).collect::<Vec<_>>().join(",");
        let mut st = c.prepare(&format!(
            "SELECT {cols} FROM projects p JOIN project_users pu ON pu.project_id=p.id WHERE pu.user_id=?1 ORDER BY p.name COLLATE NOCASE"
        ))?;
        for p in st.query_map([user.id], Project::from_row)? {
            out.push(p?);
        }
    }
    Ok(out)
}

pub fn can_access_project(db: &Db, user: &User, project_id: i64) -> bool {
    if user.is_admin() {
        return true;
    }
    db.conn()
        .query_row(
            "SELECT 1 FROM project_users WHERE project_id=?1 AND user_id=?2",
            params![project_id, user.id],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
}

#[derive(Clone, Debug, Serialize)]
pub struct Chat {
    pub id: i64,
    pub project_id: i64,
    pub user_id: Option<i64>,
    pub title: String,
    pub tool: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    pub cwd: String,
    pub worktree_branch: Option<String>,
    pub cli_session_id: Option<String>,
    pub status: String,
    pub auto_deploy: bool,
    pub schedule_id: Option<i64>,
    pub tg_topic_id: Option<i64>,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl Chat {
    pub const COLS: &'static str = "id,project_id,user_id,title,tool,model,effort,permission_mode,cwd,worktree_branch,cli_session_id,status,auto_deploy,schedule_id,tg_topic_id,cost_usd,input_tokens,output_tokens,archived,created_at,updated_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Chat> {
        Ok(Chat {
            id: r.get(0)?,
            project_id: r.get(1)?,
            user_id: r.get(2)?,
            title: r.get(3)?,
            tool: r.get(4)?,
            model: r.get(5)?,
            effort: r.get(6)?,
            permission_mode: r.get(7)?,
            cwd: r.get(8)?,
            worktree_branch: r.get(9)?,
            cli_session_id: r.get(10)?,
            status: r.get(11)?,
            auto_deploy: r.get::<_, i64>(12)? != 0,
            schedule_id: r.get(13)?,
            tg_topic_id: r.get(14)?,
            cost_usd: r.get(15)?,
            input_tokens: r.get(16)?,
            output_tokens: r.get(17)?,
            archived: r.get::<_, i64>(18)? != 0,
            created_at: r.get(19)?,
            updated_at: r.get(20)?,
        })
    }
}

pub fn get_chat(db: &Db, id: i64) -> Result<Chat> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM chats WHERE id=?1", Chat::COLS), [id], Chat::from_row)?)
}

pub fn set_chat_status(db: &Db, id: i64, status: &str) -> Result<()> {
    db.conn().execute("UPDATE chats SET status=?2, updated_at=?3 WHERE id=?1", params![id, status, now()])?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub id: i64,
    pub chat_id: i64,
    pub user_id: Option<i64>,
    pub source: String,
    pub schedule_id: Option<i64>,
    pub prompt: String,
    pub attachments: Value,
    pub chain: Value,
    pub status: String,
    pub error: Option<String>,
    pub result_text: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Display name of a sender outside the panel (e.g. "@bob (Telegram)").
    pub author: Option<String>,
}

impl Run {
    pub const COLS: &'static str = "id,chat_id,user_id,source,schedule_id,prompt,attachments,chain,status,error,result_text,cost_usd,input_tokens,output_tokens,created_at,started_at,finished_at,author";
    pub fn from_row(r: &Row) -> rusqlite::Result<Run> {
        let att: String = r.get(6)?;
        let chain: String = r.get(7)?;
        Ok(Run {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            user_id: r.get(2)?,
            source: r.get(3)?,
            schedule_id: r.get(4)?,
            prompt: r.get(5)?,
            attachments: serde_json::from_str(&att).unwrap_or(Value::Array(vec![])),
            chain: serde_json::from_str(&chain).unwrap_or(Value::Array(vec![])),
            status: r.get(8)?,
            error: r.get(9)?,
            result_text: r.get(10)?,
            cost_usd: r.get(11)?,
            input_tokens: r.get(12)?,
            output_tokens: r.get(13)?,
            created_at: r.get(14)?,
            started_at: r.get(15)?,
            finished_at: r.get(16)?,
            author: r.get(17)?,
        })
    }
}

pub fn get_run(db: &Db, id: i64) -> Result<Run> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM runs WHERE id=?1", Run::COLS), [id], Run::from_row)?)
}

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub id: i64,
    pub chat_id: i64,
    pub run_id: Option<i64>,
    pub kind: String,
    pub data: Value,
    pub created_at: String,
}

impl Message {
    pub const COLS: &'static str = "id,chat_id,run_id,kind,data,created_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Message> {
        let d: String = r.get(4)?;
        Ok(Message {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            run_id: r.get(2)?,
            kind: r.get(3)?,
            data: serde_json::from_str(&d).unwrap_or(Value::Null),
            created_at: r.get(5)?,
        })
    }
}

pub fn insert_message(db: &Db, chat_id: i64, run_id: Option<i64>, kind: &str, data: &Value) -> Result<Message> {
    let created_at = now();
    let c = db.conn();
    c.execute(
        "INSERT INTO messages(chat_id,run_id,kind,data,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![chat_id, run_id, kind, data.to_string(), created_at],
    )?;
    Ok(Message { id: c.last_insert_rowid(), chat_id, run_id, kind: kind.into(), data: data.clone(), created_at })
}

#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub id: i64,
    pub chat_id: i64,
    pub run_id: Option<i64>,
    pub kind: String,
    pub data: Value,
    pub status: String,
    pub answer: Option<Value>,
    pub answered_by: Option<String>,
    pub created_at: String,
    pub answered_at: Option<String>,
}

impl Request {
    pub const COLS: &'static str = "id,chat_id,run_id,kind,data,status,answer,answered_by,created_at,answered_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Request> {
        let d: String = r.get(4)?;
        let a: Option<String> = r.get(6)?;
        Ok(Request {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            run_id: r.get(2)?,
            kind: r.get(3)?,
            data: serde_json::from_str(&d).unwrap_or(Value::Null),
            status: r.get(5)?,
            answer: a.and_then(|a| serde_json::from_str(&a).ok()),
            answered_by: r.get(7)?,
            created_at: r.get(8)?,
            answered_at: r.get(9)?,
        })
    }
}

pub fn get_request(db: &Db, id: i64) -> Result<Request> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM requests WHERE id=?1", Request::COLS), [id], Request::from_row)?)
}

#[derive(Clone, Debug, Serialize)]
pub struct Schedule {
    pub id: i64,
    pub project_id: i64,
    pub user_id: Option<i64>,
    pub name: String,
    pub enabled: bool,
    pub kind: String,
    pub run_at: Option<String>,
    pub time_of_day: Option<String>,
    pub weekdays: Option<String>,
    pub cron: Option<String>,
    pub tool: String,
    pub model: String,
    pub effort: String,
    pub permission_mode: String,
    pub mode: String,
    pub chat_id: Option<i64>,
    pub prompts: Value,
    pub use_worktree: bool,
    pub auto_deploy: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
}

impl Schedule {
    pub const COLS: &'static str = "id,project_id,user_id,name,enabled,kind,run_at,time_of_day,weekdays,cron,tool,model,effort,permission_mode,mode,chat_id,prompts,use_worktree,auto_deploy,last_run_at,next_run_at,created_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Schedule> {
        let p: String = r.get(16)?;
        Ok(Schedule {
            id: r.get(0)?,
            project_id: r.get(1)?,
            user_id: r.get(2)?,
            name: r.get(3)?,
            enabled: r.get::<_, i64>(4)? != 0,
            kind: r.get(5)?,
            run_at: r.get(6)?,
            time_of_day: r.get(7)?,
            weekdays: r.get(8)?,
            cron: r.get(9)?,
            tool: r.get(10)?,
            model: r.get(11)?,
            effort: r.get(12)?,
            permission_mode: r.get(13)?,
            mode: r.get(14)?,
            chat_id: r.get(15)?,
            prompts: serde_json::from_str(&p).unwrap_or(Value::Array(vec![])),
            use_worktree: r.get::<_, i64>(17)? != 0,
            auto_deploy: r.get::<_, i64>(18)? != 0,
            last_run_at: r.get(19)?,
            next_run_at: r.get(20)?,
            created_at: r.get(21)?,
        })
    }
}

pub fn get_schedule(db: &Db, id: i64) -> Result<Schedule> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM schedules WHERE id=?1", Schedule::COLS), [id], Schedule::from_row)?)
}

#[derive(Clone, Debug, Serialize)]
pub struct Server {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: i64,
    pub username: String,
    pub auth_type: String,
    pub created_at: String,
}

impl Server {
    pub const COLS: &'static str = "id,name,host,port,username,auth_type,created_at";
    pub fn from_row(r: &Row) -> rusqlite::Result<Server> {
        Ok(Server {
            id: r.get(0)?,
            name: r.get(1)?,
            host: r.get(2)?,
            port: r.get(3)?,
            username: r.get(4)?,
            auth_type: r.get(5)?,
            created_at: r.get(6)?,
        })
    }
    pub fn alias(&self) -> String {
        format!("cai-{}", crate::util::slug(&self.name))
    }
}

pub fn get_server(db: &Db, id: i64) -> Result<Server> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM servers WHERE id=?1", Server::COLS), [id], Server::from_row)?)
}

#[derive(Clone, Debug, Serialize)]
pub struct Deploy {
    pub id: i64,
    pub project_id: i64,
    pub triggered_by: String,
    pub kind: String,
    pub status: String,
    pub commit_sha: Option<String>,
    pub prev_sha: Option<String>,
    pub log: String,
    pub health: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub tg_thread_id: Option<i64>,
}

impl Deploy {
    pub const COLS: &'static str = "id,project_id,triggered_by,kind,status,commit_sha,prev_sha,log,health,created_at,finished_at,tg_thread_id";
    /// Same columns with an empty log, for lists.
    pub const LIST_COLS: &'static str = "id,project_id,triggered_by,kind,status,commit_sha,prev_sha,'' AS log,health,created_at,finished_at,tg_thread_id";
    pub fn from_row(r: &Row) -> rusqlite::Result<Deploy> {
        Ok(Deploy {
            id: r.get(0)?,
            project_id: r.get(1)?,
            triggered_by: r.get(2)?,
            kind: r.get(3)?,
            status: r.get(4)?,
            commit_sha: r.get(5)?,
            prev_sha: r.get(6)?,
            log: r.get(7)?,
            health: r.get(8)?,
            created_at: r.get(9)?,
            finished_at: r.get(10)?,
            tg_thread_id: r.get(11)?,
        })
    }
}

pub fn get_deploy(db: &Db, id: i64) -> Result<Deploy> {
    Ok(db.conn().query_row(&format!("SELECT {} FROM deploys WHERE id=?1", Deploy::COLS), [id], Deploy::from_row)?)
}

/// Small facts about the installed CLIs (version, account) kept in `settings`.
pub fn set_cli_info(st: &crate::state::St, key: &str, v: &Value) {
    let _ = st.db.set_setting(&format!("info.{key}"), &v.to_string());
}

pub fn get_cli_info(db: &Db, key: &str) -> Value {
    db.get_setting(&format!("info.{key}")).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}
