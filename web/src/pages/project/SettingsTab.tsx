import { useQueryClient } from "@tanstack/react-query";
import { Save, Trash2 } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { FolderPicker } from "../../components/FolderPicker";
import { ProjectFields, type ProjectDraft } from "../../components/ProjectForm";
import { Button, Card, Field, useAction, useConfirm } from "../../components/ui";
import { api } from "../../lib/api";
import type { Project } from "../../lib/types";

export default function SettingsTab({ project }: { project: Project }) {
  const ask = useConfirm();
  const [v, setV] = useState<ProjectDraft>({ ...project });
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const nav = useNavigate();
  const save = async () => {
    const r = await run(() => api.put(`/api/projects/${project.id}`, v), "Saved");
    if (r) {
      qc.invalidateQueries({ queryKey: ["project", project.id] });
      qc.invalidateQueries({ queryKey: ["projects"] });
    }
  };
  const remove = async () => {
    if (!(await ask({ title: "Remove project", message: `Remove ${project.name} from the panel? Its chats and schedules are deleted; files on disk are kept.`, confirmLabel: "Remove", danger: true }))) return;
    const r = await run(() => api.del(`/api/projects/${project.id}`), "Project removed");
    if (r) {
      qc.invalidateQueries({ queryKey: ["projects"] });
      nav("/projects");
    }
  };
  return (
    <Card className="p-5 space-y-5 max-w-3xl">
      <Field label="Folder">
        <FolderPicker value={v.path ?? ""} onChange={(path) => setV({ ...v, path })} />
      </Field>
      <ProjectFields v={v} set={setV} />
      <div className="flex justify-between pt-2 border-t border-border">
        <Button variant="ghost" className="text-danger" icon={<Trash2 className="size-4" />} onClick={remove}>
          Remove project
        </Button>
        <Button variant="primary" icon={<Save className="size-4" />} loading={busy} onClick={save}>
          Save
        </Button>
      </div>
    </Card>
  );
}
