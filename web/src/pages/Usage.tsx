import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, BarChart3, ChevronDown, ChevronRight, Download, Info } from "lucide-react";
import React, { useEffect, useMemo, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { ErrorState, Skeleton } from "../components/Loadable";
import { Badge, Card, cx, DateRangePicker, Empty, PageHeader, Select, Spinner, Tabs } from "../components/ui";
import { api, qs } from "../lib/api";
import { useAuth } from "../lib/auth";
import { toolName } from "../lib/format";
import type { Project } from "../lib/types";

// ---------- data ----------

type Agg = {
  runs: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  reasoning: number;
  web_search: number;
  total_tokens: number;
  cost: number;
  estimated_cost: number;
};
type UserRow = Agg & { key: string; name: string; role: string; kind: "account" | "external" | "system"; projects: (Agg & { id: number; name: string })[] };
type Report = {
  timezone: string;
  range: string;
  from: string;
  to: string;
  today: string;
  scope: "all" | "self";
  totals: Agg;
  days: (Agg & { date: string })[];
  projects: (Agg & { id: number; name: string })[];
  users: UserRow[];
  models: (Agg & { tool: string; model: string })[];
  tools: (Agg & { tool: string })[];
  sources: (Agg & { source: string })[];
  chats: (Agg & { id: number; title: string; exists: boolean; project_id: number; project: string })[];
  schedules: (Agg & { id: number; name: string; project_id: number; project: string })[];
  user_options: { key: string; name: string; role: string }[];
  unpriced_models: string[];
  legacy_rows: number;
};

type Range = "today" | "week" | "month" | "custom";
type Filters = { range: Range; from: string; to: string; project: number | 0; tool: string; user: string };

// ---------- formatting ----------

export function money(n: number) {
  if (!n) return "$0.00";
  if (n >= 1000) return `$${n.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
  if (n >= 1) return `$${n.toFixed(2)}`;
  if (n >= 0.01) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(4)}`;
}
const full = (n: number) => n.toLocaleString();
function short(n: number) {
  if (n < 1000) return String(n);
  if (n < 1_000_000) return `${(n / 1000).toFixed(n < 10_000 ? 1 : 0)}k`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 2 : 1)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}
