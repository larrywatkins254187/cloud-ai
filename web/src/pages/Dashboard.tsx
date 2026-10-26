import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, Bot, Cpu, Hourglass, MemoryStick, MessageSquare, Play, RefreshCw } from "lucide-react";
import { DeploysCard } from "../components/DeployStatus";
import { Link } from "react-router";
import { Badge, Card, cx, Empty, PageHeader, StatusDot } from "../components/ui";
import { api } from "../lib/api";
import { useAuth } from "../lib/auth";
import { timeAgo, toolName } from "../lib/format";
import type { Chat } from "../lib/types";

function Meter({ label, value, max, unit, icon }: { label: string; value: number; max: number; unit: string; icon: React.ReactNode }) {
  const pct = max > 0 ? Math.min(100, (value / max) * 100) : 0;
  return (
    <Card className="p-4">
      <div className="flex items-center gap-2 text-sm text-muted">
        {icon}
        {label}
      </div>
      <div className="mt-2 text-2xl font-semibold tabular-nums">
        {Math.round(value)}
        <span className="text-sm font-normal text-muted">
          {" "}
          / {Math.round(max)} {unit}
        </span>
      </div>
      <div className="mt-2 h-1.5 rounded-full bg-panel-2 overflow-hidden">
        <div className={cx("h-full rounded-full", pct > 85 ? "bg-danger" : pct > 65 ? "bg-warn" : "bg-accent")} style={{ width: `${pct}%` }} />
      </div>
    </Card>
  );
}

function UsageBar({ label, pct, resets }: { label: string; pct: number; resets?: number | null }) {
  return (
    <div>
      <div className="flex justify-between text-xs text-muted">
        <span>{label}</span>
        <span>
          {Math.round(pct)}%{resets ? ` · resets ${new Date(resets * 1000).toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" })}` : ""}
        </span>
      </div>
      <div className="mt-1 h-1.5 rounded-full bg-panel-2 overflow-hidden">
        <div className={cx("h-full rounded-full", pct > 85 ? "bg-danger" : pct > 65 ? "bg-warn" : "bg-ok")} style={{ width: `${Math.min(100, pct)}%` }} />
      </div>
    </div>
  );
}

/** Label for a usage window from its length in minutes (Codex reports it). */
function windowLabel(mins: number | undefined, fallback: string) {
  if (!mins) return fallback;
  if (mins === 10080) return "Weekly";
  if (mins % 1440 === 0) return `${mins / 1440}-day window`;
  if (mins % 60 === 0) return `${mins / 60}-hour window`;
  return `${mins}-minute window`;
}

const STALE_MS = 10 * 60 * 1000;

