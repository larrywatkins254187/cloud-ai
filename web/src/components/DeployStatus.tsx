import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, Loader2, Rocket, RotateCcw, X, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "react-router";
import { api } from "../lib/api";
import { duration, timeAgo } from "../lib/format";
import type { Deploy } from "../lib/types";
import { Badge, Card, cx } from "./ui";

export function deployTone(s: string) {
  return s === "success" ? "ok" : s === "failed" ? "danger" : "accent";
}

export function deployLabel(d: Pick<Deploy, "kind" | "status">) {
  const what = d.kind === "rollback" ? "Rollback" : "Deploy";
  return d.status === "running" ? `${what} running` : d.status === "success" ? `${what} succeeded` : `${what} failed`;
}

/** Re-render every second while `on`, so running durations tick. */
export function useTicker(on: boolean) {
  const [, set] = useState(0);
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => set((x) => x + 1), 1000);
    return () => clearInterval(t);
  }, [on]);
}

function StatusIcon({ status, className }: { status: string; className?: string }) {
  if (status === "running") return <Loader2 className={cx("size-4 animate-spin text-accent", className)} />;
  if (status === "success") return <CheckCircle2 className={cx("size-4 text-ok", className)} />;
  return <XCircle className={cx("size-4 text-danger", className)} />;
}

/** Deploys of one project (same cache as the Deploy tab). */
export function useProjectDeploys(projectId: number | undefined, enabled = true) {
  return useQuery({
    queryKey: ["deploys", projectId],
    queryFn: () => api.get<Deploy[]>(`/api/projects/${projectId}/deploys`),
    enabled: !!projectId && enabled,
  });
}

const RECENT_MS = 15 * 60 * 1000;

/** Banner for the chat page: the project's running deploy, or the result of
 *  one that finished in the last 15 minutes. */
export function DeployBanner({ projectId, enabled }: { projectId: number; enabled: boolean }) {
  const q = useProjectDeploys(projectId, enabled);
  const [hidden, setHidden] = useState<number | null>(null);
  const d = q.data?.[0];
  const running = d?.status === "running";
  useTicker(running);
  if (!d || hidden === d.id) return null;
  if (!running && (!d.finished_at || Date.now() - new Date(d.finished_at).getTime() > RECENT_MS)) return null;
  return (
    <div
      className={cx(
        "flex items-center gap-2 px-3 sm:px-4 py-2 text-sm border-b shrink-0",
        running ? "bg-accent-soft border-accent/30" : d.status === "success" ? "bg-ok-soft border-ok/30" : "bg-danger-soft border-danger/30",
      )}
    >
      <StatusIcon status={d.status} />
      <div className="min-w-0 flex-1 truncate">
        <span className="font-medium">{running ? (d.kind === "rollback" ? "Rolling back…" : "Deploying…") : deployLabel(d)}</span>
        <span className="text-muted">
          {d.commit_sha && (
            <>
              {" "}
              · <code className="text-xs">{d.commit_sha.slice(0, 8)}</code>
            </>
          )}{" "}
          · {d.triggered_by} · {running ? duration(d.created_at) : timeAgo(d.finished_at)}
        </span>
      </div>
      <Link to={`/projects/${projectId}?tab=${running ? "deploy" : "history"}`} className="text-xs text-accent shrink-0">
        {running ? "View log" : "Details"}
      </Link>
      {!running && (
        <button className="p-1 text-muted hover:text-text shrink-0" title="Hide" aria-label="Hide" onClick={() => setHidden(d.id)}>
          <X className="size-3.5" />
        </button>
      )}
    </div>
  );
}

type ActiveDeploy = Deploy & { project_name: string };

/** Dashboard card: running deploys and the latest ones across projects. */
export function DeploysCard() {
  const q = useQuery({ queryKey: ["active-deploys"], queryFn: () => api.get<ActiveDeploy[]>("/api/deploys/active"), refetchInterval: 30_000 });
  const list = q.data ?? [];
  const running = list.filter((d) => d.status === "running");
  useTicker(running.length > 0);
  if (!list.length) return null;
  const rows = [...running, ...list.filter((d) => d.status !== "running")].slice(0, 6);
  return (
    <Card>
      <div className="px-4 py-3 border-b border-border text-sm font-medium flex items-center gap-2">
        <Rocket className="size-4" /> Deploys
        {running.length > 0 && <Badge tone="accent">{running.length} running</Badge>}
      </div>
      <div className="divide-y divide-border">
        {rows.map((d) => (
          <Link
            key={d.id}
            to={`/projects/${d.project_id}?tab=${d.status === "running" ? "deploy" : "history"}`}
            className={cx("flex items-center gap-3 px-4 py-3 hover:bg-panel-2", d.status === "running" && "bg-accent-soft/40")}
          >
            <StatusIcon status={d.status} />
            <div className="min-w-0 flex-1">
              <div className="text-sm truncate font-medium">{d.project_name}</div>
              <div className="text-xs text-muted truncate">
                {d.kind === "rollback" && (
                  <>
                    <RotateCcw className="inline size-3" /> rollback ·{" "}
                  </>
                )}
                {d.commit_sha ? <code>{d.commit_sha.slice(0, 8)}</code> : "—"} · {d.triggered_by} ·{" "}
                {d.status === "running" ? `running ${duration(d.created_at)}` : timeAgo(d.finished_at ?? d.created_at)}
              </div>
            </div>
            <Badge tone={deployTone(d.status) as any}>{d.status}</Badge>
          </Link>
        ))}
      </div>
    </Card>
  );
}
