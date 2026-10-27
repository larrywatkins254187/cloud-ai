import { Bot, CheckCircle2, ChevronRight, CircleSlash, Loader2, Square, SquareTerminal, X, XCircle } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import type { AgentProgress, AgentStatus, SubAgent } from "../lib/agents";
import { api } from "../lib/api";
import { duration, tokens } from "../lib/format";
import type { Msg } from "../lib/types";
import { Badge, Button, cx, IconButton, useConfirm, useToast } from "./ui";

export function AgentStatusIcon({ status, className }: { status: AgentStatus; className?: string }) {
  if (status === "running") return <Loader2 className={cx("size-4 animate-spin text-accent shrink-0", className)} />;
  if (status === "completed") return <CheckCircle2 className={cx("size-4 text-ok shrink-0", className)} />;
  if (status === "failed") return <XCircle className={cx("size-4 text-danger shrink-0", className)} />;
  return <CircleSlash className={cx("size-4 text-muted shrink-0", className)} />;
}

const LABEL: Record<AgentStatus, string> = { running: "Running", completed: "Done", failed: "Failed", stopped: "Stopped" };
const TONE: Record<AgentStatus, "accent" | "ok" | "danger" | "neutral"> = { running: "accent", completed: "ok", failed: "danger", stopped: "neutral" };

export function AgentStatusBadge({ status }: { status: AgentStatus }) {
  return <Badge tone={TONE[status]}>{LABEL[status]}</Badge>;
}

