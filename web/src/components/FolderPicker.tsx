import { useQuery } from "@tanstack/react-query";
import { ArrowUp, Folder, FolderGit2 } from "lucide-react";
import { useState } from "react";
import { api, qs } from "../lib/api";
import { Button, Input, Modal, Spinner } from "./ui";

/** Browse the server's folders and pick one. */
export function FolderPicker({ value, onChange }: { value: string; onChange: (p: string) => void }) {
  const [open, setOpen] = useState(false);
  const [path, setPath] = useState(value || "/");
  const q = useQuery({
    queryKey: ["browse", path],
    queryFn: () => api.get<any>(`/api/fs/browse${qs({ path })}`),
    enabled: open,
  });
  return (
    <>
      <div className="flex gap-2">
        <Input value={value} onChange={(e) => onChange(e.target.value)} placeholder="/var/www/my-app" />
        <Button
          type="button"
          variant="outline"
          onClick={() => {
            setPath(value || "/");
            setOpen(true);
          }}
        >
          Browse
        </Button>
      </div>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title="Choose a folder"
        footer={
          <>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                onChange(q.data?.path ?? path);
                setOpen(false);
              }}
            >
              Use this folder
            </Button>
          </>
        }
      >
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" disabled={!q.data?.parent} onClick={() => q.data?.parent && setPath(q.data.parent)} icon={<ArrowUp className="size-4" />}>
            Up
          </Button>
          <code className="text-xs truncate">{q.data?.path ?? path}</code>
        </div>
        {q.isLoading && <Spinner />}
        {q.error && <div className="text-sm text-danger">{(q.error as Error).message}</div>}
        <div className="rounded-lg border border-border divide-y divide-border max-h-80 overflow-y-auto scroll-thin">
          {q.data?.dirs?.map((d: any) => (
            <button key={d.path} onClick={() => setPath(d.path)} className="w-full flex items-center gap-2 px-3 py-2 text-sm hover:bg-panel-2 text-left">
              {d.git ? <FolderGit2 className="size-4 text-accent" /> : <Folder className="size-4 text-muted" />}
              {d.name}
            </button>
          ))}
          {q.data && q.data.dirs.length === 0 && <div className="px-3 py-4 text-sm text-muted">No sub-folders</div>}
        </div>
      </Modal>
    </>
  );
}
