import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  ArrowRight,
  Camera,
  Crosshair,
  Laptop,
  Maximize2,
  Minimize2,
  MousePointer2,
  Play,
  RotateCw,
  Send,
  Smartphone,
  Square,
  Tablet,
  Terminal as TerminalIcon,
  Trash2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { AgentPicker, type AgentConfig } from "../../components/AgentPicker";
import { Badge, Button, Card, cx, IconButton, Input, Modal, Select, Textarea, Toggle, useAction, useToast } from "../../components/ui";
import { api } from "../../lib/api";
import type { Attachment, Chat, Project } from "../../lib/types";

type El = {
  selector: string;
  tag: string;
  text: string;
  html: string;
  rect: { x: number; y: number; width: number; height: number };
  styles: Record<string, string>;
  url: string;
  /** Page scroll when picked (rect is relative to the viewport at that moment). */
  scroll?: Scroll;
  /** Inside a fixed/sticky element: does not move when the page scrolls. */
  pinned?: boolean;
};
type Scroll = { x: number; y: number };
type Note = { id: number; el: El; comment: string; shot?: Attachment };
type Viewport = { width: number; height: number; mobile: boolean };

const VIEWPORTS: { id: string; label: string; icon: any; v: Viewport }[] = [
  { id: "desktop", label: "Desktop", icon: Laptop, v: { width: 1280, height: 800, mobile: false } },
  { id: "tablet", label: "Tablet", icon: Tablet, v: { width: 820, height: 1180, mobile: true } },
  { id: "mobile", label: "Mobile", icon: Smartphone, v: { width: 390, height: 844, mobile: true } },
];

function DevServer({ project }: { project: Project }) {
  const q = useQuery({ queryKey: ["dev", project.id], queryFn: () => api.get<any>(`/api/projects/${project.id}/dev`), refetchInterval: 4000 });
  const [log, setLog] = useState(false);
  const { busy, run } = useAction();
  const qc = useQueryClient();
  const d = q.data;
  if (!project.dev_command)
    return <div className="text-xs text-muted">No dev command set — add one in the Settings tab to start a dev server from here.</div>;
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="font-medium">Dev server</span>
        {d === undefined ? (
          <Badge tone={q.isError ? "danger" : "neutral"}>{q.isError ? "status unavailable" : "checking…"}</Badge>
        ) : d.running ? (
          <Badge tone="ok">running · port {d.port ?? "?"}</Badge>
        ) : (
          <Badge>{d.exit ? `stopped (${d.exit})` : "stopped"}</Badge>
        )}
        <code className="text-xs text-muted truncate max-w-xs">{project.dev_command}</code>
        <div className="flex-1" />
        {d?.running ? (
          <Button size="sm" variant="outline" icon={<Square className="size-3.5" />} loading={busy} onClick={async () => (await run(() => api.post(`/api/projects/${project.id}/dev/stop`)), qc.invalidateQueries({ queryKey: ["dev", project.id] }))}>
            Stop
          </Button>
        ) : (
          <Button size="sm" variant="primary" icon={<Play className="size-3.5" />} loading={busy} disabled={d === undefined} onClick={async () => (await run(() => api.post(`/api/projects/${project.id}/dev/start`)), qc.invalidateQueries({ queryKey: ["dev", project.id] }))}>
            Start
          </Button>
        )}
        <Button size="sm" variant="ghost" icon={<TerminalIcon className="size-3.5" />} onClick={() => setLog(!log)}>
          Log
        </Button>
      </div>
      {log && <pre className="bg-code border border-border rounded-lg p-2 text-[11px] font-mono max-h-48 overflow-auto scroll-thin whitespace-pre-wrap">{d?.log || "(empty)"}</pre>}
    </div>
  );
}

