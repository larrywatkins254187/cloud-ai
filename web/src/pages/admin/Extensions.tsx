import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, FileText, Pencil, Plus, Puzzle, RefreshCw, Save, Search, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Badge, Button, Card, cx, Empty, Field, IconButton, Input, Modal, PageHeader, Select, Spinner, Tabs, Textarea, useAction } from "../../components/ui";
import { api, qs } from "../../lib/api";
import type { Project, Tool } from "../../lib/types";

type Tab = "skills" | "plugins" | "mcp" | "instructions";

const SKILL_TEMPLATE = `---
name: my-skill
description: When to use this skill, in one sentence (the agent reads this to decide).
---

# My skill

Step-by-step instructions for the agent.
`;

function ScopeBar({
  tool,
  setTool,
  project,
  setProject,
  projects,
  noTool,
}: {
  tool: Tool;
  setTool: (t: Tool) => void;
  project: number | "";
  setProject: (p: number | "") => void;
  projects: Project[];
  noTool?: boolean;
}) {
  return (
    <div className="flex flex-wrap gap-2 mb-4">
      {!noTool && (
        <div className="flex rounded-lg border border-border overflow-hidden">
          {(["claude", "codex"] as Tool[]).map((t) => (
            <button key={t} onClick={() => setTool(t)} className={cx("px-3 h-9 text-sm", tool === t ? "bg-accent text-accent-fg" : "text-muted hover:bg-panel-2")}>
              {t === "claude" ? "Claude Code" : "Codex"}
            </button>
          ))}
        </div>
      )}
      <Select className="w-auto h-9" value={project} onChange={(e) => setProject(e.target.value ? Number(e.target.value) : "")}>
        <option value="">Global (all projects)</option>
        {projects.map((p) => (
          <option key={p.id} value={p.id}>
            Project: {p.name}
          </option>
        ))}
      </Select>
    </div>
  );
}

function Skills({ project }: { project: number | "" }) {
  const q = useQuery({ queryKey: ["skills", project], queryFn: () => api.get<any[]>(`/api/ext/skills${qs({ project: project || undefined })}`) });
  const [edit, setEdit] = useState<{ path?: string; tool: Tool; scope: string; name: string; content: string } | null>(null);
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const open = async (s: any) => {
    const r = await run(() => api.get<any>(`/api/ext/skill${qs({ path: s.path })}`));
    if (r) setEdit({ path: s.path, tool: s.tool, scope: s.scope, name: s.dir.split("/").pop(), content: r.content });
  };
  const save = async () => {
    if (!edit) return;
    const r = await run(() => api.put("/api/ext/skill", { tool: edit.tool, scope: edit.scope, project: project || undefined, name: edit.name, content: edit.content }), "Skill saved");
    if (r) {
      setEdit(null);
      qc.invalidateQueries({ queryKey: ["skills"] });
    }
  };
  return (
    <>
      <div className="flex justify-end mb-3">
        <Button
          variant="primary"
          icon={<Plus className="size-4" />}
          onClick={() => setEdit({ tool: "claude", scope: project ? "project" : "user", name: "", content: SKILL_TEMPLATE })}
        >
          New skill
        </Button>
      </div>
      {q.isLoading && <Spinner />}
      {q.data?.length === 0 && (
        <Card>
          <Empty icon={<Puzzle className="size-8" />} title="No skills yet">
            Skills are folders with a SKILL.md that teach Claude or Codex how to do a specific job.
          </Empty>
        </Card>
      )}
      <Card className="divide-y divide-border">
        {q.data?.map((s) => (
          <div key={s.path} className="flex items-start gap-3 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{s.name}</span>
                <Badge tone={s.tool === "claude" ? "accent" : "ok"}>{s.tool === "claude" ? "Claude" : "Codex"}</Badge>
                <Badge>{s.scope === "user" ? "global" : "project"}</Badge>
              </div>
              <div className="text-sm text-muted line-clamp-2">{s.description}</div>
              <div className="text-[11px] text-muted font-mono truncate">{s.dir}</div>
            </div>
            <IconButton title="Edit" onClick={() => open(s)}>
              <Pencil className="size-4" />
            </IconButton>
            <IconButton
              title="Delete"
              onClick={async () => {
                if (!confirm(`Delete skill ${s.name}?`)) return;
                await run(() => api.del(`/api/ext/skill${qs({ path: s.path })}`), "Deleted");
                qc.invalidateQueries({ queryKey: ["skills"] });
              }}
            >
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        ))}
      </Card>
      <Modal
        open={!!edit}
        onClose={() => setEdit(null)}
        wide
        title={edit?.path ? `Edit ${edit.name}` : "New skill"}
        footer={
          <>
            <Button variant="ghost" onClick={() => setEdit(null)}>
              Cancel
            </Button>
            <Button variant="primary" icon={<Save className="size-4" />} loading={busy} disabled={!edit?.name.trim()} onClick={save}>
              Save
            </Button>
          </>
        }
      >
        {edit && (
          <>
            {!edit.path && (
              <div className="grid sm:grid-cols-3 gap-3">
                <Field label="Folder name">
                  <Input value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} placeholder="my-skill" />
                </Field>
                <Field label="For">
                  <Select value={edit.tool} onChange={(e) => setEdit({ ...edit, tool: e.target.value as Tool })}>
                    <option value="claude">Claude Code</option>
                    <option value="codex">Codex</option>
                  </Select>
                </Field>
                <Field label="Scope">
                  <Select value={edit.scope} onChange={(e) => setEdit({ ...edit, scope: e.target.value })}>
                    <option value="user">Global</option>
                    {project && <option value="project">This project</option>}
                  </Select>
                </Field>
              </div>
            )}
            <Textarea value={edit.content} onChange={(e) => setEdit({ ...edit, content: e.target.value })} className="font-mono text-xs min-h-[50dvh]" spellCheck={false} />
          </>
        )}
      </Modal>
    </>
  );
}

