import { useEffect, useRef } from "react";
import "@xterm/xterm/css/xterm.css";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";

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

/** xterm.js attached to a server-side PTY session over WebSocket. */
export default function TerminalView({ id }: { id: string }) {
  const el = useRef<HTMLDivElement>(null);
  const ws = useRef<WebSocket | null>(null);
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
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el.current!);
    fit.fit();
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
    };
  }, [id]);
  return (
    <div className="flex flex-col h-full min-h-0">
      <div ref={el} className="flex-1 min-h-0 bg-panel p-1" />
      <div className="flex gap-1 overflow-x-auto scroll-thin px-2 py-1.5 border-t border-border bg-panel lg:hidden">
        {KEYS.map((k) => (
          <button
            key={k.label}
            onClick={() => ws.current?.readyState === 1 && ws.current.send(k.seq)}
            className="shrink-0 rounded-md bg-panel-2 px-2.5 py-1 text-xs font-mono"
          >
            {k.label}
          </button>
        ))}
      </div>
    </div>
  );
}