/** WebSocket to the server-side Chromium tab of this project. */
function useBrowser(projectId: number, onFrame: (b: Blob) => void) {
  const ws = useRef<WebSocket | null>(null);
  const pending = useRef(new Map<number, (v: any) => void>());
  const seq = useRef(1);
  const [state, setState] = useState<{ url: string; title: string; connected: boolean; error?: string; viewport: Viewport; scroll: Scroll }>({
    url: "",
    title: "",
    connected: false,
    viewport: VIEWPORTS[0].v,
    scroll: { x: 0, y: 0 },
  });
  const [consoleLog, setConsole] = useState<any[]>([]);
  const frameCb = useRef(onFrame);
  frameCb.current = onFrame;

  useEffect(() => {
    let closed = false;
    let retry: ReturnType<typeof setTimeout>;
    const connect = () => {
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const s = new WebSocket(`${proto}//${location.host}/api/projects/${projectId}/browser/ws`);
      s.binaryType = "blob";
      s.onmessage = (m) => {
        if (typeof m.data !== "string") return frameCb.current(m.data);
        const v = JSON.parse(m.data);
        if (v.req && pending.current.has(v.req)) {
          pending.current.get(v.req)!(v);
          pending.current.delete(v.req);
        }
        switch (v.type) {
          case "hello":
            setState((x) => ({
              ...x,
              connected: true,
              url: v.url,
              title: v.title,
              error: undefined,
              viewport: { width: v.width, height: v.height, mobile: v.mobile },
              scroll: v.scroll ?? { x: 0, y: 0 },
            }));
            s.send(JSON.stringify({ type: "console" }));
            break;
          case "console_all":
            setConsole(v.entries ?? []);
            break;
          case "url":
            setState((x) => ({ ...x, url: v.url }));
            break;
          case "scroll":
            setState((x) => ({ ...x, scroll: { x: v.x, y: v.y } }));
            break;
          case "title":
            setState((x) => ({ ...x, title: v.title }));
            break;
          case "console":
            setConsole((c) => [...c.slice(-199), v.entry]);
            break;
          case "fatal":
            closed = true;
            setState((x) => ({ ...x, connected: false, error: v.message }));
            break;
          case "error":
            setState((x) => ({ ...x, error: v.message }));
            break;
        }
      };
      s.onclose = () => {
        setState((x) => ({ ...x, connected: false }));
        if (!closed) retry = setTimeout(connect, 2000);
      };
      ws.current = s;
    };
    connect();
    return () => {
      closed = true;
      clearTimeout(retry);
      ws.current?.close();
    };
  }, [projectId]);

  const send = useCallback((v: any) => {
    if (ws.current?.readyState === 1) ws.current.send(JSON.stringify(v));
  }, []);
  const request = useCallback(
    (v: any) =>
      new Promise<any>((resolve) => {
        const req = seq.current++;
        pending.current.set(req, resolve);
        send({ ...v, req });
        setTimeout(() => {
          if (pending.current.delete(req)) resolve({ type: "error", message: "timed out" });
        }, 20000);
      }),
    [send],
  );
  return { state, setState, send, request, consoleLog, setConsole };
}

function buildPrompt(url: string, vp: Viewport, notes: Note[], consoleErrors: string[], extra: string) {
  const lines: string[] = [];
  lines.push(`Page: ${url} (viewport ${vp.width}×${vp.height}${vp.mobile ? ", mobile" : ""})`);
  if (notes.length) {
    lines.push("", "Annotated elements (screenshots are attached in the same order):");
    notes.forEach((n, i) => {
      lines.push("", `${i + 1}. \`${n.el.selector}\` <${n.el.tag}>${n.el.text ? ` — text: "${n.el.text.slice(0, 160).replace(/\s+/g, " ")}"` : ""}`);
      if (n.comment.trim()) lines.push(`   Requested change: ${n.comment.trim()}`);
      lines.push(`   Styles: ${Object.entries(n.el.styles).map(([k, v]) => `${k}: ${v}`).join("; ")}`);
      lines.push("   HTML:", "```html", n.el.html.slice(0, 1500), "```");
    });
  }
  if (consoleErrors.length) {
    lines.push("", "Recent browser console errors:", ...consoleErrors.slice(-15).map((e) => `- ${e}`));
  }
  if (extra.trim()) lines.push("", extra.trim());
  return lines.join("\n");
}

