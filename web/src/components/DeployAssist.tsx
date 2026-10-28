import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, ClipboardCheck, Loader2, Rocket, Sparkles, Square, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { PermMode, Project } from "../lib/types";
import { AgentPicker, type AgentConfig } from "./AgentPicker";
import { DeployBanner } from "./DeployStatus";
import type { ProjectDraft } from "./ProjectForm";
import { Button, Card, cx, Field, Textarea, useToast } from "./ui";

type Job = {
  id: number;
  status: "running" | "done" | "error" | "stopped";
  log: string[];
  result?: { deploy_command: string; health_url: string; summary?: string; notes?: string[] | string | null };
  error?: string;
};
type Check = { name: string; status: "ok" | "warn" | "fail"; detail: string };

const CFG_KEY = "deploy-assist:agent";
function loadCfg(): AgentConfig {
  try {
    const v = JSON.parse(localStorage.getItem(CFG_KEY) ?? "");
    if (v?.tool) return { ...v, permission_mode: "plan" as PermMode };
  } catch {
    /* ignore */
  }
  return { tool: "claude", model: "", effort: "", permission_mode: "plan" };
}

/** The deploy fields of the draft, as the API expects them. */
function target(v: ProjectDraft) {
  return {
    deploy_target: v.deploy_target ?? "none",
    server_id: v.server_id ?? null,
    remote_path: v.remote_path ?? "",
    deploy_method: v.deploy_method ?? "git",
    deploy_command: v.deploy_command ?? "",
    health_url: v.health_url ?? "",
    prod_url: v.prod_url ?? "",
  };
}

function CheckIcon({ s }: { s: Check["status"] }) {
  if (s === "ok") return <CheckCircle2 className="size-4 text-ok shrink-0 mt-0.5" />;
  if (s === "warn") return <AlertTriangle className="size-4 text-warn shrink-0 mt-0.5" />;
  return <XCircle className="size-4 text-danger shrink-0 mt-0.5" />;
}

/**
 * "Set up deploy with AI": Claude Code or Codex reads the project and the server it
 * runs on (read-only) and proposes a deploy command and health check URL. Then the
 * settings can be checked without deploying, and a real test deploy can be run.
 */
