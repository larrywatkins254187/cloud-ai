import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { Clipboard, Copy } from "lucide-react";

const KEYS: { label: string; seq: string }[] = [
  { label: "Esc", seq: "\x1b" },
  { label: "Tab", seq: "\t" },
  { label: "Ctrl-C", seq: "\x03" },
  { label: "Ctrl-D", seq: "\x04" },
  { label: "↑", seq: "\x1b[A" },
  { label: "↓", seq: "\x1b[B" },
  { label: "←", seq: "\x1b[D" },
  { label: "→", seq: "\x1b[C" },
  { label: "Enter", seq: "\r" },
];

/** Copy text, falling back to execCommand where the Clipboard API is unavailable (plain http). */
async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    /* fall back */
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  try {
    document.execCommand("copy");
  } finally {
    ta.remove();
  }
}

/** xterm.js attached to a server-side PTY session over WebSocket. */
export default function TerminalView({ id }: { id: string }) {
  const el = useRef<HTMLDivElement>(null);
  const ws = useRef<WebSocket | null>(null);
  const termRef = useRef<Terminal | null>(null);
  useEffect(() => {
    const dark = document.documentElement.dataset.theme === "dark";
    const term = new Terminal({
      fontFamily: "JetBrains Mono, ui-monospace, Menlo, monospace",
      fontSize: window.innerWidth < 640 ? 12 : 13,
      cursorBlink: true,
      scrollback: 5000,
      theme: dark
        ? { background: "#0f1115", foreground: "#e6e8ee", cursor: "#8b7dff", selectionBackground: "#3a3570" }
        : { background: "#ffffff", foreground: "#15181e", cursor: "#6d5dfc", selectionBackground: "#d9d5ff" },
    });
    termRef.current = term;
    const fit = new FitAddon();
    term.loadAddon(fit);
    // Login URLs printed by `claude auth login` become clickable.
    term.loadAddon(new WebLinksAddon((_e, uri) => window.open(uri, "_blank", "noopener")));
    term.open(el.current!);
    fit.fit();

    // Linux-terminal shortcuts. Returning false makes xterm ignore the key, so for
    // paste the browser's native paste event fires and xterm inserts the text
    // (works on plain http too, where navigator.clipboard is unavailable).
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !(e.ctrlKey || e.metaKey) || e.altKey) return true;
      const k = e.key.toLowerCase();
      if (k === "v") return false; // Ctrl+V, Ctrl+Shift+V, Cmd+V → native paste
      if (k === "c" && (e.shiftKey || e.metaKey || term.hasSelection())) {
        // Ctrl+Shift+C (or Ctrl+C with a selection) copies instead of sending ^C.
        if (term.hasSelection()) copyText(term.getSelection());
        e.preventDefault(); // Chrome opens the inspector on Ctrl+Shift+C
        return false;
      }
      return true;
    });

    let closed = false;
    let retry: ReturnType<typeof setTimeout>;
    const connect = () => {
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      const s = new WebSocket(`${proto}//${location.host}/api/terminals/${id}/ws?cols=${term.cols}&rows=${term.rows}`);
      s.binaryType = "arraybuffer";
      s.onopen = () => term.reset();
      s.onmessage = (m) => term.write(typeof m.data === "string" ? m.data : new Uint8Array(m.data));
      s.onclose = () => {
        if (!closed) retry = setTimeout(connect, 2000);
      };
      ws.current = s;
    };
    connect();
    const onData = term.onData((d) => ws.current?.readyState === 1 && ws.current.send(d));
    const onResize = term.onResize(({ cols, rows }) => ws.current?.readyState === 1 && ws.current.send(JSON.stringify({ type: "resize", cols, rows })));
    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* hidden */
      }
    });
    ro.observe(el.current!);
    term.focus();
    return () => {
      closed = true;
      clearTimeout(retry);
      ro.disconnect();
      onData.dispose();
      onResize.dispose();
      ws.current?.close();
      term.dispose();
      termRef.current = null;
    };
  }, [id]);

  const paste = async () => {
    const term = termRef.current;
    if (!term) return;
    let text: string | null = null;
    try {
      text = await navigator.clipboard.readText();
    } catch {
      // No clipboard permission (or plain http): let the user paste into a prompt.
      text = window.prompt("Paste here:");
    }
    if (text) term.paste(text);
    term.focus();
  };
  const copy = () => {
    const term = termRef.current;
    if (term?.hasSelection()) copyText(term.getSelection());
    term?.focus();
  };

  return (
    <div className="flex flex-col h-full min-h-0">
      <div ref={el} className="flex-1 min-h-0 bg-panel p-1" />
      <div className="flex gap-1 overflow-x-auto scroll-thin px-2 py-1.5 border-t border-border bg-panel">
        <button onClick={paste} title="Paste (Ctrl+Shift+V)" className="shrink-0 flex items-center gap-1 rounded-md bg-panel-2 px-2.5 py-1 text-xs">
          <Clipboard className="size-3.5" /> Paste
        </button>
        <button onClick={copy} title="Copy selection (Ctrl+Shift+C)" className="shrink-0 flex items-center gap-1 rounded-md bg-panel-2 px-2.5 py-1 text-xs">
          <Copy className="size-3.5" /> Copy
        </button>
        <span className="hidden lg:flex items-center px-2 text-[11px] text-muted">Ctrl+Shift+V to paste · Ctrl+Shift+C to copy</span>
        {KEYS.map((k) => (
          <button
            key={k.label}
            onClick={() => ws.current?.readyState === 1 && ws.current.send(k.seq)}
            className="shrink-0 rounded-md bg-panel-2 px-2.5 py-1 text-xs font-mono lg:hidden"
          >
            {k.label}
          </button>
        ))}
      </div>
    </div>
  );
}
