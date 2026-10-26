import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryStatus } from "../components/Loadable";
import { Plus, SquareTerminal, X } from "lucide-react";
import { lazy, Suspense, useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import { Button, cx, Empty, Select, Spinner, useAction } from "../components/ui";
import { api } from "../lib/api";
import type { Project } from "../lib/types";

const TerminalView = lazy(() => import("../components/TerminalView"));

type T = { id: string; title: string; cwd: string; alive: boolean };

export default function Terminal() {
  const [sp, setSp] = useSearchParams();
  const list = useQuery({ queryKey: ["terminals"], queryFn: () => api.get<T[]>("/api/terminals"), refetchInterval: 10_000 });
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  const [cwd, setCwd] = useState("");
  const active = sp.get("open") ?? list.data?.[0]?.id ?? null;
  const { run } = useAction();
  const qc = useQueryClient();

  useEffect(() => {
    if (sp.get("open") && list.data && !list.data.some((t) => t.id === sp.get("open"))) qc.invalidateQueries({ queryKey: ["terminals"] });
  }, [sp, list.data, qc]);

  const open = async () => {
    const t = await run(() => api.post<T>("/api/terminals", { cwd: cwd || undefined, title: cwd ? cwd.split("/").pop() : "shell" }));
    if (t) {
      await qc.invalidateQueries({ queryKey: ["terminals"] });
      setSp({ open: t.id });
    }
  };
  const close = async (id: string) => {
    await run(() => api.del(`/api/terminals/${id}`));
    await qc.invalidateQueries({ queryKey: ["terminals"] });
    if (active === id) setSp({});
  };

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-2 px-2 h-12 border-b border-border bg-panel shrink-0 overflow-x-auto scroll-thin">
        {list.data?.map((t) => (
          <div
            key={t.id}
            className={cx("flex items-center gap-1 rounded-lg pl-3 pr-1 h-8 text-sm shrink-0 cursor-pointer", t.id === active ? "bg-accent-soft text-accent" : "text-muted hover:bg-panel-2")}
            onClick={() => setSp({ open: t.id })}
          >
            <SquareTerminal className="size-3.5" />
            <span className="max-w-40 truncate">{t.title}</span>
            <button
              className="p-1 hover:text-danger"
              title="Close"
              onClick={(e) => {
                e.stopPropagation();
                close(t.id);
              }}
            >
              <X className="size-3.5" />
            </button>
          </div>
        ))}
        <div className="flex-1" />
        <Select className="h-8 w-44 text-xs shrink-0" value={cwd} onChange={(e) => setCwd(e.target.value)}>
          <option value="">Home folder</option>
          {projects.data?.map((p) => (
            <option key={p.id} value={p.path}>
              {p.name}
            </option>
          ))}
        </Select>
        <Button size="sm" variant="primary" icon={<Plus className="size-4" />} onClick={open} className="shrink-0">
          New
        </Button>
      </div>
      <div className="flex-1 min-h-0">
        {!active && list.data === undefined ? (
          <QueryStatus q={list} />
        ) : active ? (
          <Suspense fallback={<Spinner />}>
            <TerminalView key={active} id={active} />
          </Suspense>
        ) : (
          <Empty icon={<SquareTerminal className="size-8" />} title="No terminal open">
            Open a shell on the server (as the panel's user). Sessions keep running when you close the page.
          </Empty>
        )}
      </div>
    </div>
  );
}
