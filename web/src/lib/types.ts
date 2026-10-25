export type Role = "admin" | "user";

export interface User {
  id: number;
  username: string;
  role: Role;
  totp_enabled: boolean;
  disabled: boolean;
  created_at: string;
  projects?: number[];
}

export interface Project {
  id: number;
  name: string;
  path: string;
  description: string;
  dev_command: string;
  dev_port: number | null;
  prod_url: string;
  deploy_target: "none" | "local" | "remote";
  server_id: number | null;
  deploy_method: "git" | "rsync";
  remote_path: string;
  deploy_command: string;
  rsync_excludes: string;
  auto_commit: boolean;
  health_url: string;
  auto_rollback: boolean;
  created_at: string;
  deployable?: boolean;
  chat_count?: number;
  active_count?: number;
  users?: number[];
  is_git?: boolean;
  exists?: boolean;
}

export type Tool = "claude" | "codex";
export type PermMode = "bypass" | "edits" | "ask" | "plan";
export type ChatStatus = "idle" | "queued" | "running" | "waiting" | "error";

export interface Chat {
  id: number;
  project_id: number;
  user_id: number | null;
  user?: string;
  title: string;
  tool: Tool;
  model: string;
  effort: string;
  permission_mode: PermMode;
  cwd: string;
  worktree_branch: string | null;
  cli_session_id: string | null;
  status: ChatStatus;
  auto_deploy: boolean;
  schedule_id: number | null;
  cost_usd: number;
  input_tokens: number;
  output_tokens: number;
  archived: boolean;
  created_at: string;
  updated_at: string;
  project?: { id: number; name: string; path: string; deployable: boolean };
  active_runs?: Run[];
  pending_requests?: Req[];
}

export interface Attachment {
  id: number;
  filename: string;
  path: string;
  mime: string;
  size: number;
}

export interface Run {
  id: number;
  chat_id: number;
  user_id: number | null;
  source: string;
  schedule_id: number | null;
  prompt: string;
  attachments: Attachment[];
  chain: string[];
  status: "queued" | "running" | "done" | "error" | "cancelled";
  error: string | null;
  result_text: string | null;
  cost_usd: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

export interface Msg {
  id: number;
  chat_id: number;
  run_id: number | null;
  kind: "user" | "assistant" | "thinking" | "tool_use" | "tool_result" | "error" | "system" | "plan" | "todo";
  data: any;
  created_at: string;
}

export interface Question {
  id: string;
  header: string;
  question: string;
  multi: boolean;
  allow_other: boolean;
  options: { label: string; description: string }[];
}

export interface Req {
  id: number;
  chat_id: number;
  run_id: number | null;
  kind: "approval" | "question";
  data: any;
  status: "pending" | "answered" | "expired";
  answer: any;
  answered_by: string | null;
  created_at: string;
  answered_at: string | null;
}

export interface Model {
  id: string;
  name: string;
  description: string;
  efforts: string[];
  default_effort: string | null;
  is_default: boolean;
}

export interface Models {
  claude: Model[];
  codex: Model[];
  refreshed_at: string | null;
}

export interface Schedule {
  id: number;
  project_id: number;
  project?: string;
  user?: string;
  user_id: number | null;
  name: string;
  enabled: boolean;
  kind: "once" | "daily" | "weekly" | "cron";
  run_at: string | null;
  time_of_day: string | null;
  weekdays: string | null;
  cron: string | null;
  tool: Tool;
  model: string;
  effort: string;
  permission_mode: PermMode;
  mode: "new" | "continue";
  chat_id: number | null;
  prompts: string[];
  use_worktree: boolean;
  auto_deploy: boolean;
  last_run_at: string | null;
  next_run_at: string | null;
  next_run_local?: string;
  created_at: string;
}

export interface Server {
  id: number;
  name: string;
  host: string;
  port: number;
  username: string;
  auth_type: "app_key" | "key" | "password";
  created_at: string;
}

export interface Deploy {
  id: number;
  project_id: number;
  triggered_by: string;
  kind: "deploy" | "rollback";
  status: "running" | "success" | "failed";
  commit_sha: string | null;
  prev_sha: string | null;
  log: string;
  health: string | null;
  created_at: string;
  finished_at: string | null;
}

export type ServerEvent =
  | { type: "chat_created"; chat: Chat }
  | { type: "chat_updated"; chat: Chat }
  | { type: "chat_deleted"; chat_id: number; project_id: number }
  | { type: "message"; message: Msg }
  | { type: "delta"; chat_id: number; run_id: number; kind: "text" | "thinking" | "activity"; text: string }
  | { type: "run_queued"; run: Run }
  | { type: "run_started"; run: Run }
  | { type: "run_finished"; run: Run }
  | { type: "request_created"; request: Req }
  | { type: "request_resolved"; request: Req }
  | { type: "deploy_updated"; deploy: Deploy }
  | { type: "deploy_log"; deploy_id: number; project_id: number; line: string }
  | { type: "notice"; level: string; text: string }
  | { type: "resync" }
  | { type: string; [k: string]: any };
