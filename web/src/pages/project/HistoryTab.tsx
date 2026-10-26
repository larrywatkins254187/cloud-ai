import { useInfiniteQuery } from "@tanstack/react-query";
import { queryGate, SkeletonList } from "../../components/Loadable";
import { ChevronDown, ChevronRight, History, RotateCcw } from "lucide-react";
import { useMemo, useState } from "react";
import { deployTone, useTicker } from "../../components/DeployStatus";
import { Badge, Button, Card, cx, Empty, useAction, useConfirm } from "../../components/ui";
import { api } from "../../lib/api";
import { dateTime, duration } from "../../lib/format";
import type { Deploy, Project } from "../../lib/types";
import { DeployLog } from "./DeployTab";

const PAGE = 30;
// Table columns on wide screens; rows fall back to a compact flex layout on phones.
const COLS = "md:grid md:grid-cols-[16px_150px_80px_1fr_160px_70px_150px]";
type Filter = "all" | "deploy" | "rollback" | "failed";
const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "deploy", label: "Deploys" },
  { id: "rollback", label: "Rollbacks" },
  { id: "failed", label: "Failed" },
];

/** Every deploy and rollback of the project, with logs and one-click rollback. */
export default function HistoryTab({ project }: { project: Project }) {
  const ask = useConfirm();
  const { busy, run } = useAction();
  const [open, setOpen] = useState<number | null>(null);
  const [filter, setFilter] = useState<Filter>("all");
  const q = useInfiniteQuery({
    queryKey: ["deploy-history", project.id],
    queryFn: ({ pageParam }) => api.get<Deploy[]>(`/api/projects/${project.id}/deploys?limit=${PAGE}${pageParam ? `&before=${pageParam}` : ""}`),
    initialPageParam: 0,
    getNextPageParam: (last) => (last.length === PAGE ? last[last.length - 1].id : undefined),
  });
  const all = useMemo(() => q.data?.pages.flat() ?? [], [q.data]);
  const running = all.some((d) => d.status === "running");
  useTicker(running);
  // The version currently live: the newest successful deploy or rollback.
  const live = all.find((d) => d.status === "success");
  const rows = all.filter((d) => (filter === "all" ? true : filter === "failed" ? d.status === "failed" : d.kind === filter));

  const rollback = async (d: Deploy) => {
    const sha = d.commit_sha!.slice(0, 8);
    const ok = await ask({
      title: "Roll back",
      message: `Redeploy ${project.name} at commit ${sha} (from ${dateTime(d.created_at)})?\n\nThis runs the deploy again with that version of the code.`,
      confirmLabel: `Roll back to ${sha}`,
      danger: true,
    });
    if (!ok) return;
    const r = await run(() => api.post<{ id: number }>(`/api/deploys/${d.id}/rollback`, {}), "Rollback started");
    if (r) setOpen(r.id);
  };

  const gate = queryGate(q, <SkeletonList rows={5} className="w-full" />);
  if (gate) return gate;
  if (!all.length)
    return (
      <Card>
        <Empty icon={<History className="size-8" />} title="No deploys yet">
          {project.deployable ? "Deploys and rollbacks of this project show up here." : "Configure deploys in the Settings tab first."}
        </Empty>
      </Card>
    );

  return (
    <Card>
      <div className="px-4 py-2.5 border-b border-border flex flex-wrap items-center gap-2">
        <div className="text-sm font-medium flex items-center gap-2 mr-2">
          <History className="size-4" /> Deploy history
        </div>
        {FILTERS.map((f) => (
          <button
            key={f.id}
            onClick={() => setFilter(f.id)}
            className={cx("px-2.5 py-1 rounded-md text-xs", filter === f.id ? "bg-accent-soft text-accent font-medium" : "text-muted hover:bg-panel-2")}
          >
            {f.label}
          </button>
        ))}
        <div className="flex-1" />
        {live?.commit_sha && (
          <span className="text-xs text-muted">
            Live: <code>{live.commit_sha.slice(0, 8)}</code>
          </span>
        )}
      </div>
      {/* header row, wide screens only */}
      <div className={cx(COLS, "hidden px-4 py-2 gap-3 text-[11px] uppercase tracking-wide text-muted border-b border-border")}>
        <span />
        <span>Status</span>
        <span>Commit</span>
        <span>Triggered by</span>
        <span>Started</span>
        <span>Took</span>
        <span />
      </div>
      {rows.length === 0 && <div className="p-6 text-sm text-muted text-center">Nothing matches this filter</div>}
      <div className="divide-y divide-border">
        {rows.map((d) => {
          const isLive = live?.id === d.id;
          const canRollback = d.status === "success" && !!d.commit_sha && d.commit_sha !== live?.commit_sha;
          return (
            <div key={d.id}>
              <div
                onClick={() => setOpen(open === d.id ? null : d.id)}
                className={cx(COLS, "flex items-center gap-3 px-4 py-2.5 text-sm hover:bg-panel-2 cursor-pointer")}
              >
                <button
                  className="text-muted shrink-0"
                  aria-expanded={open === d.id}
                  aria-label={open === d.id ? "Hide log" : "Show log"}
                  onClick={(e) => {
                    e.stopPropagation();
                    setOpen(open === d.id ? null : d.id);
                  }}
                >
                  {open === d.id ? <ChevronDown className="size-4" /> : <ChevronRight className="size-4" />}
                </button>
                <div className="min-w-0 flex-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 md:contents">
                  <span className="flex items-center gap-1.5 flex-wrap">
                    <Badge tone={deployTone(d.status) as any}>{d.status}</Badge>
                    {d.kind === "rollback" && <Badge tone="warn">rollback</Badge>}
                    {isLive && <Badge tone="ok">live</Badge>}
                  </span>
                  <code className="text-xs">{d.commit_sha?.slice(0, 8) ?? "—"}</code>
                  <span className="hidden md:block truncate text-muted text-xs">{d.triggered_by}</span>
                  <span className="hidden md:block text-muted text-xs">{dateTime(d.created_at)}</span>
                  <span className="hidden md:block text-muted text-xs tabular-nums">{duration(d.created_at, d.finished_at)}</span>
                  <span className="md:hidden basis-full text-xs text-muted truncate">
                    {d.triggered_by} · {dateTime(d.created_at)} · {duration(d.created_at, d.finished_at)}
                  </span>
                </div>
                <span className="flex justify-end shrink-0">
                  {canRollback && (
                    <Button
                      size="sm"
                      variant="outline"
                      icon={<RotateCcw className="size-3.5" />}
                      disabled={busy || running}
                      title={running ? "Wait for the running deploy to finish" : undefined}
                      onClick={(e) => {
                        e.stopPropagation();
                        rollback(d);
                      }}
                    >
                      Roll back here
                    </Button>
                  )}
                </span>
              </div>
              {open === d.id && (
                <div className="px-4 pb-4 space-y-2">
                  {d.health && <div className="text-xs text-ok">Health: {d.health}</div>}
                  <DeployLog id={d.id} />
                </div>
              )}
            </div>
          );
        })}
      </div>
      {q.hasNextPage && (
        <div className="p-3 text-center border-t border-border">
          <Button size="sm" variant="ghost" loading={q.isFetchingNextPage} onClick={() => q.fetchNextPage()}>
            Load older deploys
          </Button>
        </div>
      )}
    </Card>
  );
}