function SendDialog({
  project,
  open,
  onClose,
  notes,
  url,
  viewport,
  consoleLog,
  takeShot,
  onSent,
}: {
  project: Project;
  open: boolean;
  onClose: () => void;
  notes: Note[];
  url: string;
  viewport: Viewport;
  consoleLog: any[];
  takeShot: () => Promise<Attachment | undefined>;
  onSent: () => void;
}) {
  const chats = useQuery({ queryKey: ["chats", project.id, false], queryFn: () => api.get<Chat[]>(`/api/projects/${project.id}/chats`), enabled: open });
  const [target, setTarget] = useState<number | "new">("new");
  const [cfg, setCfg] = useState<AgentConfig>({ tool: "claude", model: "", effort: "", permission_mode: "bypass" });
  const [extra, setExtra] = useState("");
  const [withConsole, setWithConsole] = useState(true);
  const [withPage, setWithPage] = useState(notes.length === 0);
  const { busy, run } = useAction();
  const nav = useNavigate();
  const errors = consoleLog.filter((c) => c.level === "error" || c.level === "assert").map((c) => c.text);
  const send = async () => {
    const atts: number[] = notes.map((n) => n.shot?.id).filter(Boolean) as number[];
    if (withPage) {
      const s = await takeShot();
      if (s) atts.push(s.id);
    }
    const prompt = buildPrompt(url, viewport, notes, withConsole ? errors : [], extra);
    const r = await run(async () => {
      if (target === "new") {
        const c = await api.post<Chat>(`/api/projects/${project.id}/chats`, { ...cfg, prompt, attachments: atts });
        return c.id;
      }
      await api.post(`/api/chats/${target}/messages`, { prompt, attachments: atts });
      return target;
    }, "Sent to the AI");
    if (r) {
      onSent();
      onClose();
      nav(`/chats/${r}`);
    }
  };
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Send to AI"
      wide
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" icon={<Send className="size-4" />} loading={busy} onClick={send}>
            Send
          </Button>
        </>
      }
    >
      <Select value={target} onChange={(e) => setTarget(e.target.value === "new" ? "new" : Number(e.target.value))}>
        <option value="new">New chat</option>
        {chats.data?.map((c) => (
          <option key={c.id} value={c.id}>
            Continue: {c.title}
          </option>
        ))}
      </Select>
      {target === "new" && <AgentPicker v={cfg} set={setCfg} />}
      <Textarea value={extra} onChange={(e) => setExtra(e.target.value)} placeholder="Anything else the AI should know or do…" />
      <div className="flex flex-wrap gap-x-6">
        <Toggle checked={withPage} onChange={setWithPage} label="Attach a screenshot of the whole view" />
        {errors.length > 0 && <Toggle checked={withConsole} onChange={setWithConsole} label={`Include ${errors.length} console errors`} />}
      </div>
      <details className="text-xs">
        <summary className="cursor-pointer text-muted">Preview prompt</summary>
        <pre className="mt-2 whitespace-pre-wrap bg-code rounded-lg p-2 max-h-64 overflow-auto scroll-thin">{buildPrompt(url, viewport, notes, withConsole ? errors : [], extra)}</pre>
      </details>
    </Modal>
  );
}

