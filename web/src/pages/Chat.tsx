import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  ArrowLeft,
  Bot,
  PanelRight,
  Square,
  SquareTerminal,
  Skull,
  Brain,
  ChevronRight,
  Clock,
  Eye,
  FileText,
  GitMerge,
  Loader2,
  MoreVertical,
  Pencil,
  Rocket,
  SlidersHorizontal,
  Trash2,
  X,
} from "lucide-react";
import { memo, useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { AgentPicker } from "../components/AgentPicker";
import { Composer } from "../components/Composer";
import { Markdown } from "../components/Markdown";
import { RequestCard } from "../components/RequestCard";
import { Todos, ToolCard } from "../components/ToolCard";
import { DeployBanner } from "../components/DeployStatus";
import { ErrorState, queryGate, Skeleton, StaleNotice } from "../components/Loadable";
import { AgentsPanel, AgentStatusBadge, useStopAgent, type PanelTab } from "../components/AgentsPanel";
import { ContextMeter } from "../components/ContextMeter";
import { FilesList, useChanges } from "../components/FilesPanel";
import { buildAgents, isAgentTool, type SubAgent } from "../lib/agents";
import { BackgroundList, useBackgroundActions } from "../components/BackgroundPanel";
import { buildBackground, isBackgroundCall, withLiveStatus, type BgTask } from "../lib/background";
import { Badge, Button, cx, IconButton, Modal, StatusDot, Toggle, useAction, useConfirm, useConfirmWithOption, useToast } from "../components/ui";
import { api } from "../lib/api";
import { duration, modeLabel, timeAgo, tokens, toolName } from "../lib/format";
import { useAgentProgress, useStream, type MessagesData } from "../lib/live";
import type { Attachment, Chat as ChatT, Msg, Req } from "../lib/types";

type Item =
  | { type: "msg"; m: Msg; children?: Msg[] }
  | { type: "tool"; use: Msg; result?: Msg; children: Msg[] }
  | { type: "req"; r: Req; at: string };

/** Pair tool calls with their results and nest sub-agent messages under their Task call. */
function buildItems(messages: Msg[], requests: Req[]): Item[] {
  const results = new Map<string, Msg>();
  const byParent = new Map<string, Msg[]>();
  for (const m of messages) {
    if (m.kind === "tool_result" && m.data?.tool_use_id) results.set(m.data.tool_use_id, m);
    const parent = m.data?.parent;
    if (parent) {
      if (!byParent.has(parent)) byParent.set(parent, []);
      byParent.get(parent)!.push(m);
    }
  }
  const items: Item[] = [];
  for (const m of messages) {
    if (m.data?.parent) continue;
    if (m.kind === "tool_result" || m.kind === "agent" || m.kind === "shell") continue;
    if (m.kind === "tool_use") {
      const id = m.data.id;
      if (m.data.name === "AskUserQuestion") continue;
      items.push({ type: "tool", use: m, result: results.get(id), children: byParent.get(id) ?? [] });
    } else items.push({ type: "msg", m });
  }
  const reqItems: Item[] = requests.map((r) => ({ type: "req", r, at: r.created_at }));
  // Merge requests in by time.
  const all = [...items, ...reqItems];
  const at = (i: Item) => (i.type === "req" ? i.at : i.type === "tool" ? i.use.created_at : i.m.created_at);
  const idx = (i: Item) => (i.type === "req" ? 0 : i.type === "tool" ? i.use.id : i.m.id);
  return all.sort((a, b) => at(a).localeCompare(at(b)) || idx(a) - idx(b));
}

/** A sub-agent's own steps (its messages with the parent link removed). */
function renderSteps(msgs: Msg[]) {
  return buildItems(
    msgs.map((c) => ({ ...c, data: { ...c.data, parent: null } })),
    [],
  ).map((c) => (c.type === "tool" ? <ToolCard key={c.use.id} use={c.use} result={c.result} /> : c.type === "msg" ? <MsgView key={c.m.id} m={c.m} /> : null));
}

const PANEL_KEY = "chat:agents-panel";
function readPanelPref() {
  // On phones the panel is a drawer over the chat, so it always starts closed.
  if (window.innerWidth < 1024) return false;
  try {
    return localStorage.getItem(PANEL_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * The streaming reply of a running prompt. It subscribes to the stream on its own,
 * so each streamed token re-renders only this, not the whole conversation.
 */
function LiveTail({
  chatId,
  waiting,
  background,
  startedAt,
  onGrow,
}: {
  chatId: number;
  waiting: boolean;
  background?: boolean;
  startedAt?: string | null;
  onGrow: () => void;
}) {
  const stream = useStream(chatId);
  // Markdown re-parsing can lag behind fast streams without blocking typing or scrolling.
  const text = useDeferredValue(stream?.text ?? "");
  useLayoutEffect(onGrow, [text, stream?.thinking, onGrow]);
  return (
    <div className="space-y-2">
      {stream?.thinking && <Thinking text={stream.thinking} live />}
      {text && <Markdown text={text + " ▍"} />}
      <div className="flex items-center gap-2 text-xs text-muted">
        <Loader2 className="size-3.5 animate-spin" />
        {waiting ? "Waiting for your answer…" : stream?.activity || (background ? "Waiting for background tasks to finish…" : "Working…")}
        {startedAt && <span>· {duration(startedAt)}</span>}
      </div>
    </div>
  );
}

function MessagesSkeleton() {
  return (
    <div role="status" aria-label="Loading messages" className="space-y-4">
      <div className="flex justify-end">
        <Skeleton className="h-10 w-2/3 rounded-2xl" />
      </div>
      <div className="space-y-2">
        <Skeleton className="h-3.5 w-11/12" />
        <Skeleton className="h-3.5 w-4/5" />
        <Skeleton className="h-3.5 w-2/3" />
      </div>
      <Skeleton className="h-9 w-full rounded-xl" />
    </div>
  );
}

function UserBubble({ m }: { m: Msg }) {
  const atts: Attachment[] = m.data.attachments ?? [];
  return (
    <div className="flex justify-end">
      <div className="max-w-[88%] sm:max-w-[75%] space-y-1">
        <div className="text-[11px] text-muted text-right">
          {m.data.source === "schedule" && <Clock className="inline size-3 mr-1" />}
          {m.data.user} · {timeAgo(m.created_at)}
        </div>
        {atts.length > 0 && (
          <div className="flex flex-wrap justify-end gap-1.5">
            {atts.map((a) =>
              a.mime.startsWith("image/") ? (
                <a key={a.id} href={`/api/uploads/${a.id}`} target="_blank" rel="noreferrer">
                  <img src={`/api/uploads/${a.id}`} className="h-24 rounded-lg border border-border object-cover" alt={a.filename} />
                </a>
              ) : (
                <a key={a.id} href={`/api/uploads/${a.id}`} target="_blank" rel="noreferrer" className="flex items-center gap-1 rounded-lg border border-border px-2 py-1 text-xs">
                  <FileText className="size-3.5" /> {a.filename}
                </a>
              ),
            )}
          </div>
        )}
        {m.data.text && <div className="rounded-2xl rounded-tr-sm bg-accent text-accent-fg px-4 py-2.5 whitespace-pre-wrap break-words text-[15px]">{m.data.text}</div>}
      </div>
    </div>
  );
}

function Thinking({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="text-sm">
      <button onClick={() => setOpen(!open)} className="flex items-center gap-1.5 text-muted hover:text-text">
        <ChevronRight className={cx("size-3.5 transition", open && "rotate-90")} />
        <Brain className="size-3.5" /> {live ? "Thinking…" : "Thought"}
      </button>
      {open && <div className="mt-1 pl-5 text-muted whitespace-pre-wrap text-[13px] border-l-2 border-border ml-1.5">{text}</div>}
    </div>
  );
}

const MsgView = memo(function MsgView({ m }: { m: Msg }) {
  switch (m.kind) {
    case "user":
      return <UserBubble m={m} />;
    case "assistant":
      return <Markdown text={m.data.text} />;
    case "thinking":
      return <Thinking text={m.data.text} />;
    case "error":
      return <div className="rounded-xl border border-danger/40 bg-danger-soft text-danger text-sm px-3.5 py-2.5 whitespace-pre-wrap break-words">{m.data.text}</div>;
    case "system":
      return <div className="text-center text-xs text-muted">{m.data.text}</div>;
    case "plan":
      return (
        <div className="rounded-xl border border-border bg-panel p-3">
          <div className="text-xs text-muted mb-1">Plan</div>
          <Markdown text={m.data.text ?? ""} />
        </div>
      );
    case "todo":
      return (
        <div className="rounded-xl border border-border bg-panel p-3">
          {m.data.explanation && <div className="text-sm mb-2">{m.data.explanation}</div>}
          <Todos todos={m.data.plan ?? []} />
        </div>
      );
    default:
      return null;
  }
});

function SettingsModal({ chat, open, onClose }: { chat: ChatT; open: boolean; onClose: () => void }) {
  const [v, setV] = useState({ tool: chat.tool, model: chat.model, effort: chat.effort, permission_mode: chat.permission_mode });
  const [title, setTitle] = useState(chat.title);
  const [autoDeploy, setAutoDeploy] = useState(chat.auto_deploy);
  const { busy, run } = useAction();
  const qc = useQueryClient();
  useEffect(() => {
    if (open) {
      setV({ tool: chat.tool, model: chat.model, effort: chat.effort, permission_mode: chat.permission_mode });
      setTitle(chat.title);
      setAutoDeploy(chat.auto_deploy);
    }
  }, [open]);
  const save = async () => {
    const r = await run(() => api.put(`/api/chats/${chat.id}`, { ...v, title, auto_deploy: autoDeploy }), "Saved");
    if (r) {
      qc.invalidateQueries({ queryKey: ["chat", chat.id] });
      onClose();
    }
  };
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Chat settings"
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
      <label className="block space-y-1.5">
        <span className="text-[13px] font-medium">Title</span>
        <input className="w-full h-10 rounded-lg border border-border bg-panel px-3 text-sm" value={title} onChange={(e) => setTitle(e.target.value)} />
      </label>
      <AgentPicker v={v} set={(c) => setV({ ...c, tool: chat.tool })} lockTool readOnly={chat.read_only} />
      <p className="text-xs text-muted">
        Changes apply to the next prompt; the conversation continues where it left off.
        {chat.read_only && " This chat is read-only: the agent can read the project but not change it."}
      </p>
      {chat.project?.deployable && !chat.read_only && <Toggle checked={autoDeploy} onChange={setAutoDeploy} label="Deploy automatically when a prompt finishes" />}
    </Modal>
  );
}

export default function Chat() {
  const id = Number(useParams().id);
  const chatQ = useQuery({ queryKey: ["chat", id], queryFn: () => api.get<ChatT>(`/api/chats/${id}`) });
  const msgsQ = useQuery({ queryKey: ["messages", id], queryFn: () => api.get<MessagesData>(`/api/chats/${id}/messages`), staleTime: Infinity });
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [settings, setSettings] = useState(false);
  const [menu, setMenu] = useState(false);
  const toast = useToast();
  const ask = useConfirm();
  const askWithOption = useConfirmWithOption();
  const qc = useQueryClient();
  const nav = useNavigate();
  const { run } = useAction();
  const progress = useAgentProgress(id);
  const [panel, setPanelState] = useState(readPanelPref);
  const [focus, setFocus] = useState<string | null>(null);
  const setPanel = (open: boolean) => {
    setPanelState(open);
    try {
      localStorage.setItem(PANEL_KEY, open ? "1" : "0");
    } catch {
      /* private mode */
    }
  };
  const stopAgent = useStopAgent(id);

  const items = useMemo(() => buildItems(msgsQ.data?.messages ?? [], msgsQ.data?.requests ?? []), [msgsQ.data]);
  const chatActive = !!chatQ.data && ["running", "waiting", "queued"].includes(chatQ.data.status);
  const agents = useMemo(() => buildAgents(msgsQ.data?.messages ?? [], chatActive), [msgsQ.data, chatActive]);
  const agentByRef = useMemo(() => new Map<string, SubAgent>(agents.map((a) => [a.ref, a])), [agents]);
  const runningAgents = agents.filter((a) => a.status === "running").length;
  const bgRaw = useMemo(() => buildBackground(msgsQ.data?.messages ?? []), [msgsQ.data]);
  const bgHasRunning = bgRaw.some((t) => t.status === "running");
  const liveQ = useQuery({
    queryKey: ["bg-live", id, bgRaw.length],
    queryFn: () => api.get<{ alive: boolean; running: string[] }>(`/api/chats/${id}/background`),
    enabled: bgHasRunning,
    refetchInterval: bgHasRunning ? 5000 : false,
  });
  const bg = useMemo(() => withLiveStatus(bgRaw, bgHasRunning ? liveQ.data : undefined, liveQ.dataUpdatedAt), [bgRaw, bgHasRunning, liveQ.data, liveQ.dataUpdatedAt]);
  const bgByRef = useMemo(() => new Map<string, BgTask>(bg.filter((t) => t.ref).map((t) => [t.ref, t])), [bg]);
  const runningBg = bg.filter((t) => t.status === "running").length;
  const [panelTab, setPanelTab] = useState<PanelTab>("agents");
  const [bgFocus, setBgFocus] = useState<string | null>(null);
  const [fileFocus, setFileFocus] = useState<string | null>(null);
  const changes = useChanges(id, !!chatQ.data);
  const changedFiles = changes.data?.totals.files ?? 0;
  const bgActions = useBackgroundActions(id);
  const openBg = (taskId: string) => {
    setBgFocus(taskId);
    setPanelTab("background");
    setPanel(true);
  };
  const openAgent = (ref: string) => {
    setFocus(ref);
    setPanelTab("agents");
    setPanel(true);
  };

  const stickToBottom = useCallback(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, []);
  useLayoutEffect(stickToBottom, [items, stickToBottom]);

  const gate = queryGate(chatQ);
  if (gate) return gate;
  const chat = chatQ.data!;

  const running = chat.status === "running" || chat.status === "waiting";
  const queued = (chat.active_runs ?? []).filter((r) => r.status === "queued");
  const current = (chat.active_runs ?? []).find((r) => r.status === "running");

  const send = async (prompt: string, attachments: Attachment[]) => {
    try {
      stick.current = true;
      await api.post(`/api/chats/${id}/messages`, { prompt, attachments: attachments.map((a) => a.id) });
      qc.invalidateQueries({ queryKey: ["chat", id] });
      return true;
    } catch (e: any) {
      toast(e.message, "danger");
      return false;
    }
  };

  const loadOlder = async () => {
    const first = msgsQ.data?.messages[0]?.id;
    if (!first) return;
    const older = await api.get<MessagesData>(`/api/chats/${id}/messages?before=${first}`);
    stick.current = false;
    qc.setQueryData<MessagesData>(["messages", id], (d) => (d ? { ...d, messages: [...older.messages, ...d.messages], has_more: older.has_more } : d));
  };

  return (
    <div className="flex h-full min-h-0">
    <div className="flex flex-col flex-1 min-w-0 h-full min-h-0">
      <div className="flex items-center gap-2 px-2 sm:px-4 h-14 border-b border-border bg-panel shrink-0">
        <Link to={`/projects/${chat.project_id}`} className="p-2 text-muted hover:text-text" aria-label="Back">
          <ArrowLeft className="size-5" />
        </Link>
        <StatusDot status={chat.status} />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium truncate flex items-center gap-1.5">
            <span className="truncate">{chat.title}</span>
            {chat.read_only && (
              <Badge tone="accent" className="shrink-0">
                <Eye className="inline size-3 mr-0.5 -mt-px" />
                Read-only
              </Badge>
            )}
          </div>
          <div className="text-[11px] text-muted truncate">
            {chat.project?.name} · {toolName(chat.tool)}
            {chat.model ? ` · ${chat.model}` : ""}
            {chat.effort ? ` · ${chat.effort}` : ""} · {chat.read_only ? "read-only" : modeLabel(chat.permission_mode)}
            {chat.worktree_branch ? ` · ${chat.worktree_branch}` : ""}
          </div>
        </div>
        <div className="hidden sm:flex items-center gap-2 text-[11px] text-muted mr-1">
          {chat.cost_usd > 0 && <span>${chat.cost_usd.toFixed(2)}</span>}
          <span>{tokens(chat.input_tokens + chat.output_tokens)} tok</span>
        </div>
        <ContextMeter chatId={id} />
        <button
          title="Sub-agents, background tasks and changed files"
          aria-label="Sub-agents, background tasks and changed files"
          onClick={() => setPanel(!panel)}
          className={cx(
            "relative inline-flex h-9 items-center gap-1 rounded-lg px-2 text-muted hover:bg-panel-2 hover:text-text transition",
            panel && "bg-panel-2 text-text",
          )}
        >
          {agents.length > 0 ? <Bot className="size-4" /> : bg.length > 0 ? <SquareTerminal className="size-4" /> : <PanelRight className="size-4" />}
          {agents.length + bg.length > 0 && (
            <span className="text-xs tabular-nums">
              {runningAgents + runningBg > 0 ? `${runningAgents + runningBg}/${agents.length + bg.length}` : agents.length + bg.length}
            </span>
          )}
          {runningAgents + runningBg > 0 && <span className="absolute top-1.5 right-1 size-1.5 rounded-full bg-accent pulse-dot" />}
        </button>
        <IconButton title="Chat settings" onClick={() => setSettings(true)}>
          <SlidersHorizontal className="size-4" />
        </IconButton>
        <div className="relative">
          <IconButton title="More" onClick={() => setMenu(!menu)}>
            <MoreVertical className="size-4" />
          </IconButton>
          {menu && (
            <div className="absolute right-0 top-10 z-30 w-56 rounded-xl border border-border bg-panel shadow-xl py-1" onMouseLeave={() => setMenu(false)}>
              {chat.project?.deployable && (
                <MenuBtn
                  icon={<Rocket className="size-4" />}
                  onClick={async () => {
                    setMenu(false);
                    const ok = await ask({
                      title: "Deploy project",
                      message: `Deploy ${chat.project?.name ?? "this project"} now?`,
                      confirmLabel: "Deploy",
                    });
                    if (ok) await run(() => api.post(`/api/projects/${chat.project_id}/deploys`), "Deploy started");
                  }}
                >
                  Deploy project
                </MenuBtn>
              )}
              {chat.worktree_branch && (
                <MenuBtn
                  icon={<GitMerge className="size-4" />}
                  onClick={async () => {
                    setMenu(false);
                    const r = await askWithOption({
                      title: "Merge branch",
                      message: `Merge ${chat.worktree_branch} into the project?`,
                      confirmLabel: "Merge",
                      checkbox: { label: "Also remove the worktree and archive the chat" },
                    });
                    if (r.ok) await run(() => api.post(`/api/chats/${id}/merge`, { remove: r.checked }), "Merged");
                  }}
                >
                  Merge branch into project
                </MenuBtn>
              )}
              <MenuBtn icon={<Pencil className="size-4" />} onClick={() => (setMenu(false), setSettings(true))}>
                Rename / settings
              </MenuBtn>
              <MenuBtn
                icon={<Archive className="size-4" />}
                onClick={async () => {
                  setMenu(false);
                  await run(() => api.put(`/api/chats/${id}`, { archived: !chat.archived }), chat.archived ? "Unarchived" : "Archived");
                }}
              >
                {chat.archived ? "Unarchive" : "Archive"}
              </MenuBtn>
              <MenuBtn
                danger
                icon={<Trash2 className="size-4" />}
                onClick={async () => {
                  setMenu(false);
                  if (!(await ask({ title: "Delete chat", message: "Delete this chat and its history?", confirmLabel: "Delete", danger: true }))) return;
                  const r = await run(() => api.del(`/api/chats/${id}`));
                  if (r) nav(`/projects/${chat.project_id}`);
                }}
              >
                Delete chat
              </MenuBtn>
            </div>
          )}
        </div>
      </div>

      {chat.project?.deployable && <DeployBanner projectId={chat.project_id} enabled />}

      <div
        ref={scroller}
        className="flex-1 overflow-y-auto scroll-thin"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 120;
        }}
      >
        <div className="mx-auto max-w-3xl px-3 sm:px-5 py-5 space-y-4">
          {msgsQ.data?.has_more && (
            <div className="text-center">
              <Button size="sm" variant="ghost" onClick={loadOlder}>
                Load older messages
              </Button>
            </div>
          )}
          {msgsQ.data === undefined && (msgsQ.isError ? <ErrorState error={msgsQ.error} onRetry={msgsQ.refetch} retrying={msgsQ.isFetching} /> : <MessagesSkeleton />)}
          {msgsQ.data !== undefined && msgsQ.isError && <StaleNotice q={msgsQ} />}
          {items.map((it) =>
            it.type === "req" ? (
              <RequestCard key={`r${it.r.id}`} r={it.r} />
            ) : it.type === "tool" ? (
              (() => {
                const agent = isAgentTool(it.use.data?.name) ? agentByRef.get(it.use.data.id) : undefined;
                const task = isBackgroundCall(it.use) ? bgByRef.get(it.use.data.id) : undefined;
                const badge = agent ? <AgentStatusBadge status={agent.status} /> : task ? <AgentStatusBadge status={task.status} /> : undefined;
                return (
                  <ToolCard key={it.use.id} use={it.use} result={it.result} badge={badge}>
                    {task && (
                      <div className="flex flex-wrap gap-2">
                        <Button size="sm" variant="outline" icon={<PanelRight className="size-3.5" />} onClick={() => openBg(task.taskId)}>
                          {task.status === "running" ? "Watch output" : "Show output"}
                        </Button>
                        {task.status === "running" && (
                          <>
                            <Button size="sm" variant="outline" icon={<Square className="size-3.5" />} onClick={() => bgActions.stop(task)}>
                              Stop
                            </Button>
                            <Button size="sm" variant="outline" className="text-danger" icon={<Skull className="size-3.5" />} onClick={() => bgActions.kill(task)}>
                              Kill
                            </Button>
                          </>
                        )}
                      </div>
                    )}
                    {agent && (
                      <div className="flex flex-wrap gap-2">
                        <Button size="sm" variant="outline" icon={<PanelRight className="size-3.5" />} onClick={() => openAgent(agent.ref)}>
                          Open in sub-agents panel
                        </Button>
                        {agent.status === "running" && (
                          <Button size="sm" variant="outline" className="text-danger" icon={<Square className="size-3.5" />} onClick={() => stopAgent(agent)}>
                            Stop
                          </Button>
                        )}
                      </div>
                    )}
                    {it.children.length > 0 && <div className="space-y-2 border-l-2 border-border pl-3">{renderSteps(it.children)}</div>}
                  </ToolCard>
                );
              })()
            ) : (
              <MsgView key={it.m.id} m={it.m} />
            ),
          )}
          {running && <LiveTail chatId={id} waiting={chat.status === "waiting"} background={chat.waiting_background} startedAt={current?.started_at} onGrow={stickToBottom} />}
          {chat.status === "queued" && !running && (
            <div className="flex items-center gap-2 text-xs text-muted">
              <Clock className="size-3.5" /> Queued — waiting for a free slot
            </div>
          )}
        </div>
      </div>

      <div className="shrink-0 border-t border-border bg-bg">
        <div className="mx-auto max-w-3xl px-3 sm:px-5 py-3 space-y-2 safe-bottom">
          {queued.length > 0 && (
            <div className="space-y-1">
              {queued.map((r) => (
                <div key={r.id} className="flex items-center gap-2 rounded-lg bg-panel-2 px-3 py-1.5 text-xs">
                  <Clock className="size-3.5 text-muted shrink-0" />
                  <span className="truncate flex-1">{r.prompt}</span>
                  {r.source === "schedule" && <Badge>scheduled</Badge>}
                  <button className="text-muted hover:text-danger" title="Remove from queue" onClick={() => run(() => api.post(`/api/runs/${r.id}/cancel`))}>
                    <X className="size-3.5" />
                  </button>
                </div>
              ))}
            </div>
          )}
          <Composer
            key={id}
            draftKey={`draft:chat:${id}`}
            onSend={send}
            running={running}
            onStop={() => run(() => api.post(`/api/chats/${id}/stop`), "Stopping…")}
            placeholder={
              chat.waiting_background
                ? "Send a follow-up (this stops waiting for the background tasks)…"
                : running
                  ? "Send a follow-up (it waits in the queue)…"
                  : chat.read_only
                    ? `Ask ${toolName(chat.tool)} about the code (read-only)…`
                    : `Message ${toolName(chat.tool)}…`
            }
          />
        </div>
      </div>
      <SettingsModal chat={chat} open={settings} onClose={() => setSettings(false)} />
    </div>
      {panel && (
        <>
          {/* Phones: a drawer over the chat. Desktop: a column beside it. */}
          <div className="fixed inset-0 z-40 bg-black/40 lg:hidden" onClick={() => setPanel(false)} />
          <aside className="fixed inset-y-0 right-0 z-50 w-[min(420px,92vw)] border-l border-border shadow-xl lg:static lg:z-auto lg:w-[380px] lg:shadow-none lg:shrink-0">
            <AgentsPanel
              chatId={id}
              agents={agents}
              progress={progress}
              focus={focus}
              onFocus={setFocus}
              onClose={() => setPanel(false)}
              renderSteps={renderSteps}
              tab={panelTab}
              onTab={setPanelTab}
              bgCount={bg.length}
              bgRunning={runningBg}
              background={<BackgroundList chatId={id} tasks={bg} focus={bgFocus} onFocus={setBgFocus} isCodex={chat.tool === "codex"} />}
              filesCount={changedFiles}
              files={<FilesList chatId={id} focus={fileFocus} onFocus={setFileFocus} />}
            />
          </aside>
        </>
      )}
    </div>
  );
}

function MenuBtn({ icon, children, onClick, danger }: { icon: React.ReactNode; children: React.ReactNode; onClick: () => void; danger?: boolean }) {
  return (
    <button onClick={onClick} className={cx("w-full flex items-center gap-2.5 px-3 py-2 text-sm hover:bg-panel-2 text-left", danger && "text-danger")}>
      {icon}
      {children}
    </button>
  );
}
