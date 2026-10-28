import { useQuery } from "@tanstack/react-query";
import { ChevronRight, FileMinus, FilePen, FilePlus, Info, Loader2, RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import type { ChangedFile, ChatChanges } from "../lib/types";
import { Diff } from "./Diff";
import { cx, IconButton } from "./ui";

export function useChanges(chatId: number, enabled = true) {
  return useQuery({
    queryKey: ["changes", chatId],
    queryFn: () => api.get<ChatChanges>(`/api/chats/${chatId}/changes`),
    enabled,
    staleTime: 10_000,
  });
}

const STATUS: Record<ChangedFile["status"], { icon: React.ReactNode; label: string; cls: string }> = {
  added: { icon: <FilePlus className="size-4" />, label: "Added", cls: "text-ok" },
  modified: { icon: <FilePen className="size-4" />, label: "Modified", cls: "text-warn" },
  deleted: { icon: <FileMinus className="size-4" />, label: "Deleted", cls: "text-danger" },
};

/** "+12 −3" with coloured numbers. */
export function LineCounts({ add, del, className }: { add: number; del: number; className?: string }) {
  return (
    <span className={cx("tabular-nums text-[12px] whitespace-nowrap", className)}>
      <span className="text-ok">+{add}</span> <span className="text-danger">−{del}</span>
    </span>
  );
}

function FileDiff({ chatId, path }: { chatId: number; path: string }) {
  const q = useQuery({
    queryKey: ["change-diff", chatId, path],
    queryFn: () => api.get<{ diff: string }>(`/api/chats/${chatId}/changes/diff?path=${encodeURIComponent(path)}`),
  });
  if (q.isLoading) return <div className="p-3 text-xs text-muted flex items-center gap-2"><Loader2 className="size-3.5 animate-spin" /> Loading diff…</div>;
  if (q.isError) return <div className="p-3 text-xs text-danger">{(q.error as Error).message}</div>;
  return <Diff text={q.data?.diff ?? ""} className="max-h-[60vh]" />;
}

function Row({ f, chatId, open, onToggle }: { f: ChangedFile; chatId: number; open: boolean; onToggle: () => void }) {
  const s = STATUS[f.status];
  const slash = f.path.lastIndexOf("/");
  const dir = slash >= 0 ? f.path.slice(0, slash + 1) : "";
  const name = slash >= 0 ? f.path.slice(slash + 1) : f.path;
  return (
    <div className="rounded-xl border border-border bg-panel overflow-hidden">
      <button onClick={onToggle} className="w-full flex items-center gap-2 px-3 py-2 text-left hover:bg-panel-2" title={`${s.label}: ${f.path}`}>
        <ChevronRight className={cx("size-3.5 text-muted shrink-0 transition", open && "rotate-90")} />
        <span className={cx("shrink-0", s.cls)} aria-label={s.label}>
          {s.icon}
        </span>
        <span className="min-w-0 flex-1 truncate text-[13px] font-mono">
          <span className="text-muted">{dir}</span>
          {name}
        </span>
        {f.binary ? <span className="text-[11px] text-muted">binary</span> : <LineCounts add={f.additions} del={f.deletions} />}
      </button>
      {open && (
        <div className="border-t border-border p-2">
          {f.binary ? <div className="text-xs text-muted p-2">Binary file — no text diff.</div> : <FileDiff chatId={chatId} path={f.path} />}
        </div>
      )}
    </div>
  );
}

/** Files the chat added, changed or deleted, with per-file diffs. */
export function FilesList({ chatId, focus, onFocus }: { chatId: number; focus: string | null; onFocus: (p: string | null) => void }) {
  const q = useChanges(chatId);
  if (q.isLoading) return <div className="py-10 text-center text-sm text-muted"><Loader2 className="inline size-4 animate-spin mr-2" />Looking for changes…</div>;
  if (q.isError) return <div className="py-6 text-center text-sm text-danger">{(q.error as Error).message}</div>;
  const d = q.data!;
  const t = d.totals;
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 px-1">
        <div className="flex-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px]">
          <span className="font-medium text-[13px]">
            {t.files} file{t.files === 1 ? "" : "s"}
          </span>
          {t.added > 0 && <span className="text-ok">{t.added} added</span>}
          {t.modified > 0 && <span className="text-warn">{t.modified} modified</span>}
          {t.deleted > 0 && <span className="text-danger">{t.deleted} deleted</span>}
          {t.files > 0 && <LineCounts add={t.additions} del={t.deletions} />}
        </div>
        <IconButton title="Refresh" onClick={() => q.refetch()}>
          <RefreshCw className={cx("size-4", q.isFetching && "animate-spin")} />
        </IconButton>
      </div>
      {d.shared && (
        <div className="flex gap-2 rounded-lg bg-panel-2 px-3 py-2 text-[12px] text-muted">
          <Info className="size-3.5 mt-0.5 shrink-0" />
          <span>
            This chat shares the project folder, so the list shows every change since the chat started, including changes made elsewhere. Files this chat edited are marked with a dot.
          </span>
        </div>
      )}
      {d.mode === "tools" && t.files > 0 && (
        <div className="flex gap-2 rounded-lg bg-panel-2 px-3 py-2 text-[12px] text-muted">
          <Info className="size-3.5 mt-0.5 shrink-0" />
          <span>No git here, so the list comes from the agent's edit tools. Changes made by shell commands may be missing.</span>
        </div>
      )}
      {t.files === 0 ? (
        <div className="text-center text-sm text-muted py-10 px-4">No files changed yet. Files this chat adds, edits or deletes show up here with their diffs.</div>
      ) : (
        d.files.map((f) => (
          <div key={f.path} className="relative">
            {d.shared && f.by_chat && <span className="absolute -left-1 top-3.5 size-1.5 rounded-full bg-accent" title="Edited by this chat" />}
            <Row f={f} chatId={chatId} open={focus === f.path} onToggle={() => onFocus(focus === f.path ? null : f.path)} />
          </div>
        ))
      )}
    </div>
  );
}