/** Re-render every second while something is running, so durations tick. */
function useTick(active: boolean) {
  const [, set] = useState(0);
  useEffect(() => {
    if (!active) return;
    const t = setInterval(() => set((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, [active]);
}

/** Stop one sub-agent; the server falls back to stopping the prompt when the CLI can't. */
export function useStopAgent(chatId: number) {
  const ask = useConfirm();
  const toast = useToast();
  return async (a: SubAgent) => {
    const ok = await ask({
      title: "Stop sub-agent",
      message: `Stop “${a.description}”? If it can't be stopped on its own, the whole running prompt is stopped instead.`,
      confirmLabel: "Stop",
      danger: true,
    });
    if (!ok) return;
    try {
      const r = await api.post<{ stopped: "agent" | "turn" }>(`/api/chats/${chatId}/agents/stop`, { tool_use_id: a.ref });
      toast(r.stopped === "agent" ? "Sub-agent stopped" : "Stopping the running prompt", "ok");
    } catch (e: any) {
      toast(e.message, "danger");
    }
  };
}

const AgentRow = memo(function AgentRow({
  a,
  progress,
  open,
  onToggle,
  onStop,
  renderSteps,
}: {
  a: SubAgent;
  progress?: AgentProgress;
  open: boolean;
  onToggle: () => void;
  onStop: () => void;
  renderSteps: (msgs: Msg[]) => React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (open) ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [open]);
  const [showPrompt, setShowPrompt] = useState(false);
  const running = a.status === "running";
  const lastTool = (running && progress?.last_tool) || a.lastTool;
  const toolCount = Math.max(a.toolCount, progress?.tool_uses ?? 0);
  return (
    <div ref={ref} className={cx("rounded-xl border bg-panel", open ? "border-accent/50" : "border-border")}>
      <button onClick={onToggle} className="w-full flex items-start gap-2 px-3 py-2.5 text-left hover:bg-panel-2 rounded-xl">
        <ChevronRight className={cx("size-3.5 text-muted mt-0.5 transition shrink-0", open && "rotate-90")} />
        <AgentStatusIcon status={a.status} className="mt-0.5" />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium truncate">{a.description}</div>
          <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-muted mt-0.5">
            {a.type && <span>{a.type}</span>}
            <span>{duration(a.startedAt, a.endedAt)}</span>
            {toolCount > 0 && <span>{toolCount} tool{toolCount === 1 ? "" : "s"}</span>}
            {!!progress?.tokens && <span>{tokens(progress.tokens)} tok</span>}
            {a.background && <span>background</span>}
          </div>
          {running && lastTool && <div className="text-[11px] text-accent truncate mt-0.5">Using {lastTool}</div>}
        </div>
        <AgentStatusBadge status={a.status} />
      </button>
      {open && (
        <div className="border-t border-border p-3 space-y-3">
          {running && (
            <Button size="sm" variant="outline" icon={<Square className="size-3.5" />} onClick={onStop} className="text-danger">
              Stop this agent
            </Button>
          )}
          {a.prompt && (
            <div>
              <button onClick={() => setShowPrompt(!showPrompt)} className="text-xs text-muted hover:text-text flex items-center gap-1">
                <ChevronRight className={cx("size-3 transition", showPrompt && "rotate-90")} /> Task given
              </button>
              {showPrompt && <pre className="mt-1 text-[12px] whitespace-pre-wrap break-words bg-code rounded-lg p-2.5 max-h-60 overflow-auto scroll-thin">{a.prompt}</pre>}
            </div>
          )}
          {a.summary && !running && <div className="text-[13px] whitespace-pre-wrap break-words text-muted line-clamp-6">{a.summary}</div>}
          {a.children.length > 0 ? (
            <div className="space-y-2">{renderSteps(a.children)}</div>
          ) : (
            <div className="text-xs text-muted">{running ? "Starting…" : "No steps were reported."}</div>
          )}
        </div>
      )}
    </div>
  );
});

export function AgentsPanel({
  chatId,
  agents,
  progress,
  focus,
  onFocus,
  onClose,
  renderSteps,
  tab = "agents",
  onTab,
  bgCount = 0,
  bgRunning = 0,
  background,
}: {
  chatId: number;
  agents: SubAgent[];
  progress: Record<string, AgentProgress>;
  focus: string | null;
  onFocus: (ref: string | null) => void;
  onClose: () => void;
  renderSteps: (msgs: Msg[]) => React.ReactNode;
  /** With `onTab`, the panel also has a Background tab showing `background`. */
  tab?: "agents" | "background";
  onTab?: (t: "agents" | "background") => void;
  bgCount?: number;
  bgRunning?: number;
  background?: React.ReactNode;
}) {
  const running = agents.filter((a) => a.status === "running").length;
  useTick(running > 0);
  const stop = useStopAgent(chatId);
  // Newest first; running ones on top.
  const list = [...agents].reverse().sort((a, b) => Number(b.status === "running") - Number(a.status === "running"));
  return (
    <div className="flex flex-col h-full min-h-0 bg-bg">
      <div className="flex items-center gap-1 px-2 h-12 border-b border-border shrink-0">
        {onTab ? (
          <div role="tablist" className="flex-1 flex items-center gap-1 min-w-0">
            {(
              [
                { id: "agents", label: "Sub-agents", icon: <Bot className="size-4" />, n: agents.length, r: running },
                { id: "background", label: "Background", icon: <SquareTerminal className="size-4" />, n: bgCount, r: bgRunning },
              ] as const
            ).map((t) => (
              <button
                key={t.id}
                role="tab"
                aria-selected={tab === t.id}
                onClick={() => onTab(t.id)}
                className={cx(
                  "inline-flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-[13px] whitespace-nowrap",
                  tab === t.id ? "bg-panel-2 text-text font-medium" : "text-muted hover:text-text",
                )}
              >
                {t.icon}
                {t.label}
                {t.n > 0 && (
                  <span className={cx("text-[11px] tabular-nums rounded-md px-1", t.r > 0 ? "bg-accent-soft text-accent" : "bg-panel-2 text-muted")}>
                    {t.r > 0 ? `${t.r}/${t.n}` : t.n}
                  </span>
                )}
              </button>
            ))}
          </div>
        ) : (
          <>
            <Bot className="size-4 text-muted ml-1" />
            <div className="text-sm font-medium flex-1 ml-1">
              Sub-agents <span className="text-muted font-normal">· {running > 0 ? `${running} running · ` : ""}{agents.length} total</span>
            </div>
          </>
        )}
        <IconButton title="Close" onClick={onClose}>
          <X className="size-4" />
        </IconButton>
      </div>
      <div className="flex-1 overflow-y-auto scroll-thin p-3 space-y-2">
        {onTab && tab === "background" ? (
          background
        ) : list.length === 0 ? (
          <div className="text-center text-sm text-muted py-10 px-4">
            No sub-agents in this chat yet. When Claude or Codex hands work to a sub-agent, it shows up here with its live status and steps.
          </div>
        ) : (
          list.map((a) => (
            <AgentRow
              key={a.ref}
              a={a}
              progress={progress[a.ref]}
              open={focus === a.ref}
              onToggle={() => onFocus(focus === a.ref ? null : a.ref)}
              onStop={() => stop(a)}
              renderSteps={renderSteps}
            />
          ))
        )}
      </div>
    </div>
  );
}
