import {
  Bot,
  CheckCircle2,
  ChevronRight,
  Circle,
  CircleDot,
  FileEdit,
  FilePlus2,
  FileSearch,
  Globe,
  ListTodo,
  Loader2,
  Search,
  Terminal,
  Wrench,
  XCircle,
} from "lucide-react";
import { memo, useState } from "react";
import type { Msg } from "../lib/types";
import { Diff } from "./Diff";
import { cx } from "./ui";

function icon(name: string) {
  if (name === "Bash") return Terminal;
  if (name === "Edit" || name === "MultiEdit" || name === "NotebookEdit") return FileEdit;
  if (name === "Write") return FilePlus2;
  if (name === "Read") return FileSearch;
  if (name === "Grep" || name === "Glob") return Search;
  if (name === "WebFetch" || name === "WebSearch") return Globe;
  if (name === "TodoWrite") return ListTodo;
  if (name === "Task" || name === "Agent") return Bot;
  return Wrench;
}

function short(p: string) {
  const parts = p.split("/");
  return parts.length > 3 ? "…/" + parts.slice(-3).join("/") : p;
}

export function toolSummary(name: string, input: any): string {
  if (!input) return "";
  switch (name) {
    case "Bash":
      return input.command ?? "";
    case "Read":
    case "Write":
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return short(input.file_path ?? input.changes?.map((c: any) => c.path).join(", ") ?? "");
    case "Grep":
      return `${input.pattern ?? ""}${input.path ? " in " + short(input.path) : ""}`;
    case "Glob":
      return input.pattern ?? "";
    case "WebFetch":
      return input.url ?? "";
    case "WebSearch":
      return input.query ?? input.action?.query ?? "";
    case "Task":
    case "Agent":
      return input.description ?? input.prompt?.slice(0, 80) ?? "";
    case "TodoWrite":
      return `${input.todos?.length ?? 0} items`;
    case "Skill":
      return input.skill ?? input.name ?? "";
    default:
      return typeof input === "object" ? Object.values(input).filter((v) => typeof v === "string").join(" · ").slice(0, 120) : "";
  }
}

function editDiff(input: any): string | null {
  if (input?.changes) return input.changes.map((c: any) => `--- ${c.path} (${c.kind?.type ?? c.kind ?? ""})\n${c.diff ?? ""}`).join("\n");
  const one = (o: string, n: string) =>
    [...(o ? o.split("\n").map((l) => "-" + l) : []), ...(n ? n.split("\n").map((l) => "+" + l) : [])].join("\n");
  if (input?.old_string !== undefined) return `@@ ${input.file_path}\n${one(input.old_string, input.new_string)}`;
  if (input?.edits) return input.edits.map((e: any) => `@@ ${input.file_path}\n${one(e.old_string, e.new_string)}`).join("\n");
  if (input?.content !== undefined && input?.file_path) return `@@ ${input.file_path} (new file)\n${one("", input.content)}`;
  return null;
}

export function Todos({ todos }: { todos: { content?: string; step?: string; status: string; activeForm?: string }[] }) {
  return (
    <ul className="space-y-1 text-sm">
      {todos.map((t, i) => {
        const done = t.status === "completed";
        const active = t.status === "in_progress" || t.status === "inProgress";
        return (
          <li key={i} className="flex items-start gap-2">
            {done ? (
              <CheckCircle2 className="size-4 text-ok mt-0.5 shrink-0" />
            ) : active ? (
              <CircleDot className="size-4 text-accent mt-0.5 shrink-0" />
            ) : (
              <Circle className="size-4 text-muted mt-0.5 shrink-0" />
            )}
            <span className={cx(done && "line-through text-muted")}>{t.content ?? t.step}</span>
          </li>
        );
      })}
    </ul>
  );
}

export const ToolCard = memo(function ToolCard({
  use,
  result,
  children,
  badge,
  defaultOpen = false,
}: {
  use: Msg;
  result?: Msg;
  children?: React.ReactNode;
  /** Replaces the pending/error indicator (e.g. a sub-agent's status). */
  badge?: React.ReactNode;
  defaultOpen?: boolean;
}) {
  const name: string = use.data.name ?? "tool";
  const input = use.data.input;
  const [open, setOpen] = useState(defaultOpen);
  const Icon = icon(name);
  const pending = !result;
  const err = result?.data?.is_error;
  if (name === "TodoWrite" && input?.todos) {
    return (
      <div className="rounded-xl border border-border bg-panel px-3 py-2.5">
        <div className="flex items-center gap-2 text-xs text-muted mb-1.5">
          <ListTodo className="size-3.5" /> Tasks
        </div>
        <Todos todos={input.todos} />
      </div>
    );
  }
  const diff = ["Edit", "MultiEdit", "Write"].includes(name) ? editDiff(input) : null;
  const output = result?.data?.output;
  return (
    <div className={cx("rounded-xl border bg-panel overflow-hidden", err ? "border-danger/40" : "border-border")}>
      <button onClick={() => setOpen(!open)} className="w-full flex items-center gap-2 px-3 py-2 text-left text-sm hover:bg-panel-2">
        <ChevronRight className={cx("size-3.5 text-muted transition shrink-0", open && "rotate-90")} />
        <Icon className={cx("size-4 shrink-0", err ? "text-danger" : "text-muted")} />
        <span className="font-medium shrink-0">{name.startsWith("mcp__") ? name.split("__").slice(1).join(" · ") : name}</span>
        <span className="truncate text-muted font-mono text-xs">{toolSummary(name, input)}</span>
        <span className="flex-1" />
        {badge ?? (pending ? <Loader2 className="size-3.5 animate-spin text-muted shrink-0" /> : err ? <XCircle className="size-4 text-danger shrink-0" /> : null)}
      </button>
      {open && (
        <div className="border-t border-border p-3 space-y-2">
          {diff ? (
            <Diff text={diff} className="max-h-96" />
          ) : (
            <pre className="text-[12px] font-mono bg-code rounded-lg p-2.5 overflow-x-auto scroll-thin max-h-72 whitespace-pre-wrap break-all">
              {name === "Bash" ? input?.command : JSON.stringify(input, null, 2)}
            </pre>
          )}
          {output !== undefined && output !== null && output !== "" && (
            <pre
              className={cx(
                "text-[12px] font-mono rounded-lg p-2.5 overflow-auto scroll-thin max-h-80 whitespace-pre-wrap break-all",
                err ? "bg-danger-soft text-danger" : "bg-code text-muted",
              )}
            >
              {typeof output === "string" ? output : JSON.stringify(output, null, 2)}
            </pre>
          )}
          {children}
        </div>
      )}
    </div>
  );
});
