import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, RefreshCw, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { timeAgo, tokens } from "../lib/format";
import type { ChatContext, ContextItem } from "../lib/types";
import { cx, IconButton } from "./ui";

type ContextData = { context: ChatContext | null; live: boolean };

/** Colour per category id; unknown ones get a neutral grey. */
const COLORS: Record<string, string> = {
  messages: "#3b82f6",
  used: "#3b82f6",
  system_tools: "#f97316",
  mcp_tools: "#14b8a6",
  system_prompt: "#eab308",
  skills: "#a3a3a3",
  memory_files: "#a855f7",
  custom_agents: "#ec4899",
  slash_commands: "#84cc16",
  mcp_instructions: "#06b6d4",
};
const color = (id: string, kind: string) => COLORS[id] ?? (kind === "free" ? "transparent" : "#737373");

function pct(n: number, max: number) {
  if (!max) return "0%";
  const p = (n * 100) / max;
  return p > 0 && p < 0.1 ? "<0.1%" : `${p.toFixed(1)}%`;
}

export function useChatContext(chatId: number) {
  return useQuery({ queryKey: ["context", chatId], queryFn: () => api.get<ContextData>(`/api/chats/${chatId}/context`), staleTime: 60_000 });
}

/** Small ring in the chat header showing how full the context window is. */
function Ring({ value }: { value: number }) {
  const r = 7;
  const c = 2 * Math.PI * r;
  const v = Math.min(100, Math.max(0, value));
  const tone = v >= 85 ? "var(--danger)" : v >= 65 ? "var(--warn)" : "var(--accent)";
  return (
    <svg viewBox="0 0 18 18" className="size-4 -rotate-90" aria-hidden>
      <circle cx="9" cy="9" r={r} fill="none" stroke="currentColor" strokeOpacity="0.2" strokeWidth="2.5" />
      <circle cx="9" cy="9" r={r} fill="none" stroke={tone} strokeWidth="2.5" strokeDasharray={`${(v / 100) * c} ${c}`} strokeLinecap="round" />
    </svg>
  );
}

