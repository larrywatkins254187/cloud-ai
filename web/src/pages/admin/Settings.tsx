import { useQuery, useQueryClient } from "@tanstack/react-query";
import { queryGate } from "../../components/Loadable";
import { ArrowDown, ArrowUp, Download, Plus, RefreshCw, RotateCcw, Save, Send, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge, Button, Card, Field, IconButton, Input, NumberInput, PageHeader, Select, Textarea, Toggle, useAction, useConfirm, useToast } from "../../components/ui";
import { api } from "../../lib/api";
import { MODES, timeAgo } from "../../lib/format";

type S = Record<string, string>;

/** Every IANA timezone the browser knows, with its current UTC offset. */
function timezoneOptions(current?: string) {
  let zones: string[] = [];
  try {
    zones = (Intl as unknown as { supportedValuesOf: (k: string) => string[] }).supportedValuesOf("timeZone");
  } catch {
    zones = [];
  }
  if (!zones.includes("UTC")) zones = ["UTC", ...zones];
  if (current && !zones.includes(current)) zones = [current, ...zones];
  const now = new Date();
  return zones.map((z) => {
    let off = "";
    try {
      off = new Intl.DateTimeFormat("en-US", { timeZone: z, timeZoneName: "shortOffset" }).formatToParts(now).find((p) => p.type === "timeZoneName")?.value ?? "";
    } catch {
      off = "";
    }
    return { value: z, label: z.replace(/_/g, " "), hint: off.replace("GMT", "UTC") || undefined };
  });
}

type Price = {
  model: string;
  input: number | string;
  cache_read: number | string;
  cache_write: number | string;
  output: number | string;
  /** "auto" rows are refreshed daily; "manual" rows (added or edited here) are kept. */
  source?: "auto" | "manual";
};

type PriceStatus = { source: string; source_url: string; updated_at: string | null; checked_at: string | null; error: string | null; builtin: Price[] };

/** Turn half-typed numbers into numbers before saving. */
function cleanPrices(json: string | undefined) {
  if (!json?.trim()) return json ?? "";
  try {
    const rows = JSON.parse(json) as Price[];
    const n = (x: number | string) => (Number.isFinite(Number(x)) ? Number(x) : 0);
    return JSON.stringify(
      rows.map((r) => ({ model: r.model.trim(), input: n(r.input), cache_read: n(r.cache_read), cache_write: n(r.cache_write), output: n(r.output), ...(r.source ? { source: r.source } : {}) })),
    );
  } catch {
    return json;
  }
}

function parseRows(value: string, fallback: Price[]): Price[] {
  try {
    const rows = value.trim() ? JSON.parse(value) : fallback;
    return Array.isArray(rows) ? rows : fallback;
  } catch {
    return fallback;
  }
}

const isManual = (r: Price) => r.source !== "auto";
const family = (m: string) => (m.toLowerCase().startsWith("claude") ? "claude" : m.includes("*") && !/gpt|codex|^o\d/i.test(m) ? "other" : "openai");

type PriceFilter = "all" | "claude" | "openai" | "manual";
const PAGE = 30;

