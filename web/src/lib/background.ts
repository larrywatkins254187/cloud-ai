import type { AgentStatus } from "./agents";
import type { Msg } from "./types";

/** A background shell command (Bash with run_in_background) or a Monitor, started by Claude Code. */
export interface BgTask {
  taskId: string;
  /** Id of the tool call that started it. */
  ref: string;
  kind: "shell" | "monitor";
  command: string;
  description: string;
  status: AgentStatus;
  summary: string;
  startedAt: string;
  endedAt: string | null;
}

export function isBackgroundCall(m: Msg) {
  return m.kind === "tool_use" && (m.data?.name === "Monitor" || (m.data?.name === "Bash" && m.data?.input?.run_in_background === true));
}

/** Background tasks of a chat, from its "shell" messages (the first starts a task, later ones update it). */
export function buildBackground(messages: Msg[]): BgTask[] {
  const tasks = new Map<string, BgTask>();
  for (const m of messages) {
    if (m.kind !== "shell" || !m.data?.task_id) continue;
    const d = m.data;
    const t = tasks.get(d.task_id);
    if (!t) {
      tasks.set(d.task_id, {
        taskId: d.task_id,
        ref: d.ref ?? "",
        kind: d.kind === "monitor" ? "monitor" : "shell",
        command: d.command ?? "",
        description: d.description || d.command || "Background command",
        status: (d.status as AgentStatus) ?? "running",
        summary: d.summary ?? "",
        startedAt: m.created_at,
        endedAt: d.status && d.status !== "running" ? m.created_at : null,
      });
      continue;
    }
    if (d.status) {
      t.status = d.status as AgentStatus;
      t.endedAt = d.status === "running" ? null : t.endedAt ?? m.created_at;
    }
    if (d.summary) t.summary = d.summary;
  }
  return [...tasks.values()];
}

/** Tasks the messages still call running but whose Claude process no longer runs them ended without a report. */
export function withLiveStatus(tasks: BgTask[], live: { alive: boolean; running: string[] } | undefined, fetchedAt: number): BgTask[] {
  if (!live) return tasks;
  // Only judge tasks that existed when the live list was fetched.
  return tasks.map((t) => (t.status === "running" && new Date(t.startedAt).getTime() < fetchedAt - 2000 && !live.running.includes(t.taskId) ? { ...t, status: "stopped" as AgentStatus, summary: t.summary || "No longer running" } : t));
}