export default function PreviewTab({ project }: { project: Project }) {
  const img = useRef<HTMLImageElement>(null);
  const lastUrl = useRef<string | null>(null);
  const onFrame = useCallback((b: Blob) => {
    const u = URL.createObjectURL(b);
    if (img.current) img.current.src = u;
    if (lastUrl.current) URL.revokeObjectURL(lastUrl.current);
    lastUrl.current = u;
  }, []);
  const { state, setState, send, request, consoleLog, setConsole } = useBrowser(project.id, onFrame);
  const [mode, setMode] = useState<"interact" | "select">("interact");
  const [address, setAddress] = useState("");
  const [hover, setHover] = useState<El | null>(null);
  const [full, setFull] = useState(false);
  const card = useRef<HTMLDivElement>(null);
  const area = useRef<HTMLDivElement>(null);
  const [areaSize, setAreaSize] = useState<{ w: number; h: number } | null>(null);
  const [notes, setNotes] = useState<Note[]>([]);
  const [sendOpen, setSendOpen] = useState(false);
  const [showConsole, setShowConsole] = useState(false);
  const [typeText, setTypeText] = useState("");
  const toast = useToast();
  const lastMove = useRef(0);
  const vp = state.viewport;

  useEffect(() => setAddress(state.url), [state.url]);

  // Fullscreen: real fullscreen where the browser allows it, otherwise the card
  // just covers the window. Either way everything keeps working inside it.
  const enterFull = () => {
    setFull(true);
    card.current?.requestFullscreen?.().catch(() => {});
  };
  const exitFull = () => {
    setFull(false);
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  };
  useEffect(() => {
    const onChange = () => !document.fullscreenElement && setFull(false);
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);
  useEffect(() => {
    if (!full) return;
    // Esc goes to the page while it has focus in "Use" mode.
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !(mode === "interact" && document.activeElement === img.current) && exitFull();
    window.addEventListener("keydown", onKey);
    document.body.style.overflow = "hidden";
    return () => {
      window.removeEventListener("keydown", onKey);
      document.body.style.overflow = "";
    };
  }, [full, mode]);
  // In fullscreen, fit the page view into the free space.
  useEffect(() => {
    if (!full || !area.current) return setAreaSize(null);
    const el = area.current;
    const ro = new ResizeObserver(() => setAreaSize({ w: el.clientWidth - 16, h: el.clientHeight - 16 }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [full]);
  const fitWidth = areaSize ? Math.max(200, Math.min(areaSize.w, (areaSize.h * vp.width) / vp.height)) : undefined;

  const toPage = (e: { clientX: number; clientY: number }) => {
    const r = img.current!.getBoundingClientRect();
    return { x: Math.round(((e.clientX - r.left) * vp.width) / r.width), y: Math.round(((e.clientY - r.top) * vp.height) / r.height) };
  };
  const scale = () => (img.current ? img.current.getBoundingClientRect().width / vp.width : 1);
  /** Where to draw an element's box now: its rect moved by how far the page scrolled since it was picked. */
  const boxStyle = (el: El) => {
    const k = scale();
    const dx = el.pinned || !el.scroll ? 0 : el.scroll.x - state.scroll.x;
    const dy = el.pinned || !el.scroll ? 0 : el.scroll.y - state.scroll.y;
    return { left: (el.rect.x + dx) * k, top: (el.rect.y + dy) * k, width: el.rect.width * k, height: el.rect.height * k };
  };

  const navigate = (url: string) => url.trim() && send({ type: "navigate", url: url.trim() });
  const setViewport = (v: Viewport) => {
    send({ type: "viewport", ...v });
    setState((s) => ({ ...s, viewport: v }));
    setHover(null);
  };

  const shot = async (rect?: El["rect"]) => {
    const r = await request({ type: "screenshot", rect });
    if (r.type === "error") {
      toast(r.message, "danger");
      return undefined;
    }
    return r.attachment as Attachment;
  };

  const pickAt = async (p: { x: number; y: number }) => {
    const r = await request({ type: "inspect", ...p });
    if (!r.element) return;
    const el: El = r.element;
    const s = await shot(el.rect);
    setNotes((n) => [...n, { id: Date.now(), el, comment: "", shot: s }]);
  };

  const mods = (e: React.MouseEvent | React.KeyboardEvent | React.WheelEvent) => (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);

  return (
    <div className="space-y-3">
      <Card className="p-3">
        <DevServer project={project} />
      </Card>
      <Card ref={card} className={cx("overflow-hidden", full && "fixed inset-0 z-50 rounded-none border-0 flex flex-col")}>
        <div className="flex flex-wrap items-center gap-1.5 p-2 border-b border-border">
          <IconButton title="Back" onClick={() => send({ type: "back" })}>
            <ArrowLeft className="size-4" />
          </IconButton>
          <IconButton title="Forward" onClick={() => send({ type: "forward" })}>
            <ArrowRight className="size-4" />
          </IconButton>
          <IconButton title="Reload" onClick={() => send({ type: "reload" })}>
            <RotateCw className="size-4" />
          </IconButton>
          <form
            className="flex-1 min-w-[180px]"
            onSubmit={(e) => {
              e.preventDefault();
              navigate(address);
            }}
          >
            <Input className="h-9 font-mono text-xs" value={address} onChange={(e) => setAddress(e.target.value)} placeholder="http://127.0.0.1:3000" />
          </form>
          {project.dev_port && (
            <Button size="sm" variant="outline" onClick={() => navigate(`http://127.0.0.1:${project.dev_port}`)}>
              Dev
            </Button>
          )}
          {project.prod_url && (
            <Button size="sm" variant="outline" onClick={() => navigate(project.prod_url)}>
              Production
            </Button>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-1.5 px-2 py-1.5 border-b border-border">
          <div className="flex rounded-lg border border-border overflow-hidden">
            {VIEWPORTS.map((v) => (
              <button
                key={v.id}
                title={`${v.label} ${v.v.width}×${v.v.height}`}
                onClick={() => setViewport(v.v)}
                className={cx("px-2.5 h-8", vp.width === v.v.width ? "bg-accent-soft text-accent" : "text-muted hover:bg-panel-2")}
              >
                <v.icon className="size-4" />
              </button>
            ))}
          </div>
          <div className="flex rounded-lg border border-border overflow-hidden">
            <button onClick={() => (setMode("interact"), setHover(null))} className={cx("flex items-center gap-1 px-2.5 h-8 text-xs", mode === "interact" ? "bg-accent-soft text-accent" : "text-muted")}>
              <MousePointer2 className="size-3.5" /> Use
            </button>
            <button onClick={() => setMode("select")} className={cx("flex items-center gap-1 px-2.5 h-8 text-xs", mode === "select" ? "bg-accent-soft text-accent" : "text-muted")}>
              <Crosshair className="size-3.5" /> Select element
            </button>
          </div>
          <Button size="sm" variant="ghost" icon={<Camera className="size-4" />} onClick={async () => {
            const s = await shot();
            if (s) setNotes((n) => [...n, { id: Date.now(), el: { selector: "(whole view)", tag: "page", text: "", html: "", rect: { x: 0, y: 0, width: vp.width, height: vp.height }, styles: {}, url: state.url }, comment: "", shot: s }]);
          }}>
            Screenshot
          </Button>
          <Button size="sm" variant="ghost" icon={<TerminalIcon className="size-4" />} onClick={() => setShowConsole(!showConsole)}>
            Console {consoleLog.filter((c) => c.level === "error").length > 0 && <Badge tone="danger">{consoleLog.filter((c) => c.level === "error").length}</Badge>}
          </Button>
          <div className="flex-1" />
          <span className="text-xs text-muted truncate max-w-[200px]">{state.connected ? state.title : "connecting…"}</span>
          <IconButton title={full ? "Exit fullscreen (Esc)" : "Fullscreen"} onClick={full ? exitFull : enterFull}>
            {full ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
          </IconButton>
        </div>
        {state.error && (
          <div className="flex items-center gap-2 bg-danger-soft text-danger text-xs px-3 py-2">
            <span className="flex-1">{state.error}</span>
            <button onClick={() => setState((s) => ({ ...s, error: undefined }))}>
              <X className="size-3.5" />
            </button>
          </div>
        )}
        <div className={cx("grid lg:grid-cols-[1fr_320px]", full && "flex-1 min-h-0 grid-rows-[1fr_auto] lg:grid-rows-1")}>
          <div ref={area} className={cx("bg-panel-2 p-2 flex justify-center", full ? "min-h-0 overflow-auto items-center" : "min-h-[300px]")}>
            <div
              className="relative w-full overflow-hidden rounded"
              style={{ maxWidth: full ? fitWidth : vp.mobile ? Math.min(vp.width, 430) : undefined }}
            >
              <img
                ref={img}
                alt=""
                draggable={false}
                tabIndex={0}
                className={cx("w-full h-auto bg-white shadow rounded outline-none select-none", mode === "select" ? "cursor-crosshair" : "cursor-default")}
                style={{ aspectRatio: `${vp.width} / ${vp.height}` }}
                onMouseMove={(e) => {
                  const now = Date.now();
                  if (now - lastMove.current < 40) return;
                  lastMove.current = now;
                  const p = toPage(e);
                  if (mode === "select") request({ type: "hover", ...p }).then((r) => setHover(r.element ?? null));
                  else send({ type: "mouse", event: "move", ...p, modifiers: mods(e) });
                }}
                onMouseDown={(e) => {
                  e.currentTarget.focus();
                  if (mode === "interact") send({ type: "mouse", event: "down", ...toPage(e), button: e.button === 2 ? "right" : "left", clickCount: e.detail || 1, modifiers: mods(e) });
                }}
                onMouseUp={(e) => {
                  if (mode === "interact") send({ type: "mouse", event: "up", ...toPage(e), button: e.button === 2 ? "right" : "left", clickCount: e.detail || 1, modifiers: mods(e) });
                  else pickAt(toPage(e));
                }}
                onWheel={(e) => send({ type: "mouse", event: "wheel", ...toPage(e), deltaX: e.deltaX, deltaY: e.deltaY })}
                onKeyDown={(e) => {
                  if (mode !== "interact") return;
                  e.preventDefault();
                  send({ type: "key", event: "down", key: e.key, code: e.code, modifiers: mods(e) });
                }}
                onKeyUp={(e) => {
                  if (mode !== "interact") return;
                  e.preventDefault();
                  send({ type: "key", event: "up", key: e.key, code: e.code, modifiers: mods(e) });
                }}
                onContextMenu={(e) => e.preventDefault()}
              />
              {mode === "select" && hover && <div className="absolute pointer-events-none border-2 border-accent bg-accent/15 rounded-sm" style={boxStyle(hover)} />}
              {notes.map((n, i) =>
                // Boxes only for elements on the page that is open now.
                n.el.tag === "page" || n.el.url !== state.url ? null : (
                  <div key={n.id} className="absolute pointer-events-none border-2 border-warn rounded-sm" style={boxStyle(n.el)}>
                    <span className="absolute top-0 left-0 size-5 rounded-br-md bg-warn text-black text-[11px] font-bold flex items-center justify-center">{i + 1}</span>
                  </div>
                ),
              )}
              {!state.connected && !state.error && <div className="absolute inset-0 flex items-center justify-center text-sm text-muted">Starting browser…</div>}
            </div>
          </div>
          <div className={cx("border-t lg:border-t-0 lg:border-l border-border flex flex-col", full ? "max-h-[35dvh] lg:max-h-none min-h-0" : "max-h-[80dvh]")}>
            <div className="px-3 py-2 border-b border-border flex items-center gap-2">
              <span className="text-sm font-medium">Annotations</span>
              <Badge>{notes.length}</Badge>
              <div className="flex-1" />
              {notes.length > 0 && (
                <button className="text-xs text-muted hover:text-danger" onClick={() => setNotes([])}>
                  Clear
                </button>
              )}
            </div>
            <div className="flex-1 overflow-y-auto scroll-thin p-2 space-y-2">
              {notes.length === 0 && (
                <div className="text-xs text-muted p-2 leading-relaxed">
                  Switch to <b>Select element</b> and click parts of the page. Each pick saves the element's selector, HTML, styles and a screenshot. Add a comment to each, then send everything to Claude or Codex.
                </div>
              )}
              {notes.map((n, i) => (
                <div key={n.id} className="rounded-lg border border-border p-2 space-y-1.5">
                  <div className="flex items-start gap-2">
                    <span className="size-5 shrink-0 rounded-full bg-warn text-black text-[11px] font-bold flex items-center justify-center">{i + 1}</span>
                    <code className="text-[11px] break-all flex-1">{n.el.selector}</code>
                    <button className="text-muted hover:text-danger" onClick={() => setNotes((x) => x.filter((y) => y.id !== n.id))}>
                      <Trash2 className="size-3.5" />
                    </button>
                  </div>
                  {n.shot && <img src={`/api/uploads/${n.shot.id}`} className="max-h-28 rounded border border-border" alt="" />}
                  <Textarea
                    className="min-h-14 text-xs"
                    value={n.comment}
                    placeholder="What should change here?"
                    onChange={(e) => setNotes((x) => x.map((y) => (y.id === n.id ? { ...y, comment: e.target.value } : y)))}
                  />
                </div>
              ))}
            </div>
            <div className="p-2 border-t border-border">
              <Button className="w-full" variant="primary" icon={<Send className="size-4" />} onClick={() => setSendOpen(true)}>
                Send to AI
              </Button>
            </div>
          </div>
        </div>
        <form
          className="flex gap-2 p-2 border-t border-border lg:hidden"
          onSubmit={(e) => {
            e.preventDefault();
            if (typeText) send({ type: "text", text: typeText });
            setTypeText("");
          }}
        >
          <Input className="h-9 text-sm" value={typeText} onChange={(e) => setTypeText(e.target.value)} placeholder="Type into the focused field…" />
          <Button size="sm" variant="outline" type="submit">
            Type
          </Button>
          <Button size="sm" variant="outline" type="button" onClick={() => send({ type: "key", event: "down", key: "Enter", code: "Enter" })}>
            ⏎
          </Button>
        </form>
        {showConsole && (
          <div className="border-t border-border">
            <div className="flex items-center px-3 py-1.5 text-xs text-muted">
              Console
              <div className="flex-1" />
              <button onClick={() => (send({ type: "clear_console" }), setConsole([]))}>Clear</button>
            </div>
            <div className="max-h-56 overflow-y-auto scroll-thin font-mono text-[11px] px-3 pb-2 space-y-0.5">
              {consoleLog.length === 0 && <div className="text-muted">No messages</div>}
              {consoleLog.map((c, i) => (
                <div key={i} className={cx("whitespace-pre-wrap break-all", c.level === "error" ? "text-danger" : c.level === "warning" || c.level === "warn" ? "text-warn" : "text-muted")}>
                  {c.text}
                </div>
              ))}
            </div>
          </div>
        )}
      </Card>
      <SendDialog
        project={project}
        open={sendOpen}
        onClose={() => setSendOpen(false)}
        notes={notes}
        url={state.url}
        viewport={vp}
        consoleLog={consoleLog}
        takeShot={() => shot()}
        onSent={() => setNotes([])}
      />
    </div>
  );
}
