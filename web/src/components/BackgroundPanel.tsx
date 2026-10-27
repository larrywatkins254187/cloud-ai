import { useQuery } from "@tanstack/react-query";
import { Activity, ChevronRight, Copy, Skull, Square, SquareTerminal } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import type { BgTask } from "../lib/background";
import { api } from "../lib/api";
import { duration } from "../lib/format";
import { AgentStatusBadge, AgentStatusIcon } from "./AgentsPanel";
import { Button, cx, useConfirm, useToast } from "./ui";

type Output = { output: string; size: number; truncated: boolean; found: boolean };

/** Stop (ask Claude Code to end it) or kill (end its processes now) a background task. */
export function useBackgroundActions(chatId: number) {
  const ask = useConfirm();
  const toast = useToast();
  const act = async (t: BgTask, force: boolean) => {
    const what = t.kind === "monitor" ? "monitor" : "command";
    const ok = await ask({
      title: force ? `Kill ${what}` : `Stop ${what}`,
      message: force
        ? `Kill “${t.description}” right away? Its processes get SIGKILL and can't clean up.`
        : `Ask Claude Code to stop “${t.description}”? Use Kill if it doesn't end.`,
      confirmLabel: force ? "Kill" : "Stop",
      danger: true,
    });
    if (!ok) return;
    try {
      const r = await api.post<{ action: string; killed?: number }>(`/api/chats/${chatId}/background/${t.taskId}/stop`, { force });
      if (r.action === "stop") toast("Stopping…", "ok");
      else toast(r.killed ? `Killed ${r.killed} process group${r.killed === 1 ? "" : "s"}` : "No running processes found for it", r.killed ? "ok" : "info");
    } catch (e: any) {
      toast(e.message, "danger");
    }
  };
  return { stop: (t: BgTask) => act(t, false), kill: (t: BgTask) => act(t, true) };
}

/** Output of a task, refreshed while it runs and the viewer is open. */
function OutputView({ chatId, t }: { chatId: number; t: BgTask }) {
  const running = t.status === "running";
  const q = useQuery({
    queryKey: ["bg-output", chatId, t.taskId],
    queryFn: () => api.get<Output>(`/api/chats/${chatId}/background/${t.taskId}/output?tail=131072`),
    refetchInterval: running ? 1500 : false,
  });
  const pre = useRef<HTMLPreElement>(null);
  const stick = useRef(true);
  const toast = useToast();
  useEffect(() => {
    const el = pre.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [q.data?.output]);
  const d = q.data;
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-[11px] text-muted">
        <span>
          Output{d?.truncated ? " (last 128 KB)" : ""}
          {running && " · live"}
        </span>
        {d?.output && (
          <button className="inline-flex items-center gap-1 hover:text-text" onClick={() => navigator.clipboard?.writeText(d.output).then(() => toast("Copied", "ok"))}>
            <Copy className="size-3" /> Copy
          </button>
        )}
      </div>
      <pre
        ref={pre}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="text-[11.5px] leading-snug font-mono whitespace-pre-wrap break-all bg-code border border-border rounded-lg p-2.5 max-h-80 min-h-16 overflow-auto scroll-thin"
      >
        {d === undefined ? (q.isError ? "Couldn't load the output." : "Loading…") : !d.found ? "No output file yet." : d.output || (running ? "Waiting for output…" : "(no output)")}
      </pre>
    </div>
  );
}

const Row = memo(function Row({
  chatId,
  t,
  open,
  onToggle,
  onStop,
  onKill,
}: {
  chatId: number;
  t: BgTask;
  open: boolean;
  onToggle: () => void;
  onStop: () => void;
  onKill: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (open) ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [open]);
  const running = t.status === "running";
  const Icon = t.kind === "monitor" ? Activity : SquareTerminal;
  return (
    <div ref={ref} className={cx("rounded-xl border bg-panel", open ? "border-accent/50" : "border-border")}>
      <button onClick={onToggle} className="w-full flex items-start gap-2 px-3 py-2.5 text-left hover:bg-panel-2 rounded-xl">
        <ChevronRight className={cx("size-3.5 text-muted mt-0.5 transition shrink-0", open && "rotate-90")} />
        <AgentStatusIcon status={t.status} className="mt-0.5" />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium truncate">{t.description}</div>
          <div className="flex items-center gap-1.5 text-[11px] text-muted mt-0.5 min-w-0">
            <Icon className="size-3 shrink-0" />
            <span className="shrink-0">{t.kind === "monitor" ? "Monitor" : "Shell"}</span>
            <span className="shrink-0">· {duration(t.startedAt, t.endedAt)}</span>
          </div>
          {t.command && <div className="text-[11px] font-mono text-muted truncate mt-0.5">$ {t.command}</div>}
        </div>
        <AgentStatusBadge status={t.status} />
      </button>
      {open && (
        <div className="border-t border-border p-3 space-y-3">
          {running && (
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="outline" icon={<Square className="size-3.5" />} onClick={onStop}>
                Stop
              </Button>
              <Button size="sm" variant="outline" className="text-danger" icon={<Skull className="size-3.5" />} onClick={onKill}>
                Kill
              </Button>
            </div>
          )}
          {t.command && (
            <pre className="text-[11.5px] font-mono whitespace-pre-wrap break-all bg-code rounded-lg p-2.5 max-h-40 overflow-auto scroll-thin">$ {t.command}</pre>
          )}
          {t.summary && !running && <div className="text-[13px] text-muted whitespace-pre-wrap break-words">{t.summary}</div>}
          <OutputView chatId={chatId} t={t} />
        </div>
      )}
    </div>
  );
});

export function BackgroundList({
  chatId,
  tasks,
  focus,
  onFocus,
  isCodex,
}: {
  chatId: number;
  tasks: BgTask[];
  focus: string | null;
  onFocus: (taskId: string | null) => void;
  isCodex?: boolean;
}) {
  const { stop, kill } = useBackgroundActions(chatId);
  const running = tasks.some((t) => t.status === "running");
  const [, tick] = useState(0);
  useEffect(() => {
    if (!running) return;
    const i = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(i);
  }, [running]);
  const list = [...tasks].reverse().sort((a, b) => Number(b.status === "running") - Number(a.status === "running"));
  if (!list.length)
    return (
      <div className="text-center text-sm text-muted py-10 px-4">
        {isCodex
          ? "Background commands are shown for Claude Code chats."
          : "No background commands yet. When Claude runs a shell command in the background or starts a Monitor, it shows up here with its live output, and you can stop or kill it."}
      </div>
    );
  return (
    <div className="space-y-2">
      {list.map((t) => (
        <Row key={t.taskId} chatId={chatId} t={t} open={focus === t.taskId} onToggle={() => onFocus(focus === t.taskId ? null : t.taskId)} onStop={() => stop(t)} onKill={() => kill(t)} />
      ))}
    </div>
  );
}
