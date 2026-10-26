import { useQuery, useQueryClient } from "@tanstack/react-query";
import { queryGate } from "../../components/Loadable";
import { ArrowDownToLine, ArrowUpFromLine, GitBranch, GitCommitHorizontal, RefreshCw, Undo2 } from "lucide-react";
import { useState } from "react";
import { Diff } from "../../components/Diff";
import { Badge, Button, Card, cx, Empty, Input, Select, Spinner, useAction, useConfirm } from "../../components/ui";
import { api, qs } from "../../lib/api";
import { timeAgo } from "../../lib/format";
import type { Project } from "../../lib/types";

export default function GitTab({ project, chatId }: { project: Project; chatId?: number }) {
  const ask = useConfirm();
  const base = `/api/projects/${project.id}/git`;
  const extra = chatId ? { chat: chatId } : {};
  const status = useQuery({ queryKey: ["git", project.id, chatId, "status"], queryFn: () => api.get<any>(`${base}/status${qs(extra)}`) });
  const [sel, setSel] = useState<string | null>(null);
  const diff = useQuery({
    queryKey: ["git", project.id, chatId, "diff", sel],
    queryFn: () => api.get<{ diff: string }>(`${base}/diff${qs({ ...extra, path: sel ?? undefined })}`),
    enabled: !!status.data?.repo,
  });
  const log = useQuery({ queryKey: ["git", project.id, chatId, "log"], queryFn: () => api.get<any[]>(`${base}/log${qs(extra)}`), enabled: !!status.data?.repo });
  const branches = useQuery({ queryKey: ["git", project.id, "branches"], queryFn: () => api.get<any[]>(`${base}/branches`), enabled: !!status.data?.repo && !chatId });
  const [msg, setMsg] = useState("");
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const reload = () => qc.invalidateQueries({ queryKey: ["git", project.id] });

  const gate = queryGate(status);
  if (gate) return gate;
  if (!status.data?.repo)
    return (
      <Card>
        <Empty icon={<GitBranch className="size-8" />} title="Not a git repository">
          Run <code>git init</code> in the terminal or ask the AI to set up git.
        </Empty>
      </Card>
    );
  const s = status.data;
  const act = async (fn: () => Promise<any>, ok: string) => {
    const r = await run(fn, ok);
    reload();
    return r;
  };
  return (
    <div className="space-y-4">
      <Card className="p-4 flex flex-wrap items-center gap-3">
        <div className="flex items-center gap-2">
          <GitBranch className="size-4 text-accent" />
          {chatId ? (
            <span className="font-medium">{s.branch}</span>
          ) : (
            <Select
              className="h-8 w-auto text-sm"
              value={s.branch}
              onChange={(e) => act(() => api.post(`${base}/checkout`, { branch: e.target.value }), `Switched to ${e.target.value}`)}
            >
              {branches.data?.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </Select>
          )}
          {s.ahead > 0 && <Badge tone="accent">↑{s.ahead}</Badge>}
          {s.behind > 0 && <Badge tone="warn">↓{s.behind}</Badge>}
        </div>
        <div className="text-xs text-muted truncate">{s.remote}</div>
        <div className="flex-1" />
        <Button size="sm" variant="outline" icon={<RefreshCw className="size-4" />} onClick={reload}>
          Refresh
        </Button>
        <Button size="sm" variant="outline" icon={<ArrowDownToLine className="size-4" />} loading={busy} onClick={() => act(() => api.post(`${base}/pull`, extra), "Pulled")}>
          Pull
        </Button>
        <Button size="sm" variant="outline" icon={<ArrowUpFromLine className="size-4" />} loading={busy} onClick={() => act(() => api.post(`${base}/push`, extra), "Pushed")}>
          Push
        </Button>
        {!chatId && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              const name = prompt("New branch name");
              if (name) act(() => api.post(`${base}/checkout`, { branch: name, create: true }), `Created ${name}`);
            }}
          >
            New branch
          </Button>
        )}
      </Card>

      <div className="grid lg:grid-cols-[320px_1fr] gap-4">
        <Card className="overflow-hidden">
          <div className="px-3 py-2 border-b border-border text-sm font-medium">Changes ({s.files.length})</div>
          <div className="divide-y divide-border max-h-80 overflow-y-auto scroll-thin">
            <button onClick={() => setSel(null)} className={cx("w-full text-left px-3 py-1.5 text-sm", sel === null && "bg-accent-soft")}>
              All changes
            </button>
            {s.files.map((f: any) => (
              <div key={f.path} className={cx("group flex items-center gap-2 px-3 py-1.5 text-sm", sel === f.path && "bg-accent-soft")}>
                <span className="font-mono text-xs w-5 text-warn">{(f.index + f.worktree).trim()}</span>
                <button className="truncate flex-1 text-left" onClick={() => setSel(f.path)}>
                  {f.path}
                </button>
                <button
                  title="Discard changes"
                  className="text-muted hover:text-danger opacity-100 lg:opacity-0 group-hover:opacity-100"
                  onClick={async () => (await ask({ title: "Discard changes", message: `Discard all changes to ${f.path}?`, confirmLabel: "Discard", danger: true })) && act(() => api.post(`${base}/discard`, { ...extra, path: f.path }), "Discarded")}
                >
                  <Undo2 className="size-3.5" />
                </button>
              </div>
            ))}
          </div>
          <div className="p-3 border-t border-border space-y-2">
            <Input value={msg} onChange={(e) => setMsg(e.target.value)} placeholder="Commit message" />
            <Button
              className="w-full"
              variant="primary"
              icon={<GitCommitHorizontal className="size-4" />}
              disabled={!msg.trim() || s.files.length === 0}
              loading={busy}
              onClick={async () => {
                await act(() => api.post(`${base}/commit`, { ...extra, message: msg }), "Committed");
                setMsg("");
              }}
            >
              Commit all
            </Button>
          </div>
        </Card>
        <div className="min-w-0">{diff.isLoading ? <Spinner /> : <Diff text={diff.data?.diff ?? ""} className="max-h-[70dvh]" />}</div>
      </div>

      <Card>
        <div className="px-4 py-2.5 border-b border-border text-sm font-medium">History</div>
        <div className="divide-y divide-border max-h-96 overflow-y-auto scroll-thin">
          {log.data?.map((c) => (
            <div key={c.sha} className="px-4 py-2 text-sm flex gap-3">
              <code className="text-xs text-accent shrink-0 pt-0.5">{c.short}</code>
              <div className="min-w-0">
                <div className="truncate">{c.subject}</div>
                <div className="text-xs text-muted">
                  {c.author} · {timeAgo(c.date)}
                </div>
              </div>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
