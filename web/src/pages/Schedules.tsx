import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryStatus, SkeletonList } from "../components/Loadable";
import { ArrowDown, ArrowUp, CalendarClock, Pencil, Play, Plus, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router";
import { AgentPicker } from "../components/AgentPicker";
import { Badge, Button, Card, cx, DateTimePicker, Empty, Field, IconButton, Input, Modal, PageHeader, Select, Textarea, TimePicker, Toggle, useAction, useConfirm } from "../components/ui";
import { ymd } from "../components/controls";
import { api } from "../lib/api";
import { modeLabel, timeAgo, toolName } from "../lib/format";
import type { Chat, Project, Schedule } from "../lib/types";

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

type Draft = Omit<Schedule, "id" | "created_at" | "last_run_at" | "next_run_at" | "user_id">;

function emptyDraft(projectId?: number): Draft {
  return {
    project_id: projectId ?? 0,
    name: "",
    enabled: true,
    kind: "daily",
    run_at: null,
    time_of_day: "09:00",
    weekdays: "6,0,1,2,3",
    cron: "0 9 * * *",
    tool: "claude",
    model: "",
    effort: "",
    permission_mode: "bypass",
    mode: "new",
    chat_id: null,
    prompts: [""],
    use_worktree: false,
    auto_deploy: false,
  };
}

const CRON_PRESETS: { label: string; expr: string }[] = [
  { label: "Every 15 minutes", expr: "*/15 * * * *" },
  { label: "Every hour", expr: "0 * * * *" },
  { label: "Every 3 hours", expr: "0 */3 * * *" },
  { label: "Every day at 03:00", expr: "0 3 * * *" },
  { label: "Weekdays (Sat–Wed) at 09:00", expr: "0 9 * * 6,0,1,2,3" },
  { label: "Mon–Fri at 09:00", expr: "0 9 * * 1-5" },
  { label: "1st of every month at 08:00", expr: "0 8 1 * *" },
];

/** Cron editor: presets, one field per part, and a live preview of the next runs. */
function CronBuilder({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const parts = value.trim().split(/\s+/);
  const names = ["Minute", "Hour", "Day of month", "Month", "Day of week"];
  const hints = ["0-59, */15", "0-23, */2", "1-31", "1-12", "0-6 (Sun=0), 1-5"];
  const setPart = (i: number, v: string) => {
    const p = [...Array(5)].map((_, j) => parts[j] ?? "*");
    p[i] = v.trim() || "*";
    onChange(p.join(" "));
  };
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap gap-1.5">
        {CRON_PRESETS.map((p) => (
          <button
            type="button"
            key={p.expr}
            onClick={() => onChange(p.expr)}
            className={cx("rounded-full border px-2.5 py-1 text-xs", value === p.expr ? "border-accent bg-accent-soft text-accent" : "border-border text-muted hover:text-text")}
          >
            {p.label}
          </button>
        ))}
      </div>
      <div className="grid grid-cols-5 gap-2">
        {names.map((n, i) => (
          <label key={n} className="space-y-1">
            <span className="block text-[11px] text-muted truncate">{n}</span>
            <Input className="h-9 font-mono text-center px-1" value={parts[i] ?? "*"} onChange={(e) => setPart(i, e.target.value)} />
            <span className="block text-[10px] text-muted truncate">{hints[i]}</span>
          </label>
        ))}
      </div>
      <Field label="Expression">
        <Input className="font-mono" value={value} onChange={(e) => onChange(e.target.value)} />
      </Field>
    </div>
  );
}

function Preview({ d }: { d: Draft }) {
  const [res, setRes] = useState<{ ok: boolean; next?: string[]; error?: string } | null>(null);
  useEffect(() => {
    const t = setTimeout(() => {
      api
        .post("/api/schedules/preview", { kind: d.kind, run_at: d.run_at, time_of_day: d.time_of_day, weekdays: d.weekdays, cron: d.cron })
        .then(setRes)
        .catch(() => setRes(null));
    }, 300);
    return () => clearTimeout(t);
  }, [d.kind, d.run_at, d.time_of_day, d.weekdays, d.cron]);
  if (!res) return null;
  if (!res.ok) return <div className="text-xs text-danger">{res.error}</div>;
  return (
    <div className="rounded-lg bg-panel-2 px-3 py-2 text-xs">
      <div className="text-muted mb-1">Next runs (Asia/Tehran unless changed in Settings)</div>
      {res.next?.length ? res.next.map((n) => <div key={n} className="font-mono">{n}</div>) : <div>Never (time is in the past)</div>}
    </div>
  );
}

function Editor({ initial, id, onClose }: { initial: Draft; id?: number; onClose: () => void }) {
  const [d, setD] = useState<Draft>(initial);
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  const chats = useQuery({
    queryKey: ["chats", d.project_id, false],
    queryFn: () => api.get<Chat[]>(`/api/projects/${d.project_id}/chats`),
    enabled: !!d.project_id && d.mode === "continue",
  });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const up = (p: Partial<Draft>) => setD({ ...d, ...p });
  const project = projects.data?.find((p) => p.id === d.project_id);
  useEffect(() => {
    if (!d.project_id && projects.data?.length) up({ project_id: projects.data[0].id });
  }, [projects.data]);

  const save = async () => {
    const body = { ...d, prompts: d.prompts.filter((p) => p.trim()) };
    const r = await run(() => (id ? api.put(`/api/schedules/${id}`, body) : api.post("/api/schedules", body)), "Schedule saved");
    if (r) {
      qc.invalidateQueries({ queryKey: ["schedules"] });
      onClose();
    }
  };
  const days = (d.weekdays ?? "").split(",").filter(Boolean).map(Number);

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={id ? "Edit schedule" : "New schedule"}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={save}>
            Save
          </Button>
        </>
      }
    >
      <div className="grid sm:grid-cols-2 gap-4">
        <Field label="Name">
          <Input value={d.name} onChange={(e) => up({ name: e.target.value })} placeholder="Nightly dependency check" />
        </Field>
        <Field label="Project">
          <Select
            value={d.project_id || null}
            onChange={(id) => up({ project_id: id, chat_id: null })}
            placeholder="Choose a project…"
            emptyText="No projects found"
            options={(projects.data ?? []).map((p) => ({ value: p.id, label: p.name, hint: p.path }))}
          />
        </Field>
      </div>

      <Field label="Agent">
        <AgentPicker v={d} set={(c) => up(c)} />
      </Field>

      <div className="space-y-3">
        <div className="text-sm font-medium">When</div>
        <div className="flex flex-wrap gap-1.5">
          {(["once", "daily", "weekly", "cron"] as const).map((k) => (
            <button
              key={k}
              type="button"
              onClick={() => up({ kind: k })}
              className={cx("rounded-lg border px-3 py-1.5 text-sm", d.kind === k ? "border-accent bg-accent-soft text-accent" : "border-border text-muted")}
            >
              {k === "once" ? "Once" : k === "daily" ? "Every day" : k === "weekly" ? "Some weekdays" : "Advanced (cron)"}
            </button>
          ))}
        </div>
        {d.kind === "once" && (
          <Field label="Date and time">
            <DateTimePicker value={d.run_at ?? ""} onChange={(run_at) => up({ run_at })} min={ymd(new Date())} />
          </Field>
        )}
        {(d.kind === "daily" || d.kind === "weekly") && (
          <Field label="Time">
            <TimePicker value={d.time_of_day ?? ""} onChange={(time_of_day) => up({ time_of_day })} className="w-40" />
          </Field>
        )}
        {d.kind === "weekly" && (
          <div className="flex flex-wrap gap-1.5">
            {[6, 0, 1, 2, 3, 4, 5].map((i) => {
              const on = days.includes(i);
              return (
                <button
                  key={i}
                  type="button"
                  onClick={() => up({ weekdays: (on ? days.filter((x) => x !== i) : [...days, i]).join(",") })}
                  className={cx("w-12 rounded-lg border py-1.5 text-sm", on ? "border-accent bg-accent text-accent-fg" : "border-border text-muted")}
                >
                  {DAYS[i]}
                </button>
              );
            })}
          </div>
        )}
        {d.kind === "cron" && <CronBuilder value={d.cron ?? ""} onChange={(cron) => up({ cron })} />}
        <Preview d={d} />
      </div>

      <div className="space-y-2">
        <div className="text-sm font-medium">Prompts</div>
        <div className="text-xs text-muted">More than one prompt runs as a chain: each starts after the previous one finished successfully, in the same chat.</div>
        {d.prompts.map((p, i) => (
          <div key={i} className="flex gap-2">
            <div className="text-xs text-muted pt-2.5 w-5 text-right">{i + 1}.</div>
            <Textarea
              value={p}
              onChange={(e) => up({ prompts: d.prompts.map((x, j) => (j === i ? e.target.value : x)) })}
              className="flex-1"
              placeholder={i === 0 ? "e.g. Update dependencies, run the tests and fix anything that breaks." : "Next step…"}
            />
            <div className="flex flex-col">
              <IconButton title="Move up" disabled={i === 0} onClick={() => up({ prompts: swap(d.prompts, i, i - 1) })}>
                <ArrowUp className="size-4" />
              </IconButton>
              <IconButton title="Move down" disabled={i === d.prompts.length - 1} onClick={() => up({ prompts: swap(d.prompts, i, i + 1) })}>
                <ArrowDown className="size-4" />
              </IconButton>
              <IconButton title="Remove" disabled={d.prompts.length === 1} onClick={() => up({ prompts: d.prompts.filter((_, j) => j !== i) })}>
                <X className="size-4" />
              </IconButton>
            </div>
          </div>
        ))}
        <Button size="sm" variant="outline" icon={<Plus className="size-4" />} onClick={() => up({ prompts: [...d.prompts, ""] })}>
          Add follow-up prompt
        </Button>
      </div>

      <div className="space-y-2">
        <div className="text-sm font-medium">Conversation</div>
        <Select
          value={d.mode}
          onChange={(mode) => up({ mode })}
          aria-label="Conversation"
          options={[
            { value: "new" as const, label: "Start a new chat every time" },
            { value: "continue" as const, label: "Continue the same chat every time" },
          ]}
        />
        {d.mode === "continue" && (
          <Field label="Chat" hint="Leave empty to create one on the first run and keep using it">
            <Select
              value={d.chat_id ?? 0}
              onChange={(id) => up({ chat_id: id || null })}
              options={[
                { value: 0, label: "Create on first run" },
                ...(chats.data ?? []).filter((c) => c.tool === d.tool).map((c) => ({ value: c.id, label: c.title, hint: timeAgo(c.updated_at) })),
              ]}
            />
          </Field>
        )}
      </div>

      <div className="flex flex-wrap gap-x-6">
        <Toggle checked={d.enabled} onChange={(enabled) => up({ enabled })} label="Enabled" />
        {project?.deployable && <Toggle checked={d.auto_deploy} onChange={(auto_deploy) => up({ auto_deploy })} label="Deploy when finished" />}
        {d.mode === "new" && <Toggle checked={d.use_worktree} onChange={(use_worktree) => up({ use_worktree })} label="Separate git worktree" />}
      </div>
    </Modal>
  );
}

