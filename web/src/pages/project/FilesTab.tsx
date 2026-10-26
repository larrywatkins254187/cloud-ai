import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryStatus } from "../../components/Loadable";
import { ChevronRight, File, FilePlus, Folder, FolderPlus, Pencil, RefreshCw, Save, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Button, Card, cx, IconButton, useAction, useConfirm } from "../../components/ui";
import { api, qs } from "../../lib/api";
import { bytes } from "../../lib/format";
import type { Project } from "../../lib/types";

type Item = { name: string; path: string; dir: boolean; size: number };

export default function FilesTab({ project }: { project: Project }) {
  const ask = useConfirm();
  const [dir, setDir] = useState("");
  const [file, setFile] = useState<string | null>(null);
  const base = `/api/projects/${project.id}/files`;
  const list = useQuery({ queryKey: ["files", project.id, dir], queryFn: () => api.get<{ items: Item[] }>(`${base}${qs({ path: dir })}`) });
  const qc = useQueryClient();
  const { run } = useAction();
  const crumbs = dir ? dir.split("/") : [];
  const reload = () => qc.invalidateQueries({ queryKey: ["files", project.id] });

  const create = async (isDir: boolean) => {
    const name = prompt(isDir ? "New folder name" : "New file name");
    if (!name) return;
    const path = dir ? `${dir}/${name}` : name;
    if (isDir) await run(() => api.post(`${base}/mkdir`, { path }));
    else {
      await run(() => api.put(`${base}/content`, { path, content: "" }));
      setFile(path);
    }
    reload();
  };

  const rename = async (it: Item) => {
    const to = prompt("Rename to", it.path);
    if (!to || to === it.path) return;
    await run(() => api.post(`${base}/rename`, { from: it.path, to }));
    reload();
  };

  const remove = async (it: Item) => {
    if (!(await ask({ title: it.dir ? "Delete folder" : "Delete file", message: `Delete ${it.path}${it.dir ? " and everything inside it" : ""}?`, confirmLabel: "Delete", danger: true }))) return;
    await run(() => api.del(`${base}${qs({ path: it.path })}`));
    if (file === it.path) setFile(null);
    reload();
  };

  return (
    <div className={cx("grid gap-4", file ? "lg:grid-cols-[320px_1fr]" : "")}>
      <Card className={cx("overflow-hidden", file && "hidden lg:block")}>
        <div className="flex items-center gap-1 px-3 py-2 border-b border-border text-sm overflow-x-auto scroll-thin">
          <button className="text-accent shrink-0" onClick={() => setDir("")}>
            {project.name}
          </button>
          {crumbs.map((c, i) => (
            <span key={i} className="flex items-center gap-1 shrink-0">
              <ChevronRight className="size-3.5 text-muted" />
              <button className="hover:text-accent" onClick={() => setDir(crumbs.slice(0, i + 1).join("/"))}>
                {c}
              </button>
            </span>
          ))}
          <div className="flex-1" />
          <IconButton title="New file" onClick={() => create(false)}>
            <FilePlus className="size-4" />
          </IconButton>
          <IconButton title="New folder" onClick={() => create(true)}>
            <FolderPlus className="size-4" />
          </IconButton>
          <IconButton title="Refresh" onClick={reload}>
            <RefreshCw className="size-4" />
          </IconButton>
        </div>
        <QueryStatus q={list} />
        <div className="divide-y divide-border max-h-[65dvh] overflow-y-auto scroll-thin">
          {list.data?.items.map((it) => (
            <div key={it.path} className={cx("group flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-panel-2", file === it.path && "bg-accent-soft")}>
              <button className="flex items-center gap-2 min-w-0 flex-1 text-left" onClick={() => (it.dir ? setDir(it.path) : setFile(it.path))}>
                {it.dir ? <Folder className="size-4 text-accent shrink-0" /> : <File className="size-4 text-muted shrink-0" />}
                <span className="truncate">{it.name}</span>
              </button>
              {!it.dir && <span className="text-xs text-muted">{bytes(it.size)}</span>}
              <div className="flex opacity-100 lg:opacity-0 group-hover:opacity-100">
                <button className="p-1 text-muted hover:text-text" onClick={() => rename(it)} title="Rename">
                  <Pencil className="size-3.5" />
                </button>
                <button className="p-1 text-muted hover:text-danger" onClick={() => remove(it)} title="Delete">
                  <Trash2 className="size-3.5" />
                </button>
              </div>
            </div>
          ))}
          {list.data?.items.length === 0 && <div className="px-3 py-6 text-sm text-muted text-center">Empty folder</div>}
        </div>
      </Card>
      {file && <Editor key={file} base={base} path={file} onClose={() => setFile(null)} />}
    </div>
  );
}

function Editor({ base, path, onClose }: { base: string; path: string; onClose: () => void }) {
  const q = useQuery({ queryKey: ["file", base, path], queryFn: () => api.get<any>(`${base}/content${qs({ path })}`), staleTime: 0 });
  const [text, setText] = useState<string | null>(null);
  const { busy, run } = useAction();
  useEffect(() => {
    if (q.data?.content !== undefined) setText(q.data.content);
  }, [q.data]);
  const dirty = text !== null && q.data?.content !== undefined && text !== q.data.content;
  const save = async () => {
    if (text === null) return;
    await run(() => api.put(`${base}/content`, { path, content: text }), "Saved");
    q.refetch();
  };
  return (
    <Card className="flex flex-col min-h-[60dvh] overflow-hidden">
      <div className="flex items-center gap-2 px-3 py-2 border-b border-border">
        <code className="text-xs truncate flex-1">{path}</code>
        {dirty && <span className="text-xs text-warn">unsaved</span>}
        <Button size="sm" variant="primary" icon={<Save className="size-4" />} loading={busy} disabled={!dirty} onClick={save}>
          Save
        </Button>
        <IconButton title="Close" onClick={onClose}>
          <X className="size-4" />
        </IconButton>
      </div>
      <QueryStatus q={q} />
      {q.data?.binary ? (
        <div className="p-6 text-sm text-muted">{q.data.too_large ? "File is too large to edit here." : "Binary file — not editable."}</div>
      ) : (
        text !== null && (
          <textarea
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if ((e.ctrlKey || e.metaKey) && e.key === "s") {
                e.preventDefault();
                save();
              }
              if (e.key === "Tab") {
                e.preventDefault();
                const el = e.currentTarget;
                const s = el.selectionStart;
                const v = el.value.slice(0, s) + "  " + el.value.slice(el.selectionEnd);
                setText(v);
                requestAnimationFrame(() => el.setSelectionRange(s + 2, s + 2));
              }
            }}
            spellCheck={false}
            className="flex-1 w-full resize-none bg-code p-3 font-mono text-[13px] leading-5 focus:outline-none"
          />
        )
      )}
    </Card>
  );
}