export function DeployAssist({
  project,
  v,
  set,
  save,
}: {
  project: Project;
  v: ProjectDraft;
  set: (p: ProjectDraft) => void;
  /** Saves the draft; resolves to true when it worked. */
  save: () => Promise<boolean>;
}) {
  const [cfg, setCfg] = useState<AgentConfig>(loadCfg);
  const [notes, setNotes] = useState("");
  const [jobId, setJobId] = useState<number | null>(null);
  const [starting, setStarting] = useState(false);
  const [checks, setChecks] = useState<Check[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [testing, setTesting] = useState(false);
  const [deployed, setDeployed] = useState(false);
  const toast = useToast();
  const t = target(v);
  const ready = t.deploy_target === "local" || (t.deploy_target === "remote" && !!t.server_id && !!t.remote_path.trim());

  const job = useQuery({
    queryKey: ["deploy-assist", jobId],
    queryFn: () => api.get<Job>(`/api/deploy-assist/${jobId}`),
    enabled: jobId !== null,
    refetchInterval: (q) => (q.state.data && q.state.data.status !== "running" ? false : 1500),
  });
  const j = job.data;
  const running = !!jobId && (!j || j.status === "running");

  // The settings changed, so earlier check results no longer apply.
  useEffect(() => setChecks(null), [t.deploy_target, t.server_id, t.remote_path, t.deploy_method, t.deploy_command, t.health_url]);

  const generate = async () => {
    try {
      localStorage.setItem(CFG_KEY, JSON.stringify(cfg));
    } catch {
      /* ignore */
    }
    setStarting(true);
    try {
      const r = await api.post<{ id: number }>(`/api/projects/${project.id}/deploy-assist`, { tool: cfg.tool, model: cfg.model, effort: cfg.effort, notes, ...t });
      setJobId(r.id);
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setStarting(false);
    }
  };

  const apply = () => {
    if (!j?.result) return;
    set({ ...v, deploy_command: j.result.deploy_command, health_url: j.result.health_url || v.health_url });
    toast("Filled in the deploy command and health check URL. Check them, then save.", "ok");
  };

  const check = async () => {
    setChecking(true);
    try {
      const r = await api.post<{ ok: boolean; checks: Check[] }>(`/api/projects/${project.id}/deploy-check`, t);
      setChecks(r.checks);
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setChecking(false);
    }
  };

  const testDeploy = async () => {
    if (!confirm(`Save these settings and run a real deploy of ${project.name} now?${v.auto_rollback ? " It rolls back automatically if the health check fails." : ""}`)) return;
    setTesting(true);
    try {
      if (!(await save())) return;
      await api.post(`/api/projects/${project.id}/deploys`);
      setDeployed(true);
      toast("Test deploy started", "ok");
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setTesting(false);
    }
  };

  const notesList = j?.result?.notes ? (Array.isArray(j.result.notes) ? j.result.notes : [String(j.result.notes)]) : [];
  const differs = j?.result && (j.result.deploy_command !== v.deploy_command || (j.result.health_url && j.result.health_url !== v.health_url));

  return (
    <Card className="p-5 space-y-4 max-w-3xl">
      <div>
        <div className="flex items-center gap-2 font-medium">
          <Sparkles className="size-4 text-accent" /> Set up deploy with AI
        </div>
        <p className="text-[13px] text-muted mt-1">
          Claude Code or Codex reads this project and the server it runs on (read-only, nothing is changed) and proposes a deploy command and a health check URL.
          Choose where the project runs in the Deploy section above first.
        </p>
      </div>
      <AgentPicker v={cfg} set={(c) => setCfg({ ...c, permission_mode: "plan" })} readOnly />
      <Field label="Notes for the AI (optional)" hint="e.g. “runs with pm2 as api”, “use docker compose”, “health endpoint is /status”">
        <Textarea value={notes} onChange={(e) => setNotes(e.target.value)} rows={2} placeholder="Anything it should know" />
      </Field>
      <div className="flex flex-wrap gap-2">
        {running ? (
          <Button variant="outline" icon={<Square className="size-4" />} onClick={() => jobId && api.post(`/api/deploy-assist/${jobId}/stop`).then(() => job.refetch())}>
            Stop
          </Button>
        ) : (
          <Button variant="primary" icon={<Sparkles className="size-4" />} loading={starting} disabled={!ready} onClick={generate}>
            {j ? "Generate again" : "Generate deploy command"}
          </Button>
        )}
        <Button variant="outline" icon={<ClipboardCheck className="size-4" />} loading={checking} disabled={!ready || running} onClick={check}>
          Check without deploying
        </Button>
        <Button variant="outline" icon={<Rocket className="size-4" />} loading={testing} disabled={!ready || running || !v.deploy_command?.trim()} onClick={testDeploy}>
          Save &amp; run test deploy
        </Button>
      </div>
      {!ready && <div className="text-xs text-muted">Pick “On this server” or a server and its path in the Deploy section to enable these.</div>}

      {jobId && (
        <div className="rounded-xl border border-border bg-panel-2/40 p-3 space-y-2">
          <div className="flex items-center gap-2 text-[13px] font-medium">
            {running ? <Loader2 className="size-4 animate-spin text-accent" /> : j?.status === "done" ? <CheckCircle2 className="size-4 text-ok" /> : <XCircle className="size-4 text-danger" />}
            {running ? `${cfg.tool === "codex" ? "Codex" : "Claude"} is reading the project…` : j?.status === "done" ? "Suggestion ready" : j?.status === "stopped" ? "Stopped" : "It did not work"}
          </div>
          {(j?.log.length ?? 0) > 0 && (
            <div className="font-mono text-[11px] text-muted max-h-32 overflow-y-auto scroll-thin space-y-0.5">
              {j!.log.slice(-12).map((l, i) => (
                <div key={i} className="truncate">
                  {l}
                </div>
              ))}
            </div>
          )}
          {j?.status === "error" && <div className="text-[13px] text-danger whitespace-pre-wrap break-words">{j.error}</div>}
          {j?.result && (
            <div className="space-y-3 pt-1">
              {j.result.summary && <p className="text-[13px]">{j.result.summary}</p>}
              <Field label="Deploy command">
                <pre className="text-[12px] font-mono bg-code border border-border rounded-lg p-2.5 whitespace-pre-wrap break-all">{j.result.deploy_command}</pre>
              </Field>
              <Field label="Health check URL">
                <div className="text-[13px] font-mono break-all">{j.result.health_url || <span className="text-muted">(none suggested)</span>}</div>
              </Field>
              {notesList.length > 0 && (
                <ul className="text-[12px] text-muted list-disc pl-5 space-y-0.5">
                  {notesList.map((n, i) => (
                    <li key={i}>{n}</li>
                  ))}
                </ul>
              )}
              <Button size="sm" variant={differs ? "primary" : "outline"} onClick={apply} disabled={!differs}>
                {differs ? "Use these settings" : "Settings filled in"}
              </Button>
            </div>
          )}
        </div>
      )}

      {checks && (
        <div className="rounded-xl border border-border p-3 space-y-2">
          <div className="text-[13px] font-medium">
            {checks.some((c) => c.status === "fail") ? "Some checks failed" : checks.some((c) => c.status === "warn") ? "Looks OK, with warnings" : "All checks passed"}
          </div>
          {checks.map((c, i) => (
            <div key={i} className="flex gap-2 text-[13px]">
              <CheckIcon s={c.status} />
              <div className="min-w-0">
                <span className="font-medium">{c.name}</span>
                <span className={cx("ml-1.5 whitespace-pre-wrap break-words", c.status === "fail" ? "text-danger" : "text-muted")}>{c.detail}</span>
              </div>
            </div>
          ))}
        </div>
      )}
      {deployed && <DeployBanner projectId={project.id} enabled />}
    </Card>
  );
}
