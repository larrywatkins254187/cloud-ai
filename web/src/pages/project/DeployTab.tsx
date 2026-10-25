import { useQuery } from "@tanstack/react-query";
import { History, Rocket, RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { Badge, Button, Card, cx, Empty, Spinner, useAction } from "../../components/ui";
import { api } from "../../lib/api";
import { useIsAdmin } from "../../lib/auth";
import { dateTime, duration } from "../../lib/format";
import type { Deploy, Project } from "../../lib/types";

function statusTone(s: string) {
  return s === "success" ? "ok" : s === "failed" ? "danger" : "accent";
}

function DeployLog({ id }: { id: number }) {
  const q = useQuery({ queryKey: ["deploy", id], queryFn: () => api.get<Deploy>(`/api/deploys/${id}`) });
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    ref.current?.scrollTo(0, ref.current.scrollHeight);
  }, [q.data?.log]);
  if (q.isLoading) return <Spinner />;
  return (
    <pre ref={ref} className="bg-code border border-border rounded-lg p-3 text-[12px] leading-5 font-mono max-h-[50dvh] overflow-auto scroll-thin whitespace-pre-wrap">
      {q.data?.log || "…"}
    </pre>
  );
}

export default function DeployTab({ project }: { project: Project }) {
  const admin = useIsAdmin();
  const list = useQuery({ queryKey: ["deploys", project.id], queryFn: () => api.get<Deploy[]>(`/api/projects/${project.id}/deploys`) });
  const [open, setOpen] = useState<number | null>(null);
  const { busy, run } = useAction();
  const running = list.data?.find((d) => d.status === "running");
  useEffect(() => {
    if (running) setOpen(running.id);
  }, [running?.id]);

  if (!project.deployable)
    return (
      <Card>
        <Empty icon={<Rocket className="size-8" />} title="Deploy is not configured">
          {admin ? (
            <>
              Set it up in the{" "}
              <Link className="text-accent" to={`/projects/${project.id}?tab=settings`}>
                Settings tab
              </Link>
              : run a command here or ship the code to another server over SSH.
            </>
          ) : (
            "Ask an admin to configure deploys for this project."
          )}
        </Empty>
      </Card>
    );

  const deploy = async () => {
    if (!confirm(`Deploy ${project.name} now?`)) return;
    const r = await run(() => api.post<{ id: number }>(`/api/projects/${project.id}/deploys`), "Deploy started");
    if (r) setOpen(r.id);
  };
  const rollback = async (d: Deploy) => {
    if (!confirm(`Redeploy commit ${d.commit_sha?.slice(0, 8)}?`)) return;
    const r = await run(() => api.post<{ id: number }>(`/api/deploys/${d.id}/rollback`, {}), "Rollback started");
    if (r) setOpen(r.id);
  };

  return (
    <div className="space-y-4">
      <Card className="p-4 flex flex-wrap items-center gap-3">
        <div className="min-w-0">
          <div className="font-medium">{project.deploy_target === "remote" ? `Deploy over SSH (${project.deploy_method})` : "Deploy on this server"}</div>
          <div className="text-xs text-muted font-mono truncate">{project.deploy_command || "(no command)"}</div>
        </div>
        <div className="flex-1" />
        <Button variant="primary" icon={<Rocket className="size-4" />} loading={busy || !!running} onClick={deploy}>
          {running ? "Deploying…" : "Deploy now"}
        </Button>
      </Card>
      <Card>
        <div className="px-4 py-2.5 border-b border-border text-sm font-medium flex items-center gap-2">
          <History className="size-4" /> History
        </div>
        {list.data?.length === 0 && <div className="p-6 text-sm text-muted text-center">No deploys yet</div>}
        <div className="divide-y divide-border">
          {list.data?.map((d) => (
            <div key={d.id}>
              <button onClick={() => setOpen(open === d.id ? null : d.id)} className={cx("w-full flex flex-wrap items-center gap-2 px-4 py-2.5 text-sm text-left hover:bg-panel-2")}>
                <Badge tone={statusTone(d.status) as any}>{d.status}</Badge>
                {d.kind === "rollback" && <Badge tone="warn">rollback</Badge>}
                <code className="text-xs">{d.commit_sha?.slice(0, 8) ?? "—"}</code>
                <span className="text-muted text-xs">
                  {d.triggered_by} · {dateTime(d.created_at)} · {duration(d.created_at, d.finished_at)}
                </span>
                {d.health && <span className="text-xs text-ok">{d.health}</span>}
                <div className="flex-1" />
                {d.status === "success" && d.commit_sha && (
                  <span
                    role="button"
                    className="inline-flex items-center gap-1 text-xs text-accent"
                    onClick={(e) => {
                      e.stopPropagation();
                      rollback(d);
                    }}
                  >
                    <RotateCcw className="size-3.5" /> Redeploy this version
                  </span>
                )}
              </button>
              {open === d.id && (
                <div className="px-4 pb-4">
                  <DeployLog id={d.id} />
                </div>
              )}
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
