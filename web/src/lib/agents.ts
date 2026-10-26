import type { Msg } from "./types";

export type AgentStatus = "running" | "completed" | "failed" | "stopped";

export interface SubAgent {
  /** Id of the tool call that started the agent (Claude Task/Agent, Codex spawnAgent). */
  ref: string;
  description: string;
  type: string;
  prompt: string;
  status: AgentStatus;
  background: boolean;
  summary: string;
  startedAt: string;
  endedAt: string | null;
  /** Messages the agent produced (its own tool calls, text, thinking). */
  children: Msg[];
  toolCount: number;
  lastTool: string;
}

export interface AgentProgress {
  ref?: string | null;
  task_id?: string;
  last_tool?: string | null;
  tokens?: number | null;
  tool_uses?: number | null;
  duration_ms?: number | null;
  description?: string | null;
}

const AGENT_TOOLS = new Set(["Task", "Agent"]);

export function isAgentTool(name: string | undefined) {
  return !!name && AGENT_TOOLS.has(name);
}

/**
 * Sub-agents of a chat from its messages. Status comes from the CLI's status
 * messages and the result of the call that started the agent (background agents
 * report "running" again after that result, and finish with a later status).
 */
export function buildAgents(messages: Msg[], chatActive: boolean): SubAgent[] {
  const agents = new Map<string, SubAgent>();
  const order: string[] = [];
  const byTask = new Map<string, string>();
  for (const m of messages) {
    if (m.kind === "tool_use" && isAgentTool(m.data?.name) && !m.data?.parent) {
      const input = m.data.input ?? {};
      agents.set(m.data.id, {
        ref: m.data.id,
        description: input.description || String(input.prompt ?? "").slice(0, 80) || "Sub-agent",
        type: input.subagent_type ?? "",
        prompt: input.prompt ?? "",
        status: "running",
        background: !!input.run_in_background,
        summary: "",
        startedAt: m.created_at,
        endedAt: null,
        children: [],
        toolCount: 0,
        lastTool: "",
      });
      order.push(m.data.id);
      continue;
    }
    if (m.kind === "agent") {
      const ref = m.data?.ref || (m.data?.task_id && byTask.get(m.data.task_id));
      if (m.data?.task_id && m.data?.ref) byTask.set(m.data.task_id, m.data.ref);
      const a = ref && agents.get(ref);
      if (!a) continue;
      a.status = (m.data.status as AgentStatus) ?? a.status;
      if (m.data.background) a.background = true;
      if (m.data.summary) a.summary = m.data.summary;
      a.endedAt = a.status === "running" ? null : m.created_at;
      continue;
    }
    if (m.kind === "tool_result" && !m.data?.parent) {
      const a = agents.get(m.data?.tool_use_id);
      // The call's result ends an agent still marked running; it doesn't override a
      // reported end (a stopped agent's call returns an error afterwards).
      if (a && a.status === "running") {
        const out = typeof m.data.output === "string" ? m.data.output : "";
        // An error result caused by stopping the prompt reads as stopped, not failed.
        a.status = !m.data.is_error ? "completed" : /interrupt|stopped|cancel/i.test(out.slice(0, 300)) ? "stopped" : "failed";
        a.endedAt = m.created_at;
        if (!a.summary && typeof m.data.output === "string") a.summary = m.data.output.slice(0, 400);
      }
      continue;
    }
    const parent = m.data?.parent;
    if (parent) {
      const a = agents.get(parent);
      if (!a) continue;
      a.children.push(m);
      if (m.kind === "tool_use") {
        a.toolCount++;
        a.lastTool = m.data?.name ?? "";
      }
    }
  }
  return order.map((r) => {
    const a = agents.get(r)!;
    // A foreground agent can't outlive its prompt: if nothing reported an end, it was stopped.
    if (a.status === "running" && !chatActive && !a.background) return { ...a, status: "stopped" as AgentStatus };
    return a;
  });
}
