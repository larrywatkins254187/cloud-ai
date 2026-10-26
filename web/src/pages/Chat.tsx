import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  ArrowLeft,
  Brain,
  ChevronRight,
  Clock,
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
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { AgentPicker } from "../components/AgentPicker";
import { Composer } from "../components/Composer";
import { Markdown } from "../components/Markdown";
import { RequestCard } from "../components/RequestCard";
import { Todos, ToolCard } from "../components/ToolCard";
import { DeployBanner } from "../components/DeployStatus";
import { Badge, Button, cx, IconButton, Modal, Spinner, StatusDot, Toggle, useAction, useConfirm, useConfirmWithOption, useToast } from "../components/ui";
import { api } from "../lib/api";
import { duration, modeLabel, timeAgo, tokens, toolName } from "../lib/format";
import { useStream, type MessagesData } from "../lib/live";
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
    if (m.kind === "tool_result") continue;
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

function MsgView({ m }: { m: Msg }) {
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
}

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
      <AgentPicker v={v} set={(c) => setV({ ...c, tool: chat.tool })} lockTool />
      <p className="text-xs text-muted">Changes apply to the next prompt; the conversation continues where it left off.</p>
      {chat.project?.deployable && <Toggle checked={autoDeploy} onChange={setAutoDeploy} label="Deploy automatically when a prompt finishes" />}
    </Modal>
  );
}

export default function Chat() {
  const id = Number(useParams().id);
  const chatQ = useQuery({ queryKey: ["chat", id], queryFn: () => api.get<ChatT>(`/api/chats/${id}`) });
  const msgsQ = useQuery({ queryKey: ["messages", id], queryFn: () => api.get<MessagesData>(`/api/chats/${id}/messages`), staleTime: Infinity });
  const stream = useStream(id);
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

  const items = useMemo(() => buildItems(msgsQ.data?.messages ?? [], msgsQ.data?.requests ?? []), [msgsQ.data]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items, stream]);

  const chat = chatQ.data;
  if (chatQ.isLoading) return <Spinner page />;
  if (!chat) return <div className="p-6 text-danger text-sm">{(chatQ.error as Error)?.message ?? "Chat not found"}</div>;

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
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-2 px-2 sm:px-4 h-14 border-b border-border bg-panel shrink-0">
        <Link to={`/projects/${chat.project_id}`} className="p-2 text-muted hover:text-text" aria-label="Back">
          <ArrowLeft className="size-5" />
        </Link>
        <StatusDot status={chat.status} />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium truncate">{chat.title}</div>
          <div className="text-[11px] text-muted truncate">
            {chat.project?.name} · {toolName(chat.tool)}
            {chat.model ? ` · ${chat.model}` : ""}
            {chat.effort ? ` · ${chat.effort}` : ""} · {modeLabel(chat.permission_mode)}
            {chat.worktree_branch ? ` · ${chat.worktree_branch}` : ""}
          </div>
        </div>
        <div className="hidden sm:flex items-center gap-2 text-[11px] text-muted mr-1">
          {chat.cost_usd > 0 && <span>${chat.cost_usd.toFixed(2)}</span>}
          <span>{tokens(chat.input_tokens + chat.output_tokens)} tok</span>
        </div>
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
          {msgsQ.isLoading && <Spinner />}
          {items.map((it) =>
            it.type === "req" ? (
              <RequestCard key={`r${it.r.id}`} r={it.r} />
            ) : it.type === "tool" ? (
              <ToolCard key={it.use.id} use={it.use} result={it.result}>
                {it.children.length > 0 && (
                  <div className="space-y-2 border-l-2 border-border pl-3">
                    {buildItems(it.children.map((c) => ({ ...c, data: { ...c.data, parent: null } })), []).map((c) =>
                      c.type === "tool" ? <ToolCard key={c.use.id} use={c.use} result={c.result} /> : c.type === "msg" ? <MsgView key={c.m.id} m={c.m} /> : null,
                    )}
                  </div>
                )}
              </ToolCard>
            ) : (
              <MsgView key={it.m.id} m={it.m} />
            ),
          )}
          {running && (
            <div className="space-y-2">
              {stream?.thinking && <Thinking text={stream.thinking} live />}
              {stream?.text && <Markdown text={stream.text + " ▍"} />}
              <div className="flex items-center gap-2 text-xs text-muted">
                <Loader2 className="size-3.5 animate-spin" />
                {chat.status === "waiting" ? "Waiting for your answer…" : stream?.activity || "Working…"}
                {current?.started_at && <span>· {duration(current.started_at)}</span>}
              </div>
            </div>
          )}
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
            placeholder={running ? "Send a follow-up (it waits in the queue)…" : `Message ${toolName(chat.tool)}…`}
          />
        </div>
      </div>
      <SettingsModal chat={chat} open={settings} onClose={() => setSettings(false)} />
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