function Plugins({ tool, project }: { tool: Tool; project: number | "" }) {
  const q = useQuery({ queryKey: ["plugins", tool, project], queryFn: () => api.get<any>(`/api/ext/plugins${qs({ tool, project: project || undefined })}`) });
  const [filter, setFilter] = useState("");
  const [market, setMarket] = useState("");
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const reload = () => qc.invalidateQueries({ queryKey: ["plugins", tool] });
  const act = async (action: string, plugin: string) => {
    await run(() => api.post("/api/ext/plugins", { tool, action, plugin, scope: project ? "project" : "user", project: project || undefined }), `${plugin}: ${action} done`);
    reload();
  };
  const installed: any[] = q.data?.plugins?.installed ?? [];
  const available: any[] = q.data?.plugins?.available ?? [];
  const installedIds = new Set(installed.map((p) => p.pluginId ?? p.id ?? p.name));
  const f = filter.toLowerCase();
  const shown = useMemo(
    () => available.filter((p) => !installedIds.has(p.pluginId) && (!f || `${p.name} ${p.description ?? ""}`.toLowerCase().includes(f))).slice(0, 80),
    [available, f, installed.length],
  );
  const markets: any[] = Array.isArray(q.data?.marketplaces) ? q.data.marketplaces : [];
  return (
    <div className="space-y-4">
      <Card className="p-4 space-y-3">
        <div className="text-sm font-medium">Marketplaces</div>
        {markets.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {markets.map((m) => (
              <span key={m.name} className="inline-flex items-center gap-1 rounded-full border border-border pl-3 pr-1 py-0.5 text-sm">
                {m.name}
                <button className="p-1 text-muted hover:text-text" title="Update" onClick={async () => (await run(() => api.post("/api/ext/marketplaces", { tool, action: "update", value: m.name }), "Updated"), reload())}>
                  <RefreshCw className="size-3.5" />
                </button>
                <button className="p-1 text-muted hover:text-danger" title="Remove" onClick={async () => confirm(`Remove ${m.name}?`) && (await run(() => api.post("/api/ext/marketplaces", { tool, action: "remove", value: m.name }), "Removed"), reload())}>
                  <Trash2 className="size-3.5" />
                </button>
              </span>
            ))}
          </div>
        ) : (
          q.data?.marketplaces?.raw && <pre className="text-xs text-muted whitespace-pre-wrap">{q.data.marketplaces.raw}</pre>
        )}
        <div className="flex gap-2">
          <Input value={market} onChange={(e) => setMarket(e.target.value)} placeholder={tool === "claude" ? "owner/repo, git URL or path — e.g. anthropics/claude-plugins-official" : "git URL or path"} />
          <Button
            variant="outline"
            loading={busy}
            disabled={!market.trim()}
            onClick={async () => {
              await run(() => api.post("/api/ext/marketplaces", { tool, action: "add", value: market }), "Marketplace added");
              setMarket("");
              reload();
            }}
          >
            Add
          </Button>
        </div>
      </Card>
      {q.isLoading && <Spinner />}
      {installed.length > 0 && (
        <Card>
          <div className="px-4 py-2.5 border-b border-border text-sm font-medium">Installed</div>
          <div className="divide-y divide-border">
            {installed.map((p) => {
              const id = p.pluginId ?? p.id ?? p.name;
              return (
                <div key={id} className="flex flex-wrap items-center gap-2 px-4 py-2.5">
                  <div className="min-w-0 flex-1">
                    <div className="font-medium text-sm">{p.name ?? id}</div>
                    <div className="text-xs text-muted">
                      {id} {p.version && `· v${p.version}`} {p.scope && `· ${p.scope}`}
                    </div>
                  </div>
                  {p.enabled === false ? <Badge>disabled</Badge> : <Badge tone="ok">enabled</Badge>}
                  {tool === "claude" && (
                    <Button size="sm" variant="ghost" onClick={() => act(p.enabled === false ? "enable" : "disable", id)}>
                      {p.enabled === false ? "Enable" : "Disable"}
                    </Button>
                  )}
                  {tool === "claude" && (
                    <Button size="sm" variant="ghost" onClick={() => act("update", id)}>
                      Update
                    </Button>
                  )}
                  <Button size="sm" variant="ghost" className="text-danger" onClick={() => confirm(`Uninstall ${id}?`) && act("uninstall", id)}>
                    Uninstall
                  </Button>
                </div>
              );
            })}
          </div>
        </Card>
      )}
      <Card>
        <div className="flex items-center gap-2 px-4 py-2 border-b border-border">
          <span className="text-sm font-medium">Available</span>
          <div className="flex-1" />
          <div className="relative">
            <Search className="size-4 absolute left-2.5 top-2.5 text-muted" />
            <Input className="h-9 pl-8 w-56" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Search" />
          </div>
        </div>
        {shown.length === 0 && <div className="p-6 text-sm text-muted text-center">{available.length ? "No match" : "Add a marketplace to see plugins"}</div>}
        <div className="divide-y divide-border">
          {shown.map((p) => (
            <div key={p.pluginId} className="flex items-start gap-3 px-4 py-2.5">
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium">
                  {p.name} <span className="text-xs text-muted font-normal">{p.marketplaceName}</span>
                </div>
                {p.description && <div className="text-xs text-muted line-clamp-2">{p.description}</div>}
              </div>
              <Button size="sm" variant="outline" icon={<Download className="size-4" />} loading={busy} onClick={() => act("install", p.pluginId)}>
                Install
              </Button>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}

function Mcp({ tool, project }: { tool: Tool; project: number | "" }) {
  const q = useQuery({ queryKey: ["mcp", tool, project], queryFn: () => api.get<any>(`/api/ext/mcp${qs({ tool, project: project || undefined })}`) });
  const [add, setAdd] = useState(false);
  const [v, setV] = useState({ name: "", kind: "stdio", command: "", args: "", url: "", env: "", scope: "user" });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  useEffect(() => setV((x) => ({ ...x, scope: project ? "project" : "user" })), [project]);
  const servers: any[] = Array.isArray(q.data?.servers) ? q.data.servers : [];
  const save = async () => {
    const env = Object.fromEntries(
      v.env
        .split("\n")
        .map((l) => l.trim())
        .filter((l) => l.includes("="))
        .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
    );
    const r = await run(
      () =>
        api.post("/api/ext/mcp", {
          tool,
          action: "add",
          scope: v.scope,
          project: project || undefined,
          name: v.name,
          command: v.kind === "stdio" ? v.command : "",
          args: v.kind === "stdio" ? v.args.split(/\s+/).filter(Boolean) : [],
          url: v.kind === "http" ? v.url : "",
          env,
        }),
      "MCP server added",
    );
    if (r) {
      setAdd(false);
      qc.invalidateQueries({ queryKey: ["mcp"] });
    }
  };
  return (
    <>
      <div className="flex justify-end mb-3">
        <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setAdd(true)}>
          Add MCP server
        </Button>
      </div>
      {q.isLoading && <Spinner />}
      {servers.length === 0 && !q.isLoading && (
        <Card>
          <Empty title="No MCP servers">MCP servers give the agents extra tools (databases, browsers, APIs…).</Empty>
        </Card>
      )}
      <Card className="divide-y divide-border">
        {servers.map((s, i) => (
          <div key={i} className="flex items-start gap-3 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="font-medium">{s.name}</span>
                {s.scope && <Badge>{s.scope}</Badge>}
              </div>
              <pre className="text-[11px] text-muted font-mono whitespace-pre-wrap break-all">{JSON.stringify(s.config ?? s.transport ?? s, null, 0)}</pre>
            </div>
            <IconButton
              title="Remove"
              onClick={async () => {
                if (!confirm(`Remove ${s.name}?`)) return;
                await run(() => api.post("/api/ext/mcp", { tool, action: "remove", name: s.name, scope: s.scope ?? "user", project: project || undefined }), "Removed");
                qc.invalidateQueries({ queryKey: ["mcp"] });
              }}
            >
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        ))}
      </Card>
      {q.data?.servers?.raw && <pre className="text-xs text-muted mt-3 whitespace-pre-wrap">{q.data.servers.raw}</pre>}
      <Modal
        open={add}
        onClose={() => setAdd(false)}
        title={`Add MCP server to ${tool === "claude" ? "Claude Code" : "Codex"}`}
        footer={
          <>
            <Button variant="ghost" onClick={() => setAdd(false)}>
              Cancel
            </Button>
            <Button variant="primary" loading={busy} onClick={save}>
              Add
            </Button>
          </>
        }
      >
        <div className="grid sm:grid-cols-2 gap-3">
          <Field label="Name">
            <Input value={v.name} onChange={(e) => setV({ ...v, name: e.target.value })} placeholder="postgres" />
          </Field>
          <Field label="Type">
            <Select value={v.kind} onChange={(e) => setV({ ...v, kind: e.target.value })}>
              <option value="stdio">Command (stdio)</option>
              <option value="http">URL (HTTP)</option>
            </Select>
          </Field>
        </div>
        {tool === "claude" && project && (
          <Field label="Scope">
            <Select value={v.scope} onChange={(e) => setV({ ...v, scope: e.target.value })}>
              <option value="user">Global</option>
              <option value="project">Project (.mcp.json, shared in git)</option>
              <option value="local">Project, only on this server</option>
            </Select>
          </Field>
        )}
        {v.kind === "stdio" ? (
          <>
            <Field label="Command">
              <Input value={v.command} onChange={(e) => setV({ ...v, command: e.target.value })} placeholder="npx" />
            </Field>
            <Field label="Arguments">
              <Input value={v.args} onChange={(e) => setV({ ...v, args: e.target.value })} placeholder="-y @modelcontextprotocol/server-postgres postgres://…" />
            </Field>
            <Field label="Environment (KEY=value per line)">
              <Textarea value={v.env} onChange={(e) => setV({ ...v, env: e.target.value })} className="font-mono text-xs" />
            </Field>
          </>
        ) : (
          <Field label="URL">
            <Input value={v.url} onChange={(e) => setV({ ...v, url: e.target.value })} placeholder="https://mcp.example.com/mcp" />
          </Field>
        )}
      </Modal>
    </>
  );
}

function Instructions({ tool, project }: { tool: Tool; project: number | "" }) {
  const scope = project ? "project" : "user";
  const q = useQuery({
    queryKey: ["instructions", tool, project],
    queryFn: () => api.get<any>(`/api/ext/instructions${qs({ tool, scope, project: project || undefined })}`),
  });
  const [text, setText] = useState("");
  const { busy, run } = useAction();
  useEffect(() => setText(q.data?.content ?? ""), [q.data]);
  return (
    <Card className="p-4 space-y-3">
      <div className="flex items-center gap-2 text-sm">
        <FileText className="size-4 text-muted" />
        <code className="text-xs truncate flex-1">{q.data?.path}</code>
        {q.data && !q.data.exists && <Badge>new file</Badge>}
        <Button
          size="sm"
          variant="primary"
          icon={<Save className="size-4" />}
          loading={busy}
          onClick={() => run(() => api.put("/api/ext/instructions", { tool, scope, project: project || undefined, content: text }), "Saved")}
        >
          Save
        </Button>
      </div>
      <p className="text-xs text-muted">
        {tool === "claude" ? "CLAUDE.md" : "AGENTS.md"} is read by {tool === "claude" ? "Claude Code" : "Codex"} at the start of every session
        {project ? " in this project" : " in every project"}.
      </p>
      <Textarea value={text} onChange={(e) => setText(e.target.value)} className="font-mono text-xs min-h-[55dvh]" spellCheck={false} />
    </Card>
  );
}

export default function Extensions() {
  const [tab, setTab] = useState<Tab>("skills");
  const [tool, setTool] = useState<Tool>("claude");
  const [project, setProject] = useState<number | "">("");
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  return (
    <>
      <PageHeader title="Skills & plugins" subtitle="Extend Claude Code and Codex — globally or per project" />
      <div className="mb-4">
        <Tabs<Tab>
          value={tab}
          onChange={setTab}
          tabs={[
            { id: "skills", label: "Skills" },
            { id: "plugins", label: "Plugins" },
            { id: "mcp", label: "MCP servers" },
            { id: "instructions", label: "Instructions" },
          ]}
        />
      </div>
      <ScopeBar tool={tool} setTool={setTool} project={project} setProject={setProject} projects={projects.data ?? []} noTool={tab === "skills"} />
      {tab === "skills" && <Skills project={project} />}
      {tab === "plugins" && <Plugins key={`${tool}${project}`} tool={tool} project={project} />}
      {tab === "mcp" && <Mcp key={`${tool}${project}`} tool={tool} project={project} />}
      {tab === "instructions" && <Instructions key={`${tool}${project}`} tool={tool} project={project} />}
    </>
  );
}