function Section({ title, total, items, max, count = true }: { title: string; total?: number; items: ContextItem[]; max: number; count?: boolean }) {
  const [open, setOpen] = useState(false);
  if (!items.length) return null;
  return (
    <div>
      <button onClick={() => setOpen(!open)} className="w-full flex items-center gap-1.5 py-1.5 text-[13px] text-muted hover:text-text">
        <ChevronRight className={cx("size-3.5 transition", open && "rotate-90")} />
        <span className="flex-1 text-left">{title}</span>
        {count && <span className="tabular-nums">{items.length}</span>}
        {total !== undefined && <span className="tabular-nums w-14 text-right">{tokens(total)}</span>}
      </button>
      {open && (
        <div className="pl-5 pb-1 space-y-0.5 max-h-48 overflow-y-auto scroll-thin">
          {items.map((i, n) => (
            <div key={n} className="flex items-center gap-2 text-[12px]">
              <span className="flex-1 min-w-0 truncate font-mono" title={i.name}>
                {i.name}
                {i.server && <span className="text-muted"> · {i.server}</span>}
              </span>
              <span className="tabular-nums text-muted">{tokens(i.tokens)}</span>
              <span className="tabular-nums text-muted w-12 text-right">{pct(i.tokens, max)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Breakdown({ ctx, live, refreshing, onRefresh }: { ctx: ChatContext; live: boolean; refreshing: boolean; onRefresh: () => void }) {
  const max = ctx.max || 1;
  const bars = ctx.categories.filter((c) => !c.deferred && c.tokens > 0);
  const m = ctx.messages;
  const msgItems: ContextItem[] = m
    ? [
        { name: "Tool calls", tokens: m.tool_calls },
        { name: "Tool results", tokens: m.tool_results },
        { name: "Your messages", tokens: m.user },
        { name: "Assistant replies", tokens: m.assistant },
        { name: "Other", tokens: m.other },
      ].filter((i) => i.tokens > 0)
    : [];
  const sum = (l?: ContextItem[]) => (l ?? []).reduce((a, i) => a + i.tokens, 0);
  return (
    <div className="space-y-3">
      <div className="flex items-baseline justify-between">
        <div className="text-lg font-semibold tabular-nums">{Math.round(ctx.percentage)}% used</div>
        <div className="text-[13px] text-muted tabular-nums">
          {tokens(ctx.used)} / {tokens(ctx.max)}
        </div>
      </div>
      <div className="flex h-2 rounded-full overflow-hidden bg-panel-2" role="img" aria-label={`${Math.round(ctx.percentage)}% of the context window used`}>
        {bars
          .filter((c) => c.kind !== "free")
          .map((c) => (
            <div
              key={c.id}
              title={`${c.name}: ${tokens(c.tokens)}`}
              style={{
                width: `${(c.tokens * 100) / max}%`,
                background: c.kind === "buffer" ? "repeating-linear-gradient(45deg, #737373 0 3px, transparent 3px 6px)" : color(c.id, c.kind),
              }}
            />
          ))}
      </div>
      <div className="space-y-1">
        {ctx.categories
          .filter((c) => c.tokens > 0)
          .map((c) => (
            <div key={c.id} className={cx("flex items-center gap-2 text-[13px]", c.deferred && "text-muted")}>
              <span
                className={cx("size-2.5 rounded-[3px] shrink-0", c.kind === "free" && "border border-border")}
                style={{ background: c.kind === "buffer" ? "repeating-linear-gradient(45deg, #737373 0 2px, transparent 2px 4px)" : color(c.id, c.kind) }}
              />
              <span className="flex-1 min-w-0 truncate">{c.name}</span>
              <span className="tabular-nums text-muted">{tokens(c.tokens)}</span>
              <span className="tabular-nums w-12 text-right">{c.deferred ? "—" : pct(c.tokens, max)}</span>
            </div>
          ))}
      </div>
      {(ctx.mcp_tools?.length || ctx.skills?.length || ctx.memory_files?.length || ctx.agents?.length || msgItems.length) ? (
        <div className="border-t border-border pt-2">
          <Section title="Messages" total={sum(msgItems)} items={msgItems} max={max} count={false} />
          <Section title="Tool use by tool" items={m?.by_tool ?? []} max={max} />
          <Section title="MCP tools" total={sum(ctx.mcp_tools)} items={ctx.mcp_tools ?? []} max={max} />
          <Section title="Skills" total={sum(ctx.skills)} items={ctx.skills ?? []} max={max} />
          <Section title="Memory files" total={sum(ctx.memory_files)} items={ctx.memory_files ?? []} max={max} />
          <Section title="Custom agents" total={sum(ctx.agents)} items={ctx.agents ?? []} max={max} />
        </div>
      ) : null}
      <div className="flex items-center gap-2 text-[11px] text-muted border-t border-border pt-2">
        <span className="flex-1">
          {ctx.model} · updated {timeAgo(ctx.updated_at)}
          {ctx.auto_compact && ctx.auto_compact_threshold ? ` · compacts at ${tokens(ctx.auto_compact_threshold)}` : ""}
        </span>
        {ctx.tool === "claude" && live && (
          <IconButton title="Refresh now" onClick={onRefresh}>
            <RefreshCw className={cx("size-3.5", refreshing && "animate-spin")} />
          </IconButton>
        )}
      </div>
      {ctx.tool === "codex" && <div className="text-[11px] text-muted">Codex only reports the total size of the conversation, not a breakdown.</div>}
    </div>
  );
}

/** Header button with the context ring; opens the breakdown. */
export function ContextMeter({ chatId }: { chatId: number }) {
  const q = useChatContext(chatId);
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const ctx = q.data?.context;
  const refresh = async () => {
    setRefreshing(true);
    try {
      qc.setQueryData(["context", chatId], await api.get<ContextData>(`/api/chats/${chatId}/context?refresh=true`));
    } catch {
      /* keep the saved snapshot */
    } finally {
      setRefreshing(false);
    }
  };
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);
  return (
    <div className="relative" ref={box}>
      <button
        onClick={() => setOpen(!open)}
        title={ctx ? `Context: ${Math.round(ctx.percentage)}% used (${tokens(ctx.used)} of ${tokens(ctx.max)})` : "Context usage"}
        aria-label="Context usage"
        aria-expanded={open}
        className={cx("inline-flex h-9 items-center gap-1.5 rounded-lg px-2 text-muted hover:bg-panel-2 hover:text-text transition", open && "bg-panel-2 text-text")}
      >
        <Ring value={ctx?.percentage ?? 0} />
        <span className="text-xs tabular-nums">{ctx ? `${Math.round(ctx.percentage)}%` : "—"}</span>
      </button>
      {open && (
        <div className="fixed sm:absolute inset-x-3 sm:inset-x-auto top-16 sm:top-11 sm:right-0 z-40 sm:w-[360px] max-h-[75vh] overflow-y-auto scroll-thin rounded-xl border border-border bg-panel shadow-xl p-4">
          <div className="flex items-center justify-between mb-3">
            <div className="text-sm font-semibold">Context</div>
            <IconButton title="Close" onClick={() => setOpen(false)}>
              <X className="size-4" />
            </IconButton>
          </div>
          {ctx ? (
            <Breakdown ctx={ctx} live={!!q.data?.live} refreshing={refreshing} onRefresh={refresh} />
          ) : (
            <div className="text-sm text-muted py-4">
              Context usage shows up after the first reply.
              {q.data?.live && (
                <button className="block mt-2 text-accent hover:underline" onClick={refresh}>
                  {refreshing ? "Reading…" : "Read it now"}
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
