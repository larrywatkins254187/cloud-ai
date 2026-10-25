import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useSyncExternalStore } from "react";
import type { Msg, Req, ServerEvent } from "./types";
import { live } from "./ws";

export type MessagesData = { messages: Msg[]; requests: Req[]; has_more: boolean };

/** Streaming text per chat (cleared when the final message arrives). */
type Stream = { text: string; thinking: string; activity: string };
const streams = new Map<number, Stream>();
const streamSubs = new Set<() => void>();
function notifyStreams() {
  streamSubs.forEach((f) => f());
}
function setStream(chatId: number, s: Stream | null) {
  if (s) streams.set(chatId, s);
  else streams.delete(chatId);
  notifyStreams();
}

export function useStream(chatId: number): Stream | undefined {
  return useSyncExternalStore(
    (cb) => {
      streamSubs.add(cb);
      return () => streamSubs.delete(cb);
    },
    () => streams.get(chatId),
  );
}

function upsertRequest(qc: QueryClient, r: Req) {
  qc.setQueryData<MessagesData>(["messages", r.chat_id], (d) => {
    if (!d) return d;
    const others = d.requests.filter((x) => x.id !== r.id);
    return { ...d, requests: [r, ...others] };
  });
}

function handle(qc: QueryClient, ev: ServerEvent) {
  const e = ev as any;
  switch (e.type) {
    case "resync":
      qc.invalidateQueries();
      break;
    case "chat_created":
    case "chat_updated":
    case "chat_deleted": {
      const pid = e.type === "chat_deleted" ? e.project_id : e.chat.project_id;
      qc.invalidateQueries({ queryKey: ["chats", pid] });
      qc.invalidateQueries({ queryKey: ["recent-chats"] });
      qc.invalidateQueries({ queryKey: ["projects"] });
      if (e.type !== "chat_deleted") qc.invalidateQueries({ queryKey: ["chat", e.chat.id] });
      break;
    }
    case "message": {
      const m: Msg = e.message;
      qc.setQueryData<MessagesData>(["messages", m.chat_id], (d) =>
        d && !d.messages.some((x) => x.id === m.id) ? { ...d, messages: [...d.messages, m] } : d,
      );
      if (m.kind === "assistant" && !m.data?.parent) setStream(m.chat_id, null);
      else if (m.kind === "thinking") {
        const s = streams.get(m.chat_id);
        if (s) setStream(m.chat_id, { ...s, thinking: "" });
      }
      break;
    }
    case "delta": {
      const s = streams.get(e.chat_id) ?? { text: "", thinking: "", activity: "" };
      if (e.kind === "text") s.text += e.text;
      else if (e.kind === "thinking") s.thinking += e.text;
      else s.activity = e.text;
      setStream(e.chat_id, { ...s });
      break;
    }
    case "run_queued":
    case "run_started":
    case "run_finished":
      if (e.type === "run_finished") setStream(e.run.chat_id, null);
      qc.invalidateQueries({ queryKey: ["chat", e.run.chat_id] });
      qc.invalidateQueries({ queryKey: ["queue"] });
      qc.invalidateQueries({ queryKey: ["system"] });
      break;
    case "request_created":
    case "request_resolved":
      upsertRequest(qc, e.request);
      qc.invalidateQueries({ queryKey: ["chat", e.request.chat_id] });
      qc.invalidateQueries({ queryKey: ["pending"] });
      break;
    case "deploy_updated":
      qc.invalidateQueries({ queryKey: ["deploys", e.deploy.project_id] });
      qc.setQueryData(["deploy", e.deploy.id], (old: any) => (old ? { ...e.deploy, log: old.log } : old));
      break;
    case "deploy_log":
      qc.setQueryData(["deploy", e.deploy_id], (old: any) => (old ? { ...old, log: old.log + e.line + "\n" } : old));
      break;
  }
}

export function useLiveUpdates(onNotice: (text: string, level: string) => void) {
  const qc = useQueryClient();
  useEffect(() => {
    live.start();
    const off = live.subscribe((e) => {
      handle(qc, e);
      if (e.type === "notice") onNotice(e.text, e.level);
    });
    return () => {
      off();
      live.stop();
    };
  }, [qc, onNotice]);
}

export function useLiveStatus() {
  return useSyncExternalStore(
    (cb) => live.onStatus(cb),
    () => live.up,
  );
}
