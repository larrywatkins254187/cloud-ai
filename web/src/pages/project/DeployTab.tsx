import { useQuery } from "@tanstack/react-query";
import { QueryStatus } from "../../components/Loadable";
import { History, Rocket } from "lucide-react";
import { useEffect, useRef } from "react";
import { Link } from "react-router";
import { deployLabel, deployTone, useProjectDeploys, useTicker } from "../../components/DeployStatus";
import { Badge, Button, Card, Empty, useAction, useConfirm } from "../../components/ui";
import { api } from "../../lib/api";
import { useIsAdmin } from "../../lib/auth";
import { dateTime, duration } from "../../lib/format";
import type { Deploy, Project } from "../../lib/types";

export function DeployLog({ id }: { id: number }) {
  const q = useQuery({ queryKey: ["deploy", id], queryFn: () => api.get<Deploy>(`/api/deploys/${id}`) });
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    ref.current?.scrollTo(0, ref.current.scrollHeight);
  }, [q.data?.log]);
  if (q.data === undefined) return <QueryStatus q={q} compact title="Deploy log" />;
  return (
    <pre ref={ref} className="bg-code border border-border rounded-lg p-3 text-[12px] leading-5 font-mono max-h-[50dvh] overflow-auto scroll-thin whitespace-pre-wrap">
      {q.data?.log || "…"}
    </pre>
  );
}

export default function DeployTab({ project }: { project: Project }) {
  const admin = useIsAdmin();
  const ask = useConfirm();
  const list = useProjectDeploys(project.id);
  const { busy, run } = useAction();
  const latest = list.data?.[0];
  const running = latest?.status === "running";
  useTicker(running);

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
    const ok = await ask({ title: "Deploy project", message: `Deploy ${project.name} now?`, confirmLabel: "Deploy" });
    if (ok) await run(() => api.post<{ id: number }>(`/api/projects/${project.id}/deploys`), "Deploy started");
  };

  return (
    <div className="space-y-4">
      <Card className="p-4 flex flex-wrap items-center gap-3">
        <div className="min-w-0">
          <div className="font-medium">{project.deploy_target === "remote" ? `Deploy over SSH (${project.deploy_method})` : "Deploy on this server"}</div>
          <div className="text-xs text-muted font-mono truncate">{project.deploy_command || "(no command)"}</div>
        </div>
        <div className="flex-1" />
        <Button variant="primary" icon={<Rocket className="size-4" />} loading={busy || running} onClick={deploy}>
          {running ? "Deploying…" : "Deploy now"}
        </Button>
      </Card>
      <Card>
        <div className="px-4 py-2.5 border-b border-border text-sm font-medium flex flex-wrap items-center gap-2">
          {running ? "Current deploy" : "Latest deploy"}
          {latest && (
            <>
              <Badge tone={deployTone(latest.status) as any}>{deployLabel(latest)}</Badge>
              <span className="text-xs text-muted font-normal">
                {latest.commit_sha && <code>{latest.commit_sha.slice(0, 8)} · </code>}
                {latest.triggered_by} · {dateTime(latest.created_at)} · {duration(latest.created_at, latest.finished_at)}
              </span>
            </>
          )}
          <div className="flex-1" />
          <Link to={`/projects/${project.id}?tab=history`} className="inline-flex items-center gap-1 text-xs text-accent font-normal">
            <History className="size-3.5" /> All deploys & rollback
          </Link>
        </div>
        {list.data === undefined ? (
          <div className="p-4">
            <QueryStatus q={list} compact />
          </div>
        ) : latest ? (
          <div className="p-4 space-y-2">
            {latest.health && <div className="text-xs text-ok">Health: {latest.health}</div>}
            <DeployLog id={latest.id} />
          </div>
        ) : (
          <div className="p-6 text-sm text-muted text-center">No deploys yet</div>
        )}
      </Card>
    </div>
  );
}
