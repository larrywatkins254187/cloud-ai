import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ErrorState, QueryStatus, SkeletonList } from "../../components/Loadable";
import { Copy, Pencil, Plug, Plus, Server as ServerIcon, Trash2 } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, Empty, Field, IconButton, Input, Modal, NumberInput, PageHeader, Select, Textarea, useAction, useConfirm, useToast } from "../../components/ui";
import { api } from "../../lib/api";
import type { Server } from "../../lib/types";

function ServerModal({ server, onClose, pubkey }: { server?: Server; onClose: () => void; pubkey?: string }) {
  const [v, setV] = useState({
    name: server?.name ?? "",
    host: server?.host ?? "",
    port: server?.port ?? 22,
    username: server?.username ?? "root",
    auth_type: server?.auth_type ?? "app_key",
    secret: "",
  });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const save = async () => {
    const r = await run(() => (server ? api.put(`/api/servers/${server.id}`, { ...v, port: Number(v.port) || 22 }) : api.post("/api/servers", { ...v, port: Number(v.port) || 22 })), "Saved");
    if (r) {
      qc.invalidateQueries({ queryKey: ["servers"] });
      onClose();
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={server ? `Edit ${server.name}` : "Add server"}
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
          <Input value={v.name} onChange={(e) => setV({ ...v, name: e.target.value })} placeholder="production-1" />
        </Field>
        <Field label="Host">
          <Input value={v.host} onChange={(e) => setV({ ...v, host: e.target.value })} placeholder="203.0.113.10" />
        </Field>
        <Field label="SSH user">
          <Input value={v.username} onChange={(e) => setV({ ...v, username: e.target.value })} />
        </Field>
        <Field label="Port">
          <NumberInput value={v.port} min={1} max={65535} onChange={(x) => setV({ ...v, port: x === "" ? ("" as unknown as number) : Number(x) })} />
        </Field>
      </div>
      <Field label="Authentication">
        <Select
          value={v.auth_type}
          onChange={(t) => setV({ ...v, auth_type: t })}
          options={[
            { value: "app_key" as const, label: "This panel's SSH key", hint: "Recommended" },
            { value: "key" as const, label: "My own private key" },
            { value: "password" as const, label: "Password" },
          ]}
        />
      </Field>
      {v.auth_type === "app_key" && pubkey && (
        <div className="space-y-1.5">
          <div className="text-xs text-muted">
            Add this line to <code>~/.ssh/authorized_keys</code> of <b>{v.username}</b> on that server:
          </div>
          <pre className="text-[11px] font-mono bg-code rounded-lg p-2.5 break-all whitespace-pre-wrap select-all">{pubkey}</pre>
        </div>
      )}
      {v.auth_type === "key" && (
        <Field label="Private key" hint={server ? "Leave empty to keep the stored key" : "Stored on this server with 0600 permissions"}>
          <Textarea value={v.secret} onChange={(e) => setV({ ...v, secret: e.target.value })} className="font-mono text-xs" placeholder="-----BEGIN OPENSSH PRIVATE KEY-----" />
        </Field>
      )}
      {v.auth_type === "password" && (
        <Field label="Password" hint="Stored encrypted. Needs sshpass on this server. The AI cannot use password servers.">
          <Input type="password" value={v.secret} onChange={(e) => setV({ ...v, secret: e.target.value })} placeholder={server ? "unchanged" : ""} />
        </Field>
      )}
    </Modal>
  );
}

export default function Servers() {
  const ask = useConfirm();
  const q = useQuery({ queryKey: ["servers"], queryFn: () => api.get<Server[]>("/api/servers") });
  const key = useQuery({ queryKey: ["pubkey"], queryFn: () => api.get<{ public_key: string }>("/api/ssh/public-key"), retry: false });
  const [edit, setEdit] = useState<Server | null | "new">(null);
  const { run } = useAction();
  const toast = useToast();
  const qc = useQueryClient();
  return (
    <>
      <PageHeader
        title="Servers"
        subtitle="Other machines your projects deploy to over SSH. Claude and Codex can SSH into them too."
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEdit("new")}>
            Add server
          </Button>
        }
      />
      <Card className="p-4 mb-4 space-y-2">
        <div className="text-sm font-medium">This panel's public SSH key</div>
        {key.isError && !key.data ? (
          <ErrorState compact error={key.error} onRetry={key.refetch} retrying={key.isFetching} />
        ) : (
          <div className="flex gap-2 items-start">
            <pre className="flex-1 text-[11px] font-mono bg-code rounded-lg p-2.5 break-all whitespace-pre-wrap">{key.data?.public_key ?? "…"}</pre>
            <IconButton
              title="Copy"
              onClick={() => {
                navigator.clipboard?.writeText(key.data?.public_key ?? "");
                toast("Copied", "ok");
              }}
            >
              <Copy className="size-4" />
            </IconButton>
          </div>
        )}
      </Card>
      <QueryStatus q={q} skeleton={<SkeletonList rows={3} />} />
      {q.data === undefined ? null : q.data.length === 0 ? (
        <Card>
          <Empty icon={<ServerIcon className="size-8" />} title="No servers yet" />
        </Card>
      ) : (
        <Card className="divide-y divide-border">
          {q.data?.map((s) => (
            <div key={s.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium">{s.name}</span>
                  <Badge>{s.auth_type === "app_key" ? "panel key" : s.auth_type}</Badge>
                </div>
                <div className="text-xs text-muted font-mono">
                  {s.username}@{s.host}:{s.port}
                </div>
                {s.auth_type !== "password" && (
                  <div className="text-xs text-muted mt-0.5">
                    AI access: <code>ssh -F /var/lib/cloud-ai/ssh/config cai-{s.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}</code>
                  </div>
                )}
              </div>
              <Button
                size="sm"
                variant="outline"
                icon={<Plug className="size-4" />}
                onClick={async () => {
                  const r = await run(() => api.post<any>(`/api/servers/${s.id}/test`));
                  if (r) r.ok ? toast(`Connected: ${r.output.split("\n")[1] ?? "ok"}`, "ok") : toast(r.error, "danger");
                }}
              >
                Test
              </Button>
              <IconButton title="Edit" onClick={() => setEdit(s)}>
                <Pencil className="size-4" />
              </IconButton>
              <IconButton
                title="Delete"
                onClick={async () => {
                  if (!(await ask({ title: "Delete server", message: `Delete server ${s.name}?`, confirmLabel: "Delete", danger: true }))) return;
                  await run(() => api.del(`/api/servers/${s.id}`), "Deleted");
                  qc.invalidateQueries({ queryKey: ["servers"] });
                }}
              >
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          ))}
        </Card>
      )}
      {edit && <ServerModal server={edit === "new" ? undefined : edit} pubkey={key.data?.public_key} onClose={() => setEdit(null)} />}
    </>
  );
}
