import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { Activity, Bot, Cpu, Hourglass, MemoryStick, MessageSquare, Play, RefreshCw } from "lucide-react";
import { ChatDeployIcon } from "../components/DeployStatus";
import { ErrorState, Loadable, SkeletonCards, SkeletonList } from "../components/Loadable";
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

/** Plan names come lower-case from the CLIs ("max", "plus"). */
export function planName(p?: string | null) {
  if (!p) return "";
  return p.charAt(0).toUpperCase() + p.slice(1);
}

type Login = { method: "subscription" | "api_key" | "none"; email: string | null; plan: string | null; logged_in: boolean };

/** Usage older than this is fetched again when the dashboard shows it. */
const STALE_MS = 5 * 60 * 1000;
/** This page asks the server for fresh usage at most this often on its own. */
const AUTO_GAP_MS = 60 * 1000;

const CLAUDE_WINDOWS: [string, string][] = [
  ["five_hour", "5-hour window"],
  ["seven_day", "Weekly (all models)"],
  ["seven_day_sonnet", "Weekly (Sonnet)"],
  ["seven_day_opus", "Weekly (Opus)"],
];

function UsageHeader({ name, plan, email, at }: { name: string; plan?: string | null; email?: string | null; at?: string }) {
  return (
    <div className="text-xs flex justify-between gap-2">
      <span className="font-medium truncate">
        {name}
        {plan ? ` · ${planName(plan)}` : ""}
        {email && <span className="font-normal text-muted"> · {email}</span>}
      </span>
      {at && <span className="text-muted shrink-0">updated {timeAgo(at)}</span>}
    </div>
  );
}

function PlanUsage({ limits, errors, logins }: { limits: any; errors?: Record<string, string>; logins?: { claude: Login; codex: Login } }) {
  const qc = useQueryClient();
  const [refreshing, setRefreshing] = useState(false);
  const claude = limits?.claude;
  const codex = limits?.codex;
  const claudeAt = claude?.updatedAt ? new Date(claude.updatedAt).getTime() : 0;
  const codexAt = codex?.updatedAt ? new Date(codex.updatedAt).getTime() : 0;

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const r = await api.post<{ rate_limits: any; usage_errors?: Record<string, string>; logins?: any }>("/api/usage/refresh");
      qc.setQueryData(["system"], (old: any) =>
        old
          ? {
              ...old,
              rate_limits: r.rate_limits,
              usage_errors: r.usage_errors ?? {},
              cli: old.cli && r.logins ? { ...old.cli, logins: r.logins } : old.cli,
            }
          : old,
      );
    } catch (e: any) {
      qc.setQueryData(["system"], (old: any) => (old ? { ...old, usage_errors: { ...(old.usage_errors ?? {}), request: e.message } } : old));
    } finally {
      setRefreshing(false);
    }
  }, [qc]);

  // Ask for fresh usage when what we have is old or missing (also after a login
  // change, when the server drops the previous account's usage).
  const lastAsk = useRef(0);
  useEffect(() => {
    if (!limits || refreshing) return;
    const stale = (at: number) => Date.now() - at > STALE_MS;
    const claudeIn = logins ? logins.claude.method === "subscription" : true;
    const codexIn = logins ? logins.codex.method === "subscription" : true;
    const want = (claudeIn && stale(claudeAt)) || (codexIn && stale(codexAt));
    if (want && Date.now() - lastAsk.current > AUTO_GAP_MS) {
      lastAsk.current = Date.now();
      refresh();
    }
  }, [limits, claudeAt, codexAt, logins, refreshing, refresh]);

  const showClaude = !!claude || (logins ? logins.claude.method === "subscription" : true);
  const showCodex = !!codex || (logins ? logins.codex.method === "subscription" : true);
  if (!showClaude && !showCodex) return null;
  const empty = (tool: "claude" | "codex") => {
    const err = errors?.[tool] ?? errors?.request;
    if (refreshing) return `Loading ${tool === "claude" ? "Claude" : "Codex"} usage…`;
    if (err) return `Not available: ${err}`;
    return "No usage reported yet";
  };
  const claudeWindows = CLAUDE_WINDOWS.filter(([k]) => claude?.unifiedWindows?.[k]);
  return (
    <Card className="p-4 space-y-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">Plan usage</div>
        <button className="p-1 text-muted hover:text-text" title="Refresh usage" aria-label="Refresh usage" onClick={refresh} disabled={refreshing}>
          <RefreshCw className={cx("size-3.5", refreshing && "animate-spin")} />
        </button>
      </div>
      {showClaude && (
        <div className="space-y-2">
          <UsageHeader name="Claude" plan={logins?.claude.plan} email={logins?.claude.email} at={claude?.updatedAt} />
          {claudeWindows.map(([k, label]) => (
            <UsageBar key={k} label={label} pct={(claude.unifiedWindows[k].utilization ?? 0) * 100} resets={claude.unifiedWindows[k].resetsAt} />
          ))}
          {!claudeWindows.length && <div className="text-xs text-muted">{empty("claude")}</div>}
        </div>
      )}
      {showCodex && (
        <div className="space-y-2">
          <UsageHeader name="Codex" plan={codex?.planType ?? logins?.codex.plan} email={logins?.codex.email} at={codex?.updatedAt} />
          {codex?.primary && <UsageBar label={windowLabel(codex.primary.windowDurationMins, "Short window")} pct={codex.primary.usedPercent ?? 0} resets={codex.primary.resetsAt} />}
          {codex?.secondary && <UsageBar label={windowLabel(codex.secondary.windowDurationMins, "Weekly")} pct={codex.secondary.usedPercent ?? 0} resets={codex.secondary.resetsAt} />}
          {codex?.credits?.hasCredits && !codex.credits.unlimited && codex.credits.balance != null && (
            <div className="text-xs text-muted">Credits left: {codex.credits.balance}</div>
          )}
          {!codex?.primary && !codex?.secondary && <div className="text-xs text-muted">{empty("codex")}</div>}
        </div>
      )}
    </Card>
  );
}

