import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryStatus } from "../../components/Loadable";
import { KeyRound, Pencil, Plus, ShieldOff, Trash2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Field, IconButton, Input, Modal, PageHeader, Select, Toggle, useAction, useConfirm } from "../../components/ui";
import { api } from "../../lib/api";
import { useAuth } from "../../lib/auth";
import { timeAgo } from "../../lib/format";
import type { Project, User } from "../../lib/types";

function UserModal({ user, onClose }: { user?: User; onClose: () => void }) {
  const [username, setUsername] = useState(user?.username ?? "");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState(user?.role ?? "user");
  const [disabled, setDisabled] = useState(user?.disabled ?? false);
  const [projects, setProjects] = useState<number[]>(user?.projects ?? []);
  const all = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const save = async () => {
    const r = await run(
      () =>
        user
          ? api.put(`/api/users/${user.id}`, { role, password: password || undefined, disabled, projects })
          : api.post("/api/users", { username, password, role, projects }),
      "Saved",
    );
    if (r) {
      qc.invalidateQueries({ queryKey: ["users"] });
      qc.invalidateQueries({ queryKey: ["projects"] });
      onClose();
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={user ? `Edit ${user.username}` : "New user"}
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
      {!user && (
        <Field label="Username">
          <Input value={username} onChange={(e) => setUsername(e.target.value)} autoFocus />
        </Field>
      )}
      <Field label={user ? "New password (leave empty to keep)" : "Password"} hint="At least 8 characters. They set up 2FA on first login.">
        <Input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
      </Field>
      <Field label="Role">
        <Select value={role} onChange={(e) => setRole(e.target.value as any)}>
          <option value="user">User — only assigned projects</option>
          <option value="admin">Admin — everything</option>
        </Select>
      </Field>
      {role === "user" && (
        <div className="space-y-2">
          <div className="text-[13px] font-medium">Projects</div>
          <div className="flex flex-wrap gap-2">
            {all.data?.map((p) => {
              const on = projects.includes(p.id);
              return (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => setProjects(on ? projects.filter((x) => x !== p.id) : [...projects, p.id])}
                  className={`rounded-full border px-3 py-1 text-sm ${on ? "border-accent bg-accent-soft text-accent" : "border-border text-muted"}`}
                >
                  {p.name}
                </button>
              );
            })}
            {all.data?.length === 0 && <span className="text-sm text-muted">No projects yet</span>}
            {all.data === undefined && <QueryStatus q={all} compact title="Projects" skeleton={<span className="text-sm text-muted">Loading projects…</span>} />}
          </div>
        </div>
      )}
      {user && <Toggle checked={disabled} onChange={setDisabled} label="Disabled" hint="Disabled users cannot log in" />}
    </Modal>
  );
}

export default function Users() {
  const ask = useConfirm();
  const me = useAuth().user;
  const q = useQuery({ queryKey: ["users"], queryFn: () => api.get<User[]>("/api/users") });
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  const [edit, setEdit] = useState<User | null | "new">(null);
  const { run } = useAction();
  const qc = useQueryClient();
  const pname = (id: number) => projects.data?.find((p) => p.id === id)?.name;
  return (
    <>
      <PageHeader
        title="Users"
        subtitle="Admins manage everything; users only see the projects you assign"
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEdit("new")}>
            New user
          </Button>
        }
      />
      <Card className="divide-y divide-border">
        <QueryStatus q={q} />
        {q.data?.map((u) => (
          <div key={u.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <span className="font-medium">{u.username}</span>
                <Badge tone={u.role === "admin" ? "accent" : "neutral"}>{u.role}</Badge>
                {!u.totp_enabled && <Badge tone="warn">2FA not set up</Badge>}
                {u.disabled && <Badge tone="danger">disabled</Badge>}
              </div>
              <div className="text-xs text-muted mt-0.5">
                {u.role === "admin" ? "All projects" : (u.projects ?? []).map(pname).filter(Boolean).join(", ") || "No projects"} · created {timeAgo(u.created_at)}
              </div>
            </div>
            <IconButton title="Edit" onClick={() => setEdit(u)}>
              <Pencil className="size-4" />
            </IconButton>
            <IconButton
              title="Reset 2FA"
              onClick={async () => {
                if (!(await ask({ title: "Reset 2FA", message: `Reset 2FA for ${u.username}? They will scan a new QR code on next login.`, confirmLabel: "Reset 2FA", danger: true }))) return;
                await run(() => api.post(`/api/users/${u.id}/reset-2fa`), "2FA reset");
                qc.invalidateQueries({ queryKey: ["users"] });
              }}
            >
              <ShieldOff className="size-4" />
            </IconButton>
            {u.id !== me.id && (
              <IconButton
                title="Delete"
                onClick={async () => {
                  if (!(await ask({ title: "Delete user", message: `Delete ${u.username}?`, confirmLabel: "Delete", danger: true }))) return;
                  await run(() => api.del(`/api/users/${u.id}`), "Deleted");
                  qc.invalidateQueries({ queryKey: ["users"] });
                }}
              >
                <Trash2 className="size-4" />
              </IconButton>
            )}
          </div>
        ))}
      </Card>
      <p className="text-xs text-muted mt-3 flex items-center gap-1.5">
        <KeyRound className="size-3.5" /> Locked out? On the server run <code>cloud-ai reset-2fa &lt;user&gt;</code> or <code>cloud-ai set-password &lt;user&gt;</code>.
      </p>
      {edit && <UserModal user={edit === "new" ? undefined : edit} onClose={() => setEdit(null)} />}
    </>
  );
}
