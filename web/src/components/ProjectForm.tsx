import { useQuery } from "@tanstack/react-query";
import { api } from "../lib/api";
import type { Project, Server, User } from "../lib/types";
import { Field, Input, NumberInput, Select, Textarea, Toggle } from "./ui";

export type ProjectDraft = Partial<Project> & { users?: number[] };

/** Fields shared by "add project" and the project settings tab. */
export function ProjectFields({ v, set, showUsers = true }: { v: ProjectDraft; set: (p: ProjectDraft) => void; showUsers?: boolean }) {
  const servers = useQuery({ queryKey: ["servers"], queryFn: () => api.get<Server[]>("/api/servers") });
  const users = useQuery({ queryKey: ["users"], queryFn: () => api.get<User[]>("/api/users"), enabled: showUsers });
  const up = (p: Partial<ProjectDraft>) => set({ ...v, ...p });
  return (
    <div className="space-y-5">
      <div className="grid sm:grid-cols-2 gap-4">
        <Field label="Name">
          <Input value={v.name ?? ""} onChange={(e) => up({ name: e.target.value })} />
        </Field>
        <Field label="Production URL" hint="Opened in the live browser and given to the AI as context">
          <Input value={v.prod_url ?? ""} onChange={(e) => up({ prod_url: e.target.value })} placeholder="https://example.com" />
        </Field>
      </div>
      <Field label="Description">
        <Input value={v.description ?? ""} onChange={(e) => up({ description: e.target.value })} />
      </Field>

      <div className="space-y-3">
        <div className="text-sm font-semibold">Dev server</div>
        <div className="grid sm:grid-cols-3 gap-4">
          <Field label="Command" className="sm:col-span-2" hint="Started and stopped from the Preview tab">
            <Input value={v.dev_command ?? ""} onChange={(e) => up({ dev_command: e.target.value })} placeholder="npm run dev -- -p 4001" />
          </Field>
          <Field label="Port">
            <NumberInput
              value={v.dev_port ?? ""}
              onChange={(x) => up({ dev_port: x ? Number(x) : null })}
              min={1}
              max={65535}
              placeholder="4001"
            />
          </Field>
        </div>
      </div>

      <div className="space-y-3">
        <div className="text-sm font-semibold">Deploy</div>
        <Field label="Where does it run?">
          <Select
            value={v.deploy_target ?? "none"}
            onChange={(t) => up({ deploy_target: t })}
            options={[
              { value: "none" as const, label: "No deploy" },
              { value: "local" as const, label: "On this server" },
              { value: "remote" as const, label: "On another server (SSH)" },
            ]}
          />
        </Field>
        {v.deploy_target === "remote" && (
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Server" hint={servers.data === undefined ? (servers.isError ? "Couldn't load the server list — check your connection" : undefined) : servers.data.length ? undefined : "Add a server under Admin → Servers first"}>
              <Select
                value={v.server_id ?? null}
                onChange={(id) => up({ server_id: id })}
                placeholder="Choose a server…"
                emptyText="No servers found"
                options={(servers.data ?? []).map((s) => ({ value: s.id, label: s.name, hint: `${s.username}@${s.host}:${s.port}` }))}
              />
            </Field>
            <Field label="Path on that server">
              <Input value={v.remote_path ?? ""} onChange={(e) => up({ remote_path: e.target.value })} placeholder="/var/www/my-app" />
            </Field>
            <Field label="How to ship the code" className="sm:col-span-2">
              <Select
                value={v.deploy_method ?? "git"}
                onChange={(m) => up({ deploy_method: m })}
                options={[
                  { value: "git" as const, label: "git push here, then git pull on the server" },
                  { value: "rsync" as const, label: "Copy files with rsync" },
                ]}
              />
            </Field>
            {v.deploy_method === "rsync" && (
              <Field label="Exclude from rsync (one per line)" className="sm:col-span-2" hint="Excluded files are never deleted on the server">
                <Textarea
                  value={v.rsync_excludes ?? ""}
                  onChange={(e) => up({ rsync_excludes: e.target.value })}
                  placeholder={"node_modules\n.git\n.next\n.env\n.env.local"}
                  className="font-mono text-xs"
                />
              </Field>
            )}
          </div>
        )}
        {v.deploy_target && v.deploy_target !== "none" && (
          <>
            <Field
              label={v.deploy_target === "remote" ? "Command to run on the server" : "Deploy command"}
              hint="Runs in the project folder with bash. $DEPLOY_REF holds the commit being deployed."
            >
              <Textarea
                value={v.deploy_command ?? ""}
                onChange={(e) => up({ deploy_command: e.target.value })}
                placeholder={"npm ci && npm run build && systemctl restart my-app"}
                className="font-mono text-xs"
              />
            </Field>
            <Field label="Health check URL" hint="Checked after each deploy; failure can roll back automatically">
              <Input value={v.health_url ?? ""} onChange={(e) => up({ health_url: e.target.value })} placeholder="https://example.com/api/health" />
            </Field>
            <Toggle checked={v.auto_commit ?? true} onChange={(b) => up({ auto_commit: b })} label="Commit pending changes before deploying" />
            <Toggle checked={v.auto_rollback ?? true} onChange={(b) => up({ auto_rollback: b })} label="Roll back automatically when the health check fails" />
          </>
        )}
      </div>

      {showUsers && (users.data?.length ?? 0) > 0 && (
        <div className="space-y-2">
          <div className="text-sm font-semibold">Users with access</div>
          <div className="text-xs text-muted">Admins always see every project.</div>
          <div className="flex flex-wrap gap-2">
            {users.data!
              .filter((u) => u.role !== "admin")
              .map((u) => {
                const on = v.users?.includes(u.id) ?? false;
                return (
                  <button
                    key={u.id}
                    type="button"
                    onClick={() => up({ users: on ? (v.users ?? []).filter((x) => x !== u.id) : [...(v.users ?? []), u.id] })}
                    className={`rounded-full border px-3 py-1 text-sm ${on ? "border-accent bg-accent-soft text-accent" : "border-border text-muted"}`}
                  >
                    {u.username}
                  </button>
                );
              })}
          </div>
        </div>
      )}
    </div>
  );
}