function PlanUsage({ limits }: { limits: any }) {
  const qc = useQueryClient();
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const claude = limits?.claude?.unifiedWindows;
  const codex = limits?.codex;
  const codexAt = codex?.updatedAt ? new Date(codex.updatedAt).getTime() : 0;

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const r = await api.post<{ rate_limits: any; error?: string }>("/api/usage/refresh");
      setError(r.error ?? null);
      qc.setQueryData(["system"], (old: any) => (old ? { ...old, rate_limits: r.rate_limits } : old));
    } catch (e: any) {
      setError(e.message);
    } finally {
      setRefreshing(false);
    }
  }, [qc]);

  // Codex only reports usage while it runs, so ask for it when ours is old.
  const asked = useRef(false);
  useEffect(() => {
    if (limits && !asked.current && Date.now() - codexAt > STALE_MS) {
      asked.current = true;
      refresh();
    }
  }, [limits, codexAt, refresh]);

  if (!claude && !codex && !refreshing && !error) return null;
  return (
    <Card className="p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Plan usage</div>
        <button className="p-1 text-muted hover:text-text" title="Refresh usage" aria-label="Refresh usage" onClick={refresh} disabled={refreshing}>
          <RefreshCw className={cx("size-3.5", refreshing && "animate-spin")} />
        </button>
      </div>
      {claude && (
        <div className="space-y-2">
          <div className="text-xs font-medium">Claude</div>
          {claude.five_hour && <UsageBar label="5-hour window" pct={(claude.five_hour.utilization ?? 0) * 100} resets={claude.five_hour.resetsAt} />}
          {claude.seven_day && <UsageBar label="Weekly" pct={(claude.seven_day.utilization ?? 0) * 100} resets={claude.seven_day.resetsAt} />}
        </div>
      )}
      <div className="space-y-2">
        <div className="text-xs font-medium flex justify-between">
          <span>Codex{codex?.planType ? ` · ${codex.planType}` : ""}</span>
          {codexAt > 0 && <span className="font-normal text-muted">updated {timeAgo(codex.updatedAt)}</span>}
        </div>
        {codex?.primary && <UsageBar label={windowLabel(codex.primary.windowDurationMins, "Short window")} pct={codex.primary.usedPercent ?? 0} resets={codex.primary.resetsAt} />}
        {codex?.secondary && <UsageBar label={windowLabel(codex.secondary.windowDurationMins, "Weekly")} pct={codex.secondary.usedPercent ?? 0} resets={codex.secondary.resetsAt} />}
        {codex?.credits?.hasCredits && !codex.credits.unlimited && codex.credits.balance != null && (
          <div className="text-xs text-muted">Credits left: {codex.credits.balance}</div>
        )}
        {!codex?.primary && !codex?.secondary && (
          <div className="text-xs text-muted" title={error ?? undefined}>
            {refreshing ? "Loading Codex usage…" : error ? "Not available — is Codex installed and logged in with ChatGPT?" : "No usage reported yet"}
          </div>
        )}
      </div>
    </Card>
  );
}

export function ChatRow({ c, showProject }: { c: Chat & { project_name?: string }; showProject?: string }) {
  return (
    <Link to={`/chats/${c.id}`} className="flex items-center gap-3 px-4 py-3 hover:bg-panel-2 transition">
      <StatusDot status={c.status} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{c.title}</div>
        <div className="text-xs text-muted truncate">
          {showProject && <>{showProject} · </>}
          {toolName(c.tool)}
          {c.model && ` · ${c.model}`} · {c.user} · {timeAgo(c.updated_at)}
        </div>
      </div>
      {c.status === "waiting" && <Badge tone="warn">needs answer</Badge>}
      {c.status === "error" && <Badge tone="danger">error</Badge>}
      {c.schedule_id && <Badge>scheduled</Badge>}
    </Link>
  );
}