function CliAccount({ login }: { login?: Login }) {
  if (!login) return null;
  if (login.method === "none")
    return (
      <Link to="/admin/cli" className="text-xs text-warn">
        Not logged in
      </Link>
    );
  if (login.method === "api_key") return <div className="text-xs text-muted">API key</div>;
  return (
    <div className="text-xs text-muted truncate" title={login.email ?? undefined}>
      {login.email ?? "Logged in"}
      {login.plan && ` · ${planName(login.plan)}`}
    </div>
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
      {c.read_only && <Badge>read-only</Badge>}
      {c.schedule_id && <Badge>scheduled</Badge>}
      {c.deploy && <ChatDeployIcon d={c.deploy} />}
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
      <div className="mb-5">
      <Loadable q={sys} skeleton={<SkeletonCards />} compact title="Server status">
        {(s: any) => (
        <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
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
      </Loadable>
      </div>
      <div className="grid lg:grid-cols-3 gap-4">
        <div className="lg:col-span-2 space-y-4">
          {pending.isError && !pending.data && <ErrorState compact title="Pending answers" error={pending.error} onRetry={pending.refetch} retrying={pending.isFetching} />}
          {queue.isError && !queue.data && <ErrorState compact title="Queue" error={queue.error} onRetry={queue.refetch} retrying={queue.isFetching} />}
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
            <Loadable q={chats} skeleton={<SkeletonList rows={5} className="border-0 rounded-none" />}>
              {(list) =>
                list.length ? (
                  <div className="divide-y divide-border">
                    {list.slice(0, 20).map((c) => (
                      <ChatRow key={c.id} c={c} showProject={pname(c.project_id)} />
                    ))}
                  </div>
                ) : (
                  <Empty icon={<MessageSquare className="size-8" />} title="No chats yet">
                    Open a project and start a chat with Claude Code or Codex.
                  </Empty>
                )
              }
            </Loadable>
          </Card>
        </div>
        <div className="space-y-4">
          <PlanUsage limits={s?.rate_limits} errors={s?.usage_errors} logins={s?.cli?.logins} />
          {s?.cli && (
            <Card className="p-4 space-y-2 text-sm">
              <div className="font-medium flex items-center gap-2">
                <Bot className="size-4" /> CLIs
              </div>
              <div>
                <div className="flex justify-between">
                  <span className="text-muted">Claude Code</span>
                  <span>{s.cli.claude_version ?? "—"}</span>
                </div>
                <CliAccount login={s.cli.logins?.claude} />
              </div>
              <div>
                <div className="flex justify-between">
                  <span className="text-muted">Codex</span>
                  <span>{s.cli.codex_version ?? "—"}</span>
                </div>
                <CliAccount login={s.cli.logins?.codex} />
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