function swap<T>(a: T[], i: number, j: number): T[] {
  const b = [...a];
  [b[i], b[j]] = [b[j], b[i]];
  return b;
}

function describe(s: Schedule) {
  switch (s.kind) {
    case "once":
      return `Once at ${s.run_at?.replace("T", " ")}`;
    case "daily":
      return `Every day at ${s.time_of_day}`;
    case "weekly":
      return `${(s.weekdays ?? "")
        .split(",")
        .filter(Boolean)
        .map((d) => DAYS[Number(d)])
        .join(", ")} at ${s.time_of_day}`;
    default:
      return `cron ${s.cron}`;
  }
}

export default function Schedules() {
  const ask = useConfirm();
  const q = useQuery({ queryKey: ["schedules"], queryFn: () => api.get<{ schedules: Schedule[]; timezone: string }>("/api/schedules") });
  const [edit, setEdit] = useState<{ d: Draft; id?: number } | null>(null);
  const { run } = useAction();
  const qc = useQueryClient();
  const nav = useNavigate();
  const reload = () => qc.invalidateQueries({ queryKey: ["schedules"] });
  return (
    <>
      <PageHeader
        title="Schedules"
        subtitle={`Prompts that run on their own · times in ${q.data?.timezone ?? "Asia/Tehran"}`}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEdit({ d: emptyDraft() })}>
            New schedule
          </Button>
        }
      />
      <QueryStatus q={q} skeleton={<SkeletonList rows={3} />} />
      {q.data?.schedules.length === 0 && (
        <Card>
          <Empty icon={<CalendarClock className="size-8" />} title="No schedules yet">
            Run prompts at fixed times — once, daily, on chosen weekdays or with a cron expression — on any project, with Claude Code or Codex.
          </Empty>
        </Card>
      )}
      <div className="space-y-3">
        {q.data?.schedules.map((s) => (
          <Card key={s.id} className={cx("p-4", !s.enabled && "opacity-60")}>
            <div className="flex flex-wrap items-start gap-3">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{s.name}</span>
                  {!s.enabled && <Badge>paused</Badge>}
                  {s.prompts.length > 1 && <Badge tone="accent">{s.prompts.length} chained prompts</Badge>}
                  {s.auto_deploy && <Badge tone="ok">auto-deploy</Badge>}
                </div>
                <div className="text-sm text-muted mt-0.5">
                  {describe(s)} · <Link to={`/projects/${s.project_id}`}>{s.project}</Link> · {toolName(s.tool)}
                  {s.model && ` · ${s.model}`} · {modeLabel(s.permission_mode)} · {s.mode === "continue" ? "same chat" : "new chat each time"}
                </div>
                <div className="text-xs text-muted mt-1">
                  {s.next_run_local ? `Next: ${s.next_run_local}` : "No upcoming run"}
                  {s.last_run_at && ` · last ran ${timeAgo(s.last_run_at)}`}
                </div>
                <div className="text-sm mt-2 line-clamp-2 text-text/80">{s.prompts[0]}</div>
              </div>
              <div className="flex items-center gap-1">
                <Toggle
                  checked={s.enabled}
                  onChange={async (enabled) => {
                    await run(() => api.put(`/api/schedules/${s.id}`, { ...s, enabled }));
                    reload();
                  }}
                />
                <IconButton
                  title="Run now"
                  onClick={async () => {
                    const r = await run(() => api.post<{ chat_id: number }>(`/api/schedules/${s.id}/run`), "Started");
                    if (r) nav(`/chats/${r.chat_id}`);
                  }}
                >
                  <Play className="size-4" />
                </IconButton>
                <IconButton title="Edit" onClick={() => setEdit({ d: { ...s }, id: s.id })}>
                  <Pencil className="size-4" />
                </IconButton>
                <IconButton
                  title="Delete"
                  onClick={async () => {
                    if (!(await ask({ title: "Delete schedule", message: `Delete schedule "${s.name}"?`, confirmLabel: "Delete", danger: true }))) return;
                    await run(() => api.del(`/api/schedules/${s.id}`), "Deleted");
                    reload();
                  }}
                >
                  <Trash2 className="size-4" />
                </IconButton>
              </div>
            </div>
          </Card>
        ))}
      </div>
      {edit && <Editor initial={edit.d} id={edit.id} onClose={() => setEdit(null)} />}
    </>
  );
}
