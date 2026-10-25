import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderGit2, Plus, Rocket } from "lucide-react";
import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { FolderPicker } from "../components/FolderPicker";
import { ProjectFields, type ProjectDraft } from "../components/ProjectForm";
import { Badge, Button, Card, Empty, Field, Input, Modal, PageHeader, Select, Spinner, Tabs, useAction } from "../components/ui";
import { api } from "../lib/api";
import { useIsAdmin } from "../lib/auth";
import type { Project, Server } from "../lib/types";

type Source = "existing" | "new" | "clone" | "remote";

function AddProject({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [source, setSource] = useState<Source>("existing");
  const [path, setPath] = useState("");
  const [gitUrl, setGitUrl] = useState("");
  const [importServer, setImportServer] = useState<number | "">("");
  const [importPath, setImportPath] = useState("");
  const [draft, setDraft] = useState<ProjectDraft>({ deploy_target: "none", deploy_method: "git", auto_commit: true, auto_rollback: true });
  const servers = useQuery({ queryKey: ["servers"], queryFn: () => api.get<Server[]>("/api/servers"), enabled: open });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const nav = useNavigate();

  const submit = async () => {
    const p = await run(
      () =>
        api.post<Project>("/api/projects", {
          ...draft,
          source,
          path,
          git_url: gitUrl,
          import_server_id: importServer || undefined,
          import_remote_path: importPath,
        }),
      "Project added",
    );
    if (p) {
      qc.invalidateQueries({ queryKey: ["projects"] });
      onClose();
      nav(`/projects/${p.id}`);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Add project"
      wide
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={submit}>
            {source === "clone" ? "Clone & add" : source === "remote" ? "Copy & add" : "Add project"}
          </Button>
        </>
      }
    >
      <Tabs<Source>
        value={source}
        onChange={setSource}
        tabs={[
          { id: "existing", label: "Existing folder" },
          { id: "new", label: "New folder" },
          { id: "clone", label: "Clone from git" },
          { id: "remote", label: "From another server" },
        ]}
      />
      {source === "clone" && (
        <Field label="Git URL" hint="Private repos need SSH keys or a token in the URL on this server">
          <Input value={gitUrl} onChange={(e) => setGitUrl(e.target.value)} placeholder="git@github.com:me/app.git" />
        </Field>
      )}
      {source === "remote" && (
        <div className="grid sm:grid-cols-2 gap-4">
          <Field label="Server">
            <Select value={importServer} onChange={(e) => setImportServer(e.target.value ? Number(e.target.value) : "")}>
              <option value="">Choose…</option>
              {servers.data?.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Path on that server">
            <Input value={importPath} onChange={(e) => setImportPath(e.target.value)} placeholder="/var/www/app" />
          </Field>
        </div>
      )}
      <Field
        label={source === "existing" ? "Folder on this server" : "Local folder to create"}
        hint={source === "remote" ? "The code is copied here (rsync); the AI works on this copy and deploys go back to the server." : undefined}
      >
        {source === "existing" ? <FolderPicker value={path} onChange={setPath} /> : <Input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/root/projects/my-app" />}
      </Field>
      <div className="border-t border-border pt-4">
        <ProjectFields v={draft} set={setDraft} />
      </div>
    </Modal>
  );
}

export default function Projects() {
  const admin = useIsAdmin();
  const [adding, setAdding] = useState(false);
  const q = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  return (
    <>
      <PageHeader
        title="Projects"
        subtitle="Folders on your server that Claude Code and Codex work in"
        actions={
          admin && (
            <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setAdding(true)}>
              Add project
            </Button>
          )
        }
      />
      {q.isLoading && <Spinner />}
      {q.data?.length === 0 && (
        <Card>
          <Empty icon={<FolderGit2 className="size-8" />} title="No projects yet">
            {admin ? "Add a folder on this server, clone a repository or import one from another server." : "Ask an admin to give you access to a project."}
          </Empty>
        </Card>
      )}
      <div className="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
        {q.data?.map((p) => (
          <Link key={p.id} to={`/projects/${p.id}`}>
            <Card className="p-4 h-full hover:border-accent/60 transition">
              <div className="flex items-start justify-between gap-2">
                <div className="font-medium truncate">{p.name}</div>
                {(p.active_count ?? 0) > 0 && <Badge tone="accent">{p.active_count} active</Badge>}
              </div>
              <div className="text-xs text-muted truncate mt-0.5 font-mono">{p.path}</div>
              {p.description && <div className="text-sm text-muted mt-2 line-clamp-2">{p.description}</div>}
              <div className="flex flex-wrap gap-1.5 mt-3">
                <Badge>{p.chat_count ?? 0} chats</Badge>
                {p.deployable && (
                  <Badge tone="ok">
                    <Rocket className="size-3" />
                    {p.deploy_target === "remote" ? "remote deploy" : "deploy"}
                  </Badge>
                )}
              </div>
            </Card>
          </Link>
        ))}
      </div>
      <AddProject open={adding} onClose={() => setAdding(false)} />
    </>
  );
}
