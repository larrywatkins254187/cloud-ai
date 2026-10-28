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
export function AgentPicker({
  v,
  set,
  compact,
  lockTool,
  readOnly,
}: {
  v: AgentConfig;
  set: (c: AgentConfig) => void;
  compact?: boolean;
  lockTool?: boolean;
  /** Read-only chats have a fixed permission mode. */
  readOnly?: boolean;
}) {
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
        onChange={(t) => set({ ...v, tool: t, model: "", effort: "" })}
        aria-label="Tool"
        options={[
          { value: "claude" as Tool, label: "Claude Code" },
          { value: "codex" as Tool, label: "Codex" },
        ]}
      />
      <div className="flex gap-1 min-w-0">
        <Select
          className={cls}
          value={v.model}
          onChange={(m) => set({ ...v, model: m, effort: "" })}
          aria-label="Model"
          searchable={list.length > 5}
          options={[
            { value: "", label: "Default model" },
            ...list.map((m) => ({ value: m.id, label: m.name, hint: m.description || (m.name !== m.id ? m.id : undefined) })),
            ...(v.model && !list.some((m) => m.id === v.model) ? [{ value: v.model, label: v.model }] : []),
          ]}
        />
        {!compact && (
          <IconButton title="Refresh model list from the CLIs" onClick={refresh} disabled={refreshing}>
            <RefreshCw className={cx("size-4", refreshing && "animate-spin")} />
          </IconButton>
        )}
      </div>
      <Select
        className={cls}
        value={v.effort}
        onChange={(e) => set({ ...v, effort: e })}
        aria-label="Reasoning effort"
        options={[{ value: "", label: "Default effort" }, ...efforts.map((e) => ({ value: e, label: `${e[0].toUpperCase()}${e.slice(1)} effort` }))]}
      />
      {readOnly ? (
        <Select
          className={cls}
          value={"read-only"}
          disabled
          onChange={() => {}}
          aria-label="Permissions"
          options={[{ value: "read-only", label: "Read-only", hint: "Can read, cannot change anything" }]}
        />
      ) : (
        <Select
          className={cls}
          value={v.permission_mode}
          onChange={(m) => set({ ...v, permission_mode: m })}
          aria-label="Permissions"
          options={MODES.map((m) => ({ value: m.id as PermMode, label: m.label, hint: m.hint }))}
        />
      )}
    </div>
  );
}
