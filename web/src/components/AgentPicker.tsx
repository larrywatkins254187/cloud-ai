import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { api } from "../lib/api";
import { MODES } from "../lib/format";
import type { Models, PermMode, Tool } from "../lib/types";
import { cx, IconButton, Select } from "./ui";
import { useState } from "react";

export interface AgentConfig {
  tool: Tool;
  model: string;
  effort: string;
  permission_mode: PermMode;
}

export function useModels() {
  return useQuery({ queryKey: ["models"], queryFn: () => api.get<Models>("/api/models"), staleTime: 5 * 60_000 });
}

/** Tool, model, reasoning effort and permission mode selectors. */
export function AgentPicker({ v, set, compact, lockTool }: { v: AgentConfig; set: (c: AgentConfig) => void; compact?: boolean; lockTool?: boolean }) {
  const models = useModels();
  const [refreshing, setRefreshing] = useState(false);
  const list = models.data?.[v.tool] ?? [];
  const current = list.find((m) => m.id === v.model) ?? list.find((m) => m.is_default);
  const efforts = current?.efforts ?? [];
  const refresh = async () => {
    setRefreshing(true);
    try {
      await api.post("/api/models/refresh");
      await models.refetch();
    } finally {
      setRefreshing(false);
    }
  };
  const cls = compact ? "h-8 text-[13px]" : "";
  return (
    <div className={cx("grid gap-2", compact ? "grid-cols-2 sm:flex sm:flex-wrap" : "sm:grid-cols-4 grid-cols-2")}>
      <Select
        className={cls}
        value={v.tool}
        disabled={lockTool}
        onChange={(e) => set({ ...v, tool: e.target.value as Tool, model: "", effort: "" })}
        aria-label="Tool"
      >
        <option value="claude">Claude Code</option>
        <option value="codex">Codex</option>
      </Select>
      <div className="flex gap-1">
        <Select className={cls} value={v.model} onChange={(e) => set({ ...v, model: e.target.value, effort: "" })} aria-label="Model">
          <option value="">Default model</option>
          {list.map((m) => (
            <option key={m.id} value={m.id} title={m.description}>
              {m.name}
            </option>
          ))}
          {v.model && !list.some((m) => m.id === v.model) && <option value={v.model}>{v.model}</option>}
        </Select>
        {!compact && (
          <IconButton title="Refresh model list from the CLIs" onClick={refresh} disabled={refreshing}>
            <RefreshCw className={cx("size-4", refreshing && "animate-spin")} />
          </IconButton>
        )}
      </div>
      <Select className={cls} value={v.effort} onChange={(e) => set({ ...v, effort: e.target.value })} aria-label="Reasoning effort">
        <option value="">Default effort</option>
        {efforts.map((e) => (
          <option key={e} value={e}>
            {e} effort
          </option>
        ))}
      </Select>
      <Select className={cls} value={v.permission_mode} onChange={(e) => set({ ...v, permission_mode: e.target.value as PermMode })} aria-label="Permissions">
        {MODES.map((m) => (
          <option key={m.id} value={m.id} title={m.hint}>
            {m.label}
          </option>
        ))}
      </Select>
    </div>
  );
}