const dayLabel = (d: string, opts: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" }) => new Date(`${d}T12:00:00`).toLocaleDateString(undefined, opts);
const sourceName = (s: string) => ({ panel: "Panel", telegram: "Telegram", schedule: "Schedules", preview: "Live preview" })[s] ?? (s ? s[0].toUpperCase() + s.slice(1) : "Other");

function Cost({ a, className }: { a: Agg; className?: string }) {
  const est = a.estimated_cost > 0;
  return (
    <span className={cx("tabular-nums", className)} title={est ? `Includes ${money(a.estimated_cost)} estimated from the price table` : undefined}>
      {est && a.estimated_cost >= a.cost - 1e-9 ? "≈ " : ""}
      {money(a.cost)}
      {est && a.estimated_cost < a.cost - 1e-9 && <span className="text-muted">*</span>}
    </span>
  );
}

// ---------- filters in the URL ----------

function useFilters(): [Filters, (p: Partial<Filters>) => void] {
  const [sp, setSp] = useSearchParams();
  const f: Filters = {
    range: (["today", "week", "month", "custom"].includes(sp.get("range") ?? "") ? sp.get("range") : "today") as Range,
    from: sp.get("from") ?? "",
    to: sp.get("to") ?? "",
    project: Number(sp.get("project")) || 0,
    tool: sp.get("tool") ?? "",
    user: sp.get("user") ?? "",
  };
  const set = (p: Partial<Filters>) => {
    const n = { ...f, ...p };
    const next = new URLSearchParams(sp);
    for (const [k, v] of Object.entries(n)) {
      if (v === "" || v === 0 || (k === "range" && v === "today")) next.delete(k);
      else next.set(k, String(v));
    }
    if (n.range !== "custom") (next.delete("from"), next.delete("to"));
    setSp(next, { replace: true });
  };
  return [f, set];
}

function useReport(f: Filters, user?: string) {
  const params = {
    range: f.range,
    from: f.range === "custom" ? f.from : undefined,
    to: f.range === "custom" ? f.to : undefined,
    project_id: f.project || undefined,
    tool: f.tool || undefined,
    user: user || undefined,
  };
  return useQuery({
    queryKey: ["usage", params],
    queryFn: () => api.get<Report>(`/api/usage/report${qs(params)}`),
    enabled: f.range !== "custom" || (!!f.from && !!f.to),
    placeholderData: keepPreviousData,
    refetchInterval: 60_000,
  });
}

// ---------- page ----------

export default function Usage() {
  const { user } = useAuth();
  const admin = user.role === "admin";
  const [f, setF] = useFilters();
  const [sp, setSp] = useSearchParams();
  const tab = (sp.get("tab") === "users" && admin ? "users" : "system") as "system" | "users";
  const setTab = (t: "system" | "users") => {
    const n = new URLSearchParams(sp);
    if (t === "system") n.delete("tab");
    else n.set("tab", t);
    n.delete("user");
    setSp(n, { replace: true });
  };

  return (
    <>
      <PageHeader
        title="Usage"
        subtitle={admin ? "Tokens and cost across the whole panel, by project, model and person" : "Your tokens and cost, by project and model"}
      />
      {admin && (
        <div className="mb-4">
          <Tabs
            value={tab}
            onChange={setTab}
            tabs={[
              { id: "system", label: "System" },
              { id: "users", label: "Per user" },
            ]}
          />
        </div>
      )}
      {tab === "system" ? <SystemView f={f} setF={setF} admin={admin} /> : <UsersView f={f} setF={setF} />}
    </>
  );
}

function FilterBar({ f, setF, report, children }: { f: Filters; setF: (p: Partial<Filters>) => void; report?: Report; children?: React.ReactNode }) {
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  // Projects that were deleted but still have usage can be filtered too.
  const projectOptions = useMemo(() => {
    const list = (projects.data ?? []).map((p) => ({ value: p.id, label: p.name, hint: p.path }));
    for (const p of report?.projects ?? []) if (!list.some((o) => o.value === p.id)) list.push({ value: p.id, label: p.name, hint: "" });
    return [{ value: 0, label: "All projects", hint: "" }, ...list];
  }, [projects.data, report?.projects]);
  const ranges: { id: Range; label: string }[] = [
    { id: "today", label: "Today" },
    { id: "week", label: "This week" },
    { id: "month", label: "This month" },
    { id: "custom", label: "Custom" },
  ];
  return (
    <div className="flex flex-wrap items-center gap-2 mb-4">
      <div role="radiogroup" aria-label="Period" className="inline-flex rounded-lg border border-border bg-panel p-0.5">
        {ranges.map((r) => (
          <button
            key={r.id}
            type="button"
            role="radio"
            aria-checked={f.range === r.id}
            onClick={() => {
              if (r.id === "custom" && !f.from) {
                const end = report?.today ?? new Date().toISOString().slice(0, 10);
                const d = new Date(`${end}T12:00:00`);
                d.setDate(d.getDate() - 29);
                setF({ range: "custom", from: d.toISOString().slice(0, 10), to: end });
              } else setF({ range: r.id });
            }}
            className={cx("h-8 px-3 rounded-md text-[13px] transition whitespace-nowrap", f.range === r.id ? "bg-accent text-accent-fg font-medium" : "text-muted hover:text-text")}
          >
            {r.label}
          </button>
        ))}
      </div>
      {f.range === "custom" && (
        <DateRangePicker className="w-64 max-w-full h-9" from={f.from} to={f.to} onChange={(from, to) => setF({ from, to })} max={report?.today} aria-label="Date range" />
      )}
      <Select className="w-48 max-w-full h-9" value={f.project} onChange={(project) => setF({ project })} options={projectOptions} searchable aria-label="Project" />
      <Select
        className="w-36 h-9"
        value={f.tool}
        onChange={(tool) => setF({ tool })}
        aria-label="Tool"
        options={[
          { value: "", label: "All tools" },
          { value: "claude", label: "Claude Code" },
          { value: "codex", label: "Codex" },
        ]}
      />
      {children}
    </div>
  );
}

function PeriodLine({ r, fetching }: { r?: Report; fetching: boolean }) {
  if (!r) return null;
  const same = r.from === r.to;
  return (
    <div className="flex items-center gap-2 text-xs text-muted mb-3 -mt-1">
      <span>
        {same ? dayLabel(r.from, { weekday: "long", month: "long", day: "numeric", year: "numeric" }) : `${dayLabel(r.from, { month: "short", day: "numeric", year: "numeric" })} – ${dayLabel(r.to, { month: "short", day: "numeric", year: "numeric" })}`}
        {" · "}
        {r.timezone}
        {r.range === "week" && " · weeks start on Saturday"}
      </span>
      {fetching && <Spinner inline className="size-3.5" />}
    </div>
  );
}

function SystemView({ f, setF, admin }: { f: Filters; setF: (p: Partial<Filters>) => void; admin: boolean }) {
  const q = useReport(f);
  const r = q.data;
  return (
    <>
      <FilterBar f={f} setF={setF} report={r}>
        <div className="flex-1" />
        {r && <ExportButton r={r} name="usage" />}
      </FilterBar>
      <PeriodLine r={r} fetching={q.isFetching} />
      <ReportBody q={q} showUsers={admin} />
    </>
  );
}

function UsersView({ f, setF }: { f: Filters; setF: (p: Partial<Filters>) => void }) {
  const all = useReport(f);
  const one = useReport(f, f.user || undefined);
  const r = all.data;
  const userOptions = [{ value: "", label: "All users", hint: "Compare everyone" }, ...(r?.user_options ?? []).map((u) => ({ value: u.key, label: u.name, hint: u.role === "admin" ? "Admin" : u.role === "user" ? "User" : u.key.startsWith("ext:") ? "Telegram" : "" }))];
  return (
    <>
      <FilterBar f={f} setF={setF} report={r}>
        <Select className="w-48 max-w-full h-9" value={f.user} onChange={(user) => setF({ user })} options={userOptions} searchable aria-label="User" />
        <div className="flex-1" />
        {f.user ? one.data && <ExportButton r={one.data} name={`usage-${userOptions.find((o) => o.value === f.user)?.label ?? "user"}`} /> : r && <ExportButton r={r} name="usage-users" only="users" />}
      </FilterBar>
      <PeriodLine r={f.user ? one.data : r} fetching={(f.user ? one : all).isFetching} />
      {f.user ? (
        <ReportBody q={one} showUsers={false} />
      ) : !r ? (
        all.isError ? <ErrorState error={all.error} onRetry={all.refetch} retrying={all.isFetching} /> : <LoadingBody />
      ) : (
        <>
          <Tiles a={r.totals} />
          <UsersTable rows={r.users} total={r.totals} onPick={(user) => setF({ user })} accounts={r.user_options} />
          <Notes r={r} />
        </>
      )}
    </>
  );
}

function LoadingBody() {
  return (
    <div role="status" aria-label="Loading" className="space-y-4">
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        {Array.from({ length: 8 }, (_, i) => (
          <Skeleton key={i} className="h-[74px] rounded-xl" />
        ))}
      </div>
      <Skeleton className="h-64 rounded-xl" />
      <Skeleton className="h-48 rounded-xl" />
    </div>
  );
}

function ReportBody({ q, showUsers }: { q: ReturnType<typeof useReport>; showUsers: boolean }) {
  const r = q.data;
  if (!r) return q.isError ? <ErrorState error={q.error} onRetry={q.refetch} retrying={q.isFetching} /> : <LoadingBody />;
  return (
    <div className="space-y-4">
      <Tiles a={r.totals} />
      <DailyChart days={r.days} today={r.today} />
      <Breakdown r={r} showUsers={showUsers} />
      <Notes r={r} />
    </div>
  );
}

function Notes({ r }: { r: Report }) {
  const items: React.ReactNode[] = [];
  if (r.totals.estimated_cost > 0)
    items.push(
      <>
        Codex reports no price, so its cost (marked ≈ or *) is estimated from the price table{" "}
        {r.scope === "all" ? (
          <Link className="text-accent underline" to="/admin/settings">
            in Settings
          </Link>
        ) : (
          "in Settings"
        )}
        .
      </>,
    );
  if (r.unpriced_models.length) items.push(<>No price matches {r.unpriced_models.join(", ")}; those tokens count as $0.</>);
  if (r.legacy_rows > 0) items.push(<>Runs from before this version only recorded input and output totals, so their cached tokens are counted as input.</>);
  if (!items.length) return null;
  return (
    <div className="mt-4 space-y-1">
      {items.map((t, i) => (
        <div key={i} className="flex gap-1.5 text-xs text-muted">
          <Info className="size-3.5 shrink-0 mt-px" />
          <span>{t}</span>
        </div>
      ))}
    </div>
  );
}

// ---------- tiles ----------

function Tiles({ a }: { a: Agg }) {
  const tiles: { label: string; value: React.ReactNode; title?: string; hint?: string }[] = [
    { label: "Cost", value: <Cost a={a} />, hint: a.estimated_cost > 0 ? `${money(a.estimated_cost)} estimated` : "Reported by the CLIs" },
    { label: "Total tokens", value: short(a.total_tokens), title: full(a.total_tokens), hint: `${full(a.runs)} ${a.runs === 1 ? "run" : "runs"}` },
    { label: "Input", value: short(a.input), title: full(a.input), hint: "Not from cache" },
    { label: "Output", value: short(a.output), title: full(a.output), hint: a.reasoning ? `${short(a.reasoning)} reasoning` : "Includes reasoning" },
    { label: "Cache read", value: short(a.cache_read), title: full(a.cache_read), hint: cacheHit(a) },
    { label: "Cache write", value: short(a.cache_write), title: full(a.cache_write), hint: "Prompt cache created" },
    { label: "Avg. cost per run", value: money(a.runs ? a.cost / a.runs : 0), hint: a.runs ? `${short(Math.round(a.total_tokens / a.runs))} tokens per run` : "—" },
    { label: "Web searches", value: full(a.web_search), hint: "Claude Code" },
  ];
  return (
    <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
      {tiles.map((t) => (
        <Card key={t.label} className="px-4 py-3 min-w-0">
          <div className="text-xs text-muted">{t.label}</div>
          <div className="text-xl font-semibold tracking-tight tabular-nums mt-0.5 truncate" title={t.title}>
            {t.value}
          </div>
          {t.hint && <div className="text-[11px] text-muted mt-0.5 truncate">{t.hint}</div>}
        </Card>
      ))}
    </div>
  );
}

function cacheHit(a: Agg) {
  const denom = a.input + a.cache_read + a.cache_write;
  if (!denom) return "Share of input from cache";
  return `${Math.round((a.cache_read / denom) * 100)}% of input from cache`;
}

// ---------- daily chart ----------

/** Axis top and step using 1 / 2 / 2.5 / 5 × 10ⁿ steps, about four gridlines. */
function niceScale(v: number) {
  if (v <= 0) return { max: 1, step: 0.25 };
  const raw = v / 4;
  const p = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = ([1, 2, 2.5, 5, 10].find((m) => m * p >= raw) ?? 10) * p;
  return { max: Math.ceil(v / step - 1e-9) * step, step };
}

function DailyChart({ days, today }: { days: Report["days"]; today: string }) {
  const [metric, setMetric] = useState<"cost" | "tokens">("cost");
  const box = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(600);
  const [hover, setHover] = useState<number | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  if (days.length <= 1) return null; // A single day is fully described by the tiles.
  const val = (d: Agg) => (metric === "cost" ? d.cost : d.total_tokens);
  const { max, step } = niceScale(Math.max(...days.map(val)));
  const H = 220;
  const padL = 48;
  const padB = 24;
  const padT = 8;
  const plotW = Math.max(50, w - padL - 4);
  const plotH = H - padB - padT;
  const slot = plotW / days.length;
  const gap = days.length > 60 ? 1 : 2;
  const barW = Math.max(1, Math.min(36, slot - gap));
  const ticks = Array.from({ length: Math.round(max / step) + 1 }, (_, i) => i * step);
  const dp = step >= 1 ? (Number.isInteger(step) ? 0 : 1) : Math.min(4, Math.ceil(-Math.log10(step) - 1e-9) + (String(step).includes("5") ? 1 : 0));
  const fmt = (v: number) => (metric === "cost" ? (v >= 1000 ? `$${short(Math.round(v))}` : `$${v.toFixed(dp)}`) : short(Math.round(v)));
  const every = Math.ceil(days.length / Math.max(2, Math.floor(plotW / 56)));
  const h = hover !== null ? days[hover] : null;
  const empty = days.every((d) => val(d) === 0);

  return (
    <Card className="p-4">
      <div className="flex items-center justify-between gap-3 mb-3">
        <div className="text-sm font-medium">{metric === "cost" ? "Cost per day" : "Tokens per day"}</div>
        <div role="radiogroup" aria-label="Chart metric" className="inline-flex rounded-lg border border-border p-0.5">
          {(["cost", "tokens"] as const).map((m) => (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={metric === m}
              onClick={() => setMetric(m)}
              className={cx("h-7 px-2.5 rounded-md text-xs", metric === m ? "bg-panel-2 text-text font-medium" : "text-muted hover:text-text")}
            >
              {m === "cost" ? "Cost" : "Tokens"}
            </button>
          ))}
        </div>
      </div>
      <div ref={box} className="relative" onMouseLeave={() => setHover(null)}>
        <svg width={w} height={H} role="img" aria-label={`${metric === "cost" ? "Cost" : "Tokens"} per day`} className="block">
          {ticks.map((t, i) => {
            const y = padT + plotH - (t / max) * plotH;
            return (
              <g key={i}>
                <line x1={padL} x2={w} y1={y} y2={y} stroke="var(--border)" strokeDasharray={i === 0 ? undefined : "2 4"} />
                <text x={padL - 6} y={y + 3.5} textAnchor="end" fontSize={10} fill="var(--muted)">
                  {fmt(t)}
                </text>
              </g>
            );
          })}
          {days.map((d, i) => {
            const v = val(d);
            const bh = v > 0 ? Math.max(2, (v / max) * plotH) : 0;
            const x = padL + i * slot + (slot - barW) / 2;
            const y = padT + plotH - bh;
            const future = d.date > today;
            const r = Math.min(4, barW / 2, bh);
            return (
              <g key={d.date}>
                {bh > 0 && (
                  <path
                    d={`M${x},${y + bh} V${y + r} Q${x},${y} ${x + r},${y} H${x + barW - r} Q${x + barW},${y} ${x + barW},${y + r} V${y + bh} Z`}
                    fill="var(--accent)"
                    opacity={hover === null || hover === i ? 1 : 0.45}
                  />
                )}
                {future && <line x1={x} x2={x + barW} y1={padT + plotH} y2={padT + plotH} stroke="var(--border)" strokeWidth={2} />}
                <rect x={padL + i * slot} y={padT} width={slot} height={plotH} fill="transparent" onMouseEnter={() => setHover(i)} onClick={() => setHover(i)} />
                {i % every === 0 && (
                  <text x={padL + i * slot + slot / 2} y={H - 6} textAnchor="middle" fontSize={10} fill={d.date === today ? "var(--text)" : "var(--muted)"}>
                    {days.length > 31 && new Date(`${d.date}T12:00:00`).getDate() === 1 ? dayLabel(d.date, { month: "short" }) : dayLabel(d.date, days.length > 31 ? { day: "numeric" } : undefined)}
                  </text>
                )}
              </g>
            );
          })}
        </svg>
        {empty && <div className="absolute inset-0 flex items-center justify-center text-sm text-muted pointer-events-none">No usage in this period</div>}
        {h && hover !== null && (
          <div
            className="absolute z-10 pointer-events-none rounded-lg border border-border bg-panel shadow-lg px-3 py-2 text-xs w-52"
            style={{ left: Math.min(Math.max(0, padL + hover * slot + slot / 2 - 104), w - 208), top: 0 }}
          >
            <div className="font-medium text-text mb-1">{dayLabel(h.date, { weekday: "short", month: "short", day: "numeric", year: "numeric" })}</div>
            <TipRow k="Cost" v={<Cost a={h} />} />
            <TipRow k="Runs" v={full(h.runs)} />
            <TipRow k="Input" v={full(h.input)} />
            <TipRow k="Output" v={full(h.output)} />
            <TipRow k="Cache read" v={full(h.cache_read)} />
            <TipRow k="Cache write" v={full(h.cache_write)} />
          </div>
        )}
      </div>
    </Card>
  );
}

function TipRow({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-3 py-px">
      <span className="text-muted">{k}</span>
      <span className="tabular-nums text-text">{v}</span>
    </div>
  );
}

// ---------- tables ----------

type Col = { key: keyof Agg; label: string; short?: string };
const COLS: Col[] = [
  { key: "runs", label: "Runs" },
  { key: "input", label: "Input" },
  { key: "output", label: "Output" },
  { key: "cache_read", label: "Cache read" },
  { key: "cache_write", label: "Cache write" },
  { key: "total_tokens", label: "Total tokens" },
  { key: "cost", label: "Cost" },
];

type Row = Agg & { _key: string; _label: React.ReactNode; _sub?: React.ReactNode; _text: string; _children?: Row[] };

function useSort(initial: keyof Agg = "cost") {
  const [sort, setSort] = useState<{ key: keyof Agg; desc: boolean }>({ key: initial, desc: true });
  const toggle = (key: keyof Agg) => setSort((s) => (s.key === key ? { key, desc: !s.desc } : { key, desc: true }));
  const apply = <T extends Agg>(rows: T[]) => [...rows].sort((a, b) => (sort.desc ? 1 : -1) * ((b[sort.key] as number) - (a[sort.key] as number)));
  return { sort, toggle, apply };
}

function DataTable({ rows, total, empty, nameLabel }: { rows: Row[]; total: Agg; empty: string; nameLabel: string }) {
  const { sort, toggle, apply } = useSort();
  const [open, setOpen] = useState<Set<string>>(new Set());
  const sorted = apply(rows);
  if (!rows.length) return <Empty icon={<BarChart3 className="size-8" />} title={empty} />;
  const share = (r: Agg) => (sort.key === "cost" ? (total.cost ? r.cost / total.cost : 0) : total[sort.key] ? (r[sort.key] as number) / (total[sort.key] as number) : 0);
  const cell = (r: Agg, c: Col) => (c.key === "cost" ? <Cost a={r} /> : <span title={full(r[c.key] as number)}>{c.key === "runs" ? full(r.runs) : short(r[c.key] as number)}</span>);
  const renderRow = (r: Row, depth = 0): React.ReactNode => {
    const isOpen = open.has(r._key);
    const kids = r._children ? apply(r._children) : [];
    return (
      <React.Fragment key={r._key}>
        <tr className={cx("border-t border-border", depth > 0 && "bg-panel-2/50")}>
          <td className="py-2 pl-4 pr-3 min-w-[12rem] max-w-[22rem]">
            <div className="flex items-start gap-1.5" style={{ paddingLeft: depth * 18 }}>
              {kids.length > 0 ? (
                <button
                  type="button"
                  className="mt-0.5 p-0.5 -ml-1 rounded text-muted hover:text-text"
                  aria-label={isOpen ? "Collapse" : "Expand"}
                  aria-expanded={isOpen}
                  onClick={() => setOpen((s) => (s.has(r._key) ? (s.delete(r._key), new Set(s)) : new Set(s).add(r._key)))}
                >
                  {isOpen ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
                </button>
              ) : depth === 0 && rows.some((x) => x._children?.length) ? (
                <span className="w-[18px] shrink-0" />
              ) : null}
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm" title={r._text}>
                  {r._label}
                </div>
                {r._sub && <div className="text-xs text-muted truncate">{r._sub}</div>}
                {depth === 0 && (
                  <div className="mt-1 h-1 rounded-full bg-panel-2 overflow-hidden" aria-hidden>
                    <div className="h-full rounded-full bg-accent" style={{ width: `${Math.max(share(r) * 100, share(r) > 0 ? 1 : 0)}%` }} />
                  </div>
                )}
              </div>
            </div>
          </td>
          {COLS.map((c) => (
            <td key={c.key} className={cx("py-2 px-3 text-right tabular-nums whitespace-nowrap text-sm", c.key === "cost" ? "font-medium" : "text-muted", c.key === sort.key && "text-text")}>
              {cell(r, c)}
            </td>
          ))}
          <td className="py-2 pr-4 pl-2 text-right text-xs text-muted tabular-nums whitespace-nowrap">{depth === 0 ? `${Math.round(share(r) * 100)}%` : ""}</td>
        </tr>
        {isOpen && kids.map((k) => renderRow(k, depth + 1))}
      </React.Fragment>
    );
  };
  return (
    <div className="overflow-x-auto scroll-thin">
      <table className="w-full text-sm">
        <thead>
          <tr className="text-xs text-muted">
            <th className="text-left font-medium py-2 pl-4 pr-3">{nameLabel}</th>
            {COLS.map((c) => (
              <th key={c.key} className="text-right font-medium py-2 px-3 whitespace-nowrap">
                <button type="button" className={cx("inline-flex items-center gap-0.5 hover:text-text", sort.key === c.key && "text-text")} onClick={() => toggle(c.key)}>
                  {c.label}
                  {sort.key === c.key && (sort.desc ? <ArrowDown className="size-3" /> : <ArrowUp className="size-3" />)}
                </button>
              </th>
            ))}
            <th className="text-right font-medium py-2 pr-4 pl-2">Share</th>
          </tr>
        </thead>
        <tbody>{sorted.map((r) => renderRow(r))}</tbody>
        <tfoot>
          <tr className="border-t-2 border-border font-medium">
            <td className="py-2 pl-4 pr-3 text-sm">Total</td>
            {COLS.map((c) => (
              <td key={c.key} className="py-2 px-3 text-right tabular-nums whitespace-nowrap text-sm">
                {cell(total, c)}
              </td>
            ))}
            <td className="py-2 pr-4 pl-2 text-right text-xs text-muted">100%</td>
          </tr>
        </tfoot>
      </table>
    </div>
  );
}

type BTab = "projects" | "users" | "models" | "tools" | "chats" | "schedules" | "sources";

function breakdownRows(r: Report, t: BTab): Row[] {
  switch (t) {
    case "projects":
      return r.projects.map((p) => ({ ...p, _key: `p${p.id}`, _text: p.name, _label: <Link className="hover:text-accent" to={`/projects/${p.id}`}>{p.name}</Link> }));
    case "users":
      return r.users.map((u) => userRow(u));
    case "models":
      return r.models.map((m) => ({ ...m, _key: `${m.tool}/${m.model}`, _text: m.model, _label: <span className="font-mono text-[13px]">{m.model}</span>, _sub: toolName(m.tool) }));
    case "tools":
      return r.tools.map((x) => ({ ...x, _key: x.tool, _text: toolName(x.tool), _label: toolName(x.tool) }));
    case "chats":
      return r.chats.map((c) => ({
        ...c,
        _key: `c${c.id}`,
        _text: c.title,
        _label: c.exists ? <Link className="hover:text-accent" to={`/chats/${c.id}`}>{c.title || "Untitled chat"}</Link> : <span className="text-muted">{c.title}</span>,
        _sub: c.project,
      }));
    case "schedules":
      return r.schedules.map((s) => ({ ...s, _key: `s${s.id}`, _text: s.name, _label: <Link className="hover:text-accent" to="/schedules">{s.name}</Link>, _sub: s.project }));
    case "sources":
      return r.sources.map((s) => ({ ...s, _key: s.source, _text: sourceName(s.source), _label: sourceName(s.source) }));
  }
}

function userRow(u: UserRow): Row {
  return {
    ...u,
    _key: `u${u.key}`,
    _text: u.name,
    _label: (
      <span className="inline-flex items-center gap-1.5">
        {u.name}
        {u.role === "admin" && <Badge tone="accent">admin</Badge>}
        {u.kind === "external" && <Badge>Telegram</Badge>}
      </span>
    ),
    _children: u.projects.map((p) => ({ ...p, _key: `u${u.key}p${p.id}`, _text: p.name, _label: p.name })),
  };
}

function Breakdown({ r, showUsers }: { r: Report; showUsers: boolean }) {
  const [t, setT] = useState<BTab>("projects");
  const tabs: { id: BTab; label: string }[] = [
    { id: "projects", label: `Projects (${r.projects.length})` },
    ...(showUsers ? [{ id: "users" as BTab, label: `Users (${r.users.length})` }] : []),
    { id: "models", label: `Models (${r.models.length})` },
    { id: "tools", label: "Tools" },
    { id: "chats", label: `Chats (${r.chats.length})` },
    { id: "schedules", label: `Schedules (${r.schedules.length})` },
    { id: "sources", label: "Sources" },
  ];
  const nameLabel = { projects: "Project", users: "User", models: "Model", tools: "Tool", chats: "Chat", schedules: "Schedule", sources: "Started from" }[t];
  return (
    <Card>
      <div className="px-2 pt-1">
        <Tabs tabs={tabs} value={t} onChange={setT} />
      </div>
      <DataTable rows={breakdownRows(r, t)} total={r.totals} nameLabel={nameLabel} empty={t === "schedules" ? "No scheduled runs in this period" : "No usage in this period"} />
    </Card>
  );
}

function UsersTable({ rows, total, onPick, accounts }: { rows: UserRow[]; total: Agg; onPick: (key: string) => void; accounts: Report["user_options"] }) {
  // Accounts with no usage still appear, so it's clear they used nothing.
  const zero: Agg = { runs: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, reasoning: 0, web_search: 0, total_tokens: 0, cost: 0, estimated_cost: 0 };
  const all: UserRow[] = [...rows, ...accounts.filter((a) => !rows.some((r) => r.key === a.key)).map((a) => ({ ...zero, key: a.key, name: a.name, role: a.role, kind: "account" as const, projects: [] }))];
  const data = all.map((u) => {
    const row = userRow(u);
    row._label = (
      <button type="button" className="inline-flex items-center gap-1.5 hover:text-accent text-left" onClick={() => onPick(u.key)} title="Show this user's details">
        {row._label}
      </button>
    );
    return row;
  });
  return (
    <Card className="mt-4">
      <div className="px-4 pt-3 pb-1 text-sm font-medium">Usage per user</div>
      <div className="px-4 pb-2 text-xs text-muted">Expand a user to see their projects, or click a name for their full report.</div>
      <DataTable rows={data} total={total} nameLabel="User" empty="No users" />
    </Card>
  );
}

// ---------- CSV ----------

function csvCell(v: unknown) {
  const s = String(v ?? "");
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

function toCsv(r: Report, only?: "users") {
  const head = ["section", "name", "detail", "runs", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "reasoning_tokens", "web_searches", "total_tokens", "cost_usd", "estimated_cost_usd"];
  const line = (section: string, name: string, detail: string, a: Agg) =>
    [section, name, detail, a.runs, a.input, a.output, a.cache_read, a.cache_write, a.reasoning, a.web_search, a.total_tokens, a.cost.toFixed(6), a.estimated_cost.toFixed(6)].map(csvCell).join(",");
  const out = [head.join(","), line("total", `${r.from}..${r.to}`, r.timezone, r.totals)];
  if (only !== "users") {
    r.days.forEach((d) => out.push(line("day", d.date, "", d)));
    r.projects.forEach((p) => out.push(line("project", p.name, String(p.id), p)));
  }
  r.users.forEach((u) => {
    out.push(line("user", u.name, u.role || u.kind, u));
    u.projects.forEach((p) => out.push(line("user_project", u.name, p.name, p)));
  });
  if (only !== "users") {
    r.models.forEach((m) => out.push(line("model", m.model, m.tool, m)));
    r.tools.forEach((t) => out.push(line("tool", t.tool, "", t)));
    r.chats.forEach((c) => out.push(line("chat", c.title, c.project, c)));
    r.schedules.forEach((s) => out.push(line("schedule", s.name, s.project, s)));
    r.sources.forEach((s) => out.push(line("source", s.source, "", s)));
  }
  return out.join("\n");
}

function ExportButton({ r, name, only }: { r: Report; name: string; only?: "users" }) {
  const download = () => {
    const blob = new Blob(["﻿" + toCsv(r, only)], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${name.replace(/[^\w.-]+/g, "_")}_${r.from}_${r.to}.csv`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return (
    <button type="button" onClick={download} className="inline-flex items-center gap-1.5 h-9 px-3 rounded-lg border border-border text-[13px] font-medium hover:bg-panel-2">
      <Download className="size-4" /> Export CSV
    </button>
  );
}