export default function Dashboard() {
  const { user } = useAuth();
  const sys = useQuery({ queryKey: ["system"], queryFn: () => api.get("/api/system"), refetchInterval: 10_000 });
  const chats = useQuery({ queryKey: ["recent-chats"], queryFn: () => api.get<Chat[]>("/api/chats") });
  const queue = useQuery({ queryKey: ["queue"], queryFn: () => api.get("/api/queue"), refetchInterval: 15_000 });
  const pending = useQuery({ queryKey: ["pending"], queryFn: () => api.get<any[]>("/api/requests/pending") });
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<any[]>("/api/projects") });
  const s = sys.data;
  const pname = (id: number) => projects.data?.find((p) => p.id === id)?.name ?? "";

  return (
    <>
      <PageHeader title={`Hi ${user.username}`} subtitle="What is running on your server right now" />
      {s && (
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3 mb-5">
          <Meter label="CPU" value={s.system.cpu_percent} max={100} unit="%" icon={<Cpu className="size-4" />} />
          <Meter
            label="RAM used"
            value={s.system.mem.total_mb - s.system.mem.available_mb}
            max={s.system.mem.total_mb}
            unit="MB"
            icon={<MemoryStick className="size-4" />}
          />
          <Card className="p-4">
            <div className="flex items-center gap-2 text-sm text-muted">
              <Activity className="size-4" /> Running
            </div>
            <div className="mt-2 text-2xl font-semibold tabular-nums">{s.counts.running_claude + s.counts.running_codex}</div>
            <div className="text-xs text-muted mt-1">
              Claude {s.counts.running_claude}/{s.limits.max_claude} · Codex {s.counts.running_codex}/{s.limits.max_codex}
            </div>
          </Card>
          <Card className="p-4">
            <div className="flex items-center gap-2 text-sm text-muted">
              <Hourglass className="size-4" /> Queued
            </div>
            <div className="mt-2 text-2xl font-semibold tabular-nums">{s.counts.queued}</div>
            <div className="text-xs text-muted mt-1">
              {s.counts.waiting} waiting for an answer · swap {s.system.mem.swap_total_mb - s.system.mem.swap_free_mb} MB
            </div>
          </Card>
        </div>
      )}
      <div className="grid lg:grid-cols-3 gap-4">
        <div className="lg:col-span-2 space-y-4">
          {(pending.data?.length ?? 0) > 0 && (
            <Card>
              <div className="px-4 py-3 border-b border-border text-sm font-medium text-warn">Waiting for your answer</div>
              <div className="divide-y divide-border">
                {pending.data!.map((r) => (
                  <Link key={r.id} to={`/chats/${r.chat_id}`} className="block px-4 py-3 hover:bg-panel-2">
                    <div className="text-sm font-medium">
                      {r.kind === "approval" ? `Approve ${r.data.tool}` : r.data.questions?.[0]?.question ?? "Question"}
                    </div>
                    <div className="text-xs text-muted">
                      {r.project} · {r.chat_title} · {timeAgo(r.created_at)}
                    </div>
                  </Link>
                ))}
              </div>
            </Card>
          )}
          <DeploysCard />
          {(queue.data?.runs?.length ?? 0) > 0 && (
            <Card>
              <div className="px-4 py-3 border-b border-border text-sm font-medium">Running & queued</div>
              <div className="divide-y divide-border">
                {queue.data.runs.map((r: any) => (
                  <Link key={r.id} to={`/chats/${r.chat_id}`} className="flex items-center gap-3 px-4 py-3 hover:bg-panel-2">
                    {r.status === "running" ? <Play className="size-4 text-accent" /> : <Hourglass className="size-4 text-muted" />}
                    <div className="min-w-0 flex-1">
                      <div className="text-sm truncate">{r.prompt}</div>
                      <div className="text-xs text-muted truncate">
                        {r.project} · {r.chat_title} · {toolName(r.tool)} {r.source === "schedule" && "· scheduled"}
                      </div>
                    </div>
                  </Link>
                ))}
              </div>
            </Card>
          )}
          <Card>
            <div className="px-4 py-3 border-b border-border text-sm font-medium">Recent chats</div>
            {chats.data?.length ? (
              <div className="divide-y divide-border">
                {chats.data.slice(0, 20).map((c) => (
                  <ChatRow key={c.id} c={c} showProject={pname(c.project_id)} />
                ))}
              </div>
            ) : (
              <Empty icon={<MessageSquare className="size-8" />} title="No chats yet">
                Open a project and start a chat with Claude Code or Codex.
              </Empty>
            )}
          </Card>
        </div>
        <div className="space-y-4">
          <PlanUsage limits={s?.rate_limits} />
          {s?.cli && (
            <Card className="p-4 space-y-2 text-sm">
              <div className="font-medium flex items-center gap-2">
                <Bot className="size-4" /> CLIs
              </div>
              <div className="flex justify-between">
                <span className="text-muted">Claude Code</span>
                <span>{s.cli.claude_version ?? "—"}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted">Codex</span>
                <span>{s.cli.codex_version ?? "—"}</span>
              </div>
              <Link to="/admin/cli" className="text-accent text-xs">
                Manage logins & updates →
              </Link>
            </Card>
          )}
          {s && (
            <Card className="p-4 text-xs text-muted space-y-1">
              <div>Panel memory: {s.system.app_rss_mb} MB</div>
              <div>Load: {s.system.load?.join(" ")}</div>
              <div>Timezone: {s.timezone}</div>
            </Card>
          )}
        </div>
      </div>
    </>
  );
}
