use std::path::PathBuf;

/// Process-level configuration from the command line / environment.
/// Everything else (limits, Telegram, paths of the CLIs, …) lives in the
/// `settings` table so it can be changed from the admin panel.
#[derive(Clone, Debug)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: String,
}

impl Config {
    pub fn from_env_and_args(args: &mut Vec<String>) -> Config {
        let mut data_dir = std::env::var("CLOUD_AI_DATA").unwrap_or_else(|_| "/var/lib/cloud-ai".into());
        let mut bind = std::env::var("CLOUD_AI_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
        let mut rest = Vec::new();
        let mut it = args.drain(..);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--data" => data_dir = it.next().unwrap_or(data_dir),
                "--bind" => bind = it.next().unwrap_or(bind),
                _ => rest.push(a),
            }
        }
        drop(it);
        *args = rest;
        Config { data_dir: PathBuf::from(data_dir), bind }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("cloud-ai.db")
    }
    pub fn uploads_dir(&self) -> PathBuf {
        self.data_dir.join("uploads")
    }
    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir.join("worktrees")
    }
    pub fn ssh_dir(&self) -> PathBuf {
        self.data_dir.join("ssh")
    }
    pub fn screenshots_dir(&self) -> PathBuf {
        self.data_dir.join("screenshots")
    }
    pub fn setup_token_path(&self) -> PathBuf {
        self.data_dir.join("setup-token")
    }
}
