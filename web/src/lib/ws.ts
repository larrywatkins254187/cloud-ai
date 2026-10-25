import type { ServerEvent } from "./types";

type Listener = (e: ServerEvent) => void;

/** One WebSocket for all live updates, reconnecting with backoff. */
class LiveSocket {
  private ws: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private statusListeners = new Set<(up: boolean) => void>();
  private retry = 0;
  private stopped = true;
  up = false;

  start() {
    if (!this.stopped) return;
    this.stopped = false;
    this.connect();
  }

  stop() {
    this.stopped = true;
    this.ws?.close();
    this.ws = null;
  }

  private connect() {
    if (this.stopped) return;
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${location.host}/api/ws`);
    this.ws = ws;
    ws.onopen = () => {
      const wasDown = !this.up;
      this.retry = 0;
      this.setUp(true);
      // After a reconnect, refetch everything we may have missed.
      if (wasDown) this.emit({ type: "resync" });
    };
    ws.onmessage = (m) => {
      try {
        this.emit(JSON.parse(m.data));
      } catch {
        /* ignore */
      }
    };
    ws.onclose = () => {
      this.setUp(false);
      if (this.stopped) return;
      const delay = Math.min(15000, 500 * 2 ** this.retry++);
      setTimeout(() => this.connect(), delay);
    };
    ws.onerror = () => ws.close();
  }

  private setUp(up: boolean) {
    this.up = up;
    this.statusListeners.forEach((l) => l(up));
  }

  private emit(e: ServerEvent) {
    this.listeners.forEach((l) => l(e));
  }

  subscribe(l: Listener) {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  }

  onStatus(l: (up: boolean) => void) {
    this.statusListeners.add(l);
    return () => {
      this.statusListeners.delete(l);
    };
  }
}

export const live = new LiveSocket();

// Reconnect quickly when a phone comes back to the app.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && !live.up) {
    live.stop();
    live.start();
  }
});