/** Editable price list; stored as JSON in the `usage_prices` setting. */
function PriceTable({ value, onChange, builtin }: { value: string; onChange: (json: string) => void; builtin: Price[] }) {
  const rows = parseRows(value, builtin);
  const [filter, setFilter] = useState<PriceFilter>("all");
  const [search, setSearch] = useState("");
  const [showAll, setShowAll] = useState(false);
  const ask = useConfirm();
  // Keep partially typed numbers ("1.") while editing; they are stored as numbers.
  const num = (x: number | string) => (typeof x === "number" ? x : x === "" || x.endsWith(".") ? x : Number(x));
  const set = (next: Price[]) =>
    onChange(JSON.stringify(next.map((r) => ({ ...r, input: num(r.input), cache_read: num(r.cache_read), cache_write: num(r.cache_write), output: num(r.output) }))));
  // Editing a row makes it manual, so the daily update leaves it alone.
  const upd = (i: number, p: Partial<Price>) => set(rows.map((r, j) => (j === i ? { ...r, ...p, source: "manual" } : r)));
  const move = (i: number, d: number) => {
    const n = [...rows];
    [n[i], n[i + d]] = [n[i + d], n[i]];
    set(n);
  };
  const manualCount = rows.filter(isManual).length;
  const q = search.trim().toLowerCase();
  const shown = rows
    .map((r, i) => ({ r, i }))
    .filter(({ r }) => (filter === "all" ? true : filter === "manual" ? isManual(r) : family(r.model) === filter))
    .filter(({ r }) => !q || r.model.toLowerCase().includes(q));
  const visible = showAll || q ? shown : shown.slice(0, PAGE);
  const canMove = filter === "all" && !q;
  const cols: { k: "input" | "cache_read" | "cache_write" | "output"; label: string }[] = [
    { k: "input", label: "Input" },
    { k: "cache_read", label: "Cached input" },
    { k: "cache_write", label: "Cache write" },
    { k: "output", label: "Output" },
  ];
  const tabs: { id: PriceFilter; label: string }[] = [
    { id: "all", label: `All ${rows.length}` },
    { id: "claude", label: "Claude" },
    { id: "openai", label: "OpenAI / Codex" },
    { id: "manual", label: `Manual ${manualCount}` },
  ];
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex rounded-lg border border-border p-0.5 text-xs">
          {tabs.map((t) => (
            <button
              key={t.id}
              className={"px-2.5 py-1 rounded-md " + (filter === t.id ? "bg-panel-2 text-text font-medium" : "text-muted hover:text-text")}
              onClick={() => setFilter(t.id)}
            >
              {t.label}
            </button>
          ))}
        </div>
        <Input className="h-8 text-xs flex-1 min-w-[160px]" placeholder="Search models…" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search models" />
      </div>
      <div className="hidden md:grid grid-cols-[minmax(0,1.6fr)_repeat(4,minmax(0,1fr))_auto] gap-2 px-1 text-[11px] font-medium text-muted">
        <div>Model or pattern</div>
        {cols.map((c) => (
          <div key={c.k}>{c.label} $/1M</div>
        ))}
        <div className="w-[104px]" />
      </div>
      {visible.map(({ r, i }) => (
        <div key={i} className="grid grid-cols-2 md:grid-cols-[minmax(0,2.4fr)_repeat(4,minmax(0,1fr))_auto] gap-2 items-center rounded-lg border border-border md:border-0 p-2 md:p-0">
          <div className="col-span-2 md:col-span-1 flex items-center gap-1.5 min-w-0">
            <Input className="h-9 font-mono text-xs min-w-0 flex-1" value={r.model} onChange={(e) => upd(i, { model: e.target.value })} placeholder="gpt-5*" aria-label="Model or pattern" />
            <span title={isManual(r) ? "Added or edited by hand: daily updates keep it" : "Updated daily from the price list"}>
              <Badge tone={isManual(r) ? "warn" : "neutral"} className="w-14 justify-center">{isManual(r) ? "manual" : "auto"}</Badge>
            </span>
          </div>
          {cols.map((c) => (
            <label key={c.k} className="block">
              <span className="md:hidden block text-[11px] text-muted mb-0.5">{c.label} $/1M</span>
              <NumberInput className="h-9" decimals steppers={false} min={0} step={0.05} value={r[c.k]} onChange={(x) => upd(i, { [c.k]: x } as Partial<Price>)} aria-label={`${c.label} price`} />
            </label>
          ))}
          <div className="col-span-2 md:col-span-1 flex justify-end">
            <IconButton title="Move up" disabled={!canMove || i === 0} onClick={() => move(i, -1)}>
              <ArrowUp className="size-4" />
            </IconButton>
            <IconButton title="Move down" disabled={!canMove || i === rows.length - 1} onClick={() => move(i, 1)}>
              <ArrowDown className="size-4" />
            </IconButton>
            <IconButton title="Remove" onClick={() => set(rows.filter((_, j) => j !== i))}>
              <X className="size-4" />
            </IconButton>
          </div>
        </div>
      ))}
      {!shown.length && <div className="text-xs text-muted px-1">No prices match.</div>}
      {!showAll && !q && shown.length > PAGE && (
        <button className="text-xs text-accent" onClick={() => setShowAll(true)}>
          Show all {shown.length} rows
        </button>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          icon={<Plus className="size-4" />}
          onClick={() => {
            setFilter("all");
            setSearch("");
            set([{ model: "", input: 0, cache_read: 0, cache_write: 0, output: 0, source: "manual" }, ...rows]);
          }}
        >
          Add price
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={<RotateCcw className="size-4" />}
          disabled={!manualCount}
          onClick={async () => {
            if (!(await ask({ title: "Remove manual prices", message: `Remove the ${manualCount} manual row(s) and use only the automatic prices?`, confirmLabel: "Remove", danger: true }))) return;
            const auto = rows.filter((r) => !isManual(r));
            set(auto.length ? auto : builtin);
          }}
        >
          Remove manual prices
        </Button>
      </div>
    </div>
  );
}

/** The Usage prices card: update status, "Update now" and the table. */
function UsagePrices({ value, saved, onChange }: { value: string; saved: string; onChange: (json: string) => void }) {
  const status = useQuery({ queryKey: ["price-status"], queryFn: () => api.get<PriceStatus>("/api/usage/prices") });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const st = status.data;
  const updateNow = async () => {
    const r = await run(async () => {
      // Save edits first so manual rows are kept by the update.
      if (cleanPrices(value) !== cleanPrices(saved)) await api.put("/api/settings", { usage_prices: cleanPrices(value) });
      return api.post<PriceStatus & { usage_prices: string }>("/api/usage/prices/update");
    }, "Prices updated");
    if (r) {
      onChange(r.usage_prices);
      qc.setQueryData(["price-status"], r);
      qc.setQueryData(["settings"], (old: S | undefined) => (old ? { ...old, usage_prices: r.usage_prices } : old));
    } else {
      status.refetch();
    }
  };
  return (
    <Card className="p-5 space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium">Usage prices</div>
          <div className="text-xs text-muted mt-0.5">
            USD per 1M tokens, used on the Usage page for runs whose CLI reports no cost (Codex). Claude Code reports its own cost. A row with the model's exact name wins;
            otherwise the first matching pattern (<code>*</code> matches anything).
          </div>
        </div>
        <Button size="sm" variant="outline" icon={<RefreshCw className="size-4" />} loading={busy} onClick={updateNow}>
          Update now
        </Button>
      </div>
      <div className="rounded-lg bg-panel-2 px-3 py-2 text-xs space-y-1">
        <div>
          Updated automatically every day from the{" "}
          <a className="text-accent" href={st?.source_url} target="_blank" rel="noreferrer">
            {st?.source ?? "public price list"}
          </a>
          , covering all current Claude and OpenAI models. Rows you add or edit are marked <b>manual</b> and never overwritten.
        </div>
        <div className="text-muted">
          {st?.updated_at ? <>Last updated {timeAgo(st.updated_at)}</> : <>Not updated yet: using the prices shipped with this version</>}
          {st?.checked_at && st.checked_at !== st.updated_at && <> · last try {timeAgo(st.checked_at)}</>}
        </div>
        {st?.error && <div className="text-danger">Last update failed: {st.error}</div>}
      </div>
      <PriceTable value={value} onChange={onChange} builtin={st?.builtin ?? []} />
    </Card>
  );
}

type Target = "claude" | "codex" | "app";
type VersionInfo = { installed: string | null; latest: string | null; update_available: boolean; error?: string | null };
type CheckResult = Record<Target, VersionInfo> & { idle: boolean };
const TARGETS: { id: Target; name: string }[] = [
  { id: "app", name: "cloud-ai" },
  { id: "claude", name: "Claude Code" },
  { id: "codex", name: "Codex" },
];

/** Check installed vs latest versions now and offer to update each one. */
function UpdateCheck() {
  const ask = useConfirm();
  const toast = useToast();
  const [checking, setChecking] = useState(false);
  const [updating, setUpdating] = useState<Target | null>(null);
  const [res, setRes] = useState<CheckResult | null>(null);
  const check = async () => {
    setChecking(true);
    try {
      setRes(await api.post<CheckResult>("/api/cli/check"));
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setChecking(false);
    }
  };
  const update = async (t: Target, name: string, v: VersionInfo) => {
    const ok = await ask({
      title: `Update ${name}`,
      message:
        `Update ${name} from ${v.installed ?? "?"} to ${v.latest}?` +
        (t === "app" ? "\n\nThe panel restarts after the update and reconnects by itself." : "\n\nIdle agent processes are stopped first; this only runs when no prompt or deploy is running."),
      confirmLabel: "Update now",
    });
    if (!ok) return;
    setUpdating(t);
    try {
      const r = await api.post<{ report: string[] }>("/api/cli/update", { only: t });
      toast(r.report.join(" · "), r.report.some((x) => x.startsWith("ERROR")) ? "danger" : "ok");
      if (t !== "app") await check();
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setUpdating(null);
    }
  };
  return (
    <div className="rounded-lg border border-border">
      <div className="flex items-center justify-between gap-3 px-3 py-2 border-b border-border">
        <span className="text-sm font-medium">Versions</span>
        <Button size="sm" variant="outline" icon={<RefreshCw className="size-4" />} loading={checking} onClick={check}>
          Check for updates now
        </Button>
      </div>
      {!res ? (
        <div className="px-3 py-3 text-xs text-muted">Click “Check for updates now” to compare installed and latest versions.</div>
      ) : (
        <div className="divide-y divide-border">
          {TARGETS.map(({ id, name }) => {
            const v = res[id];
            return (
              <div key={id} className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm">
                <span className="w-28 font-medium">{name}</span>
                <span className="text-muted text-xs flex-1 min-w-40">
                  {v.installed ?? "not installed"} {v.latest ? `→ latest ${v.latest}` : v.error ? `· ${v.error}` : "· latest unknown"}
                </span>
                {v.update_available ? (
                  <Button size="sm" variant="primary" icon={<Download className="size-4" />} loading={updating === id} disabled={!!updating} onClick={() => update(id, name, v)}>
                    Update to {v.latest}
                  </Button>
                ) : v.latest && v.installed ? (
                  <Badge tone="ok">up to date</Badge>
                ) : null}
              </div>
            );
          })}
          {!res.idle && <div className="px-3 py-2 text-xs text-warn">Something is running now — updates wait until all prompts and deploys finish.</div>}
        </div>
      )}
    </div>
  );
}

export default function Settings() {
  const q = useQuery({ queryKey: ["settings"], queryFn: () => api.get<S>("/api/settings") });
  const [v, setV] = useState<S>({});
  const { busy, run } = useAction();
  const toast = useToast();
  const qc = useQueryClient();
  useEffect(() => {
    if (q.data) setV(q.data);
  }, [q.data]);
  const gate = queryGate(q);
  if (gate) return gate;
  const up = (k: string, val: string) => setV({ ...v, [k]: val });
  const bool = (k: string) => v[k] === "1" || v[k] === "true";
  const save = async () => {
    const r = await run(() => api.put<S>("/api/settings", { ...v, usage_prices: cleanPrices(v.usage_prices) }), "Settings saved");
    if (r) qc.setQueryData(["settings"], r);
  };
  return (
    <>
      <PageHeader
        title="Settings"
        actions={
          <Button variant="primary" icon={<Save className="size-4" />} loading={busy} onClick={save}>
            Save
          </Button>
        }
      />
      <div className="space-y-4 max-w-3xl">
        <Card className="p-5 space-y-4">
          <div className="font-medium">Resources</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Max parallel Claude Code sessions">
              <NumberInput min={1} max={50} value={v.max_claude ?? ""} onChange={(x) => up("max_claude", x)} />
            </Field>
            <Field label="Max parallel Codex sessions">
              <NumberInput min={1} max={50} value={v.max_codex ?? ""} onChange={(x) => up("max_codex", x)} />
            </Field>
            <Field label="Minimum free RAM to start a prompt (MB)" hint="Below this, prompts wait in the queue (swap is not counted)">
              <NumberInput min={0} step={50} value={v.ram_guard_mb ?? ""} onChange={(x) => up("ram_guard_mb", x)} suffix="MB" />
            </Field>
            <Field label="Stop idle agent processes after (minutes)" hint="The conversation resumes automatically on the next prompt">
              <NumberInput min={1} value={v.idle_kill_minutes ?? ""} onChange={(x) => up("idle_kill_minutes", x)} suffix="min" />
            </Field>
            <Field
              label="Wait for background tasks up to (minutes)"
              hint="A prompt is done only when its background sub-agents and shell commands finish, or after this long"
            >
              <NumberInput min={1} value={v.background_wait_minutes ?? ""} onChange={(x) => up("background_wait_minutes", x)} suffix="min" />
            </Field>
          </div>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="font-medium">Agents</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Default permission mode for new chats">
              <Select
                value={v.default_permission_mode}
                onChange={(m) => up("default_permission_mode", m)}
                options={MODES.map((m) => ({ value: m.id, label: m.label, hint: m.hint }))}
              />
            </Field>
            <Field label="Timezone for schedules">
              <Select value={v.timezone ?? ""} onChange={(tz) => up("timezone", tz)} searchable options={timezoneOptions(v.timezone)} placeholder="Asia/Tehran" />
            </Field>
            <Field label="claude binary">
              <Input value={v.claude_path ?? ""} onChange={(e) => up("claude_path", e.target.value)} />
            </Field>
            <Field label="codex binary">
              <Input value={v.codex_path ?? ""} onChange={(e) => up("codex_path", e.target.value)} />
            </Field>
          </div>
          <Field label="Extra instructions for every agent" hint="Appended to the system prompt of Claude Code and Codex">
            <Textarea value={v.extra_system_prompt ?? ""} onChange={(e) => up("extra_system_prompt", e.target.value)} />
          </Field>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="flex items-center justify-between">
            <div className="font-medium">Telegram</div>
            <Button
              size="sm"
              variant="outline"
              icon={<Send className="size-4" />}
              onClick={async () => {
                await run(() => api.put("/api/settings", v));
                const r = await run(() => api.post<any>("/api/telegram/test"));
                if (r) r.ok ? toast(`Connected as @${r.bot.username}`, "ok") : toast(r.error, "danger");
              }}
            >
              Save & test
            </Button>
          </div>
          <ol className="text-xs text-muted list-decimal pl-4 space-y-0.5">
            <li>Create a bot with @BotFather and paste its token below.</li>
            <li>Create a group, enable Topics in its settings, add the bot and make it admin (Manage topics).</li>
            <li>Send /id in the group and paste the chat id below (it starts with -100).</li>
          </ol>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Bot token">
              <Input value={v.telegram_token ?? ""} onChange={(e) => up("telegram_token", e.target.value)} placeholder="123456:ABC…" />
            </Field>
            <Field label="Group chat id">
              <Input value={v.telegram_chat_id ?? ""} onChange={(e) => up("telegram_chat_id", e.target.value)} placeholder="-1001234567890" />
            </Field>
          </div>
          <Field label="Telegram user ids allowed to answer and deploy" hint="Comma separated; empty = anyone in the group">
            <Input value={v.telegram_allowed_users ?? ""} onChange={(e) => up("telegram_allowed_users", e.target.value)} />
          </Field>
          <div className="flex flex-wrap gap-x-6">
            <Toggle checked={bool("telegram_send_prompts")} onChange={(b) => up("telegram_send_prompts", b ? "1" : "0")} label="Send every prompt" />
            <Toggle checked={bool("telegram_send_results")} onChange={(b) => up("telegram_send_results", b ? "1" : "0")} label="Include the final answer" />
          </div>
          <Field label="Public URL of this panel" hint="Used for 'Open in panel' links">
            <Input value={v.public_url ?? ""} onChange={(e) => up("public_url", e.target.value)} placeholder="https://ai.example.com" />
          </Field>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="font-medium">Updates</div>
          <UpdateCheck />
          <Toggle
            checked={bool("auto_update_enabled")}
            onChange={(b) => up("auto_update_enabled", b ? "1" : "0")}
            label="Update Claude Code and Codex automatically"
            hint="Only when no session is running; retried every 15 minutes until idle"
          />
          <Field label="Daily update time (panel timezone)">
            <Select
              className="w-40"
              value={String(Number(v.update_hour ?? 5))}
              onChange={(h) => up("update_hour", h)}
              aria-label="Daily update hour"
              options={Array.from({ length: 24 }, (_, h) => ({ value: String(h), label: `${String(h).padStart(2, "0")}:00` }))}
            />
          </Field>
          <Toggle checked={bool("app_auto_update")} onChange={(b) => up("app_auto_update", b ? "1" : "0")} label="Update cloud-ai itself from GitHub releases" />
          <Field label="Release repository">
            <Input value={v.app_update_repo ?? ""} onChange={(e) => up("app_update_repo", e.target.value)} />
          </Field>
        </Card>

        <UsagePrices value={v.usage_prices ?? ""} saved={q.data?.usage_prices ?? ""} onChange={(s) => up("usage_prices", s)} />

        <Card className="p-5 space-y-4">
          <div className="font-medium">Live browser</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Chromium binary" hint="Empty = find chromium / google-chrome automatically">
              <Input value={v.browser_path ?? ""} onChange={(e) => up("browser_path", e.target.value)} />
            </Field>
            <Field label="Close the browser after idle (minutes)">
              <NumberInput min={1} value={v.browser_idle_minutes ?? ""} onChange={(x) => up("browser_idle_minutes", x)} suffix="min" />
            </Field>
          </div>
        </Card>
      </div>
    </>
  );
}
