import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect, useSyncExternalStore } from "react";
import type { AgentProgress } from "./agents";
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

/** Latest live progress per sub-agent (by its tool call id), per chat. */
const agentProgress = new Map<number, Record<string, AgentProgress>>();
const progressSubs = new Set<() => void>();
const EMPTY: Record<string, AgentProgress> = {};

export function useAgentProgress(chatId: number): Record<string, AgentProgress> {
  return useSyncExternalStore(
    (cb) => {
      progressSubs.add(cb);
      return () => progressSubs.delete(cb);
    },
    () => agentProgress.get(chatId) ?? EMPTY,
  );
}

function setAgentProgress(chatId: number, p: AgentProgress) {
  if (!p.ref) return;
  agentProgress.set(chatId, { ...(agentProgress.get(chatId) ?? {}), [p.ref]: p });
  progressSubs.forEach((f) => f());
}

const changeTimers = new Map<number, ReturnType<typeof setTimeout>>();
function refreshChangesSoon(qc: QueryClient, chatId: number) {
  if (changeTimers.has(chatId)) return;
  changeTimers.set(
    chatId,
    setTimeout(() => {
      changeTimers.delete(chatId);
      qc.invalidateQueries({ queryKey: ["changes", chatId] });
    }, 2500),
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
      // A finished edit or command may have changed files: refresh the Files tab (debounced).
      if (m.kind === "tool_result") refreshChangesSoon(qc, m.chat_id);
      else if (m.kind === "thinking") {
        const s = streams.get(m.chat_id);
        if (s) setStream(m.chat_id, { ...s, thinking: "" });
      }
      break;
    }
    case "delta": {
      if (e.kind === "agent") {
        try {
          setAgentProgress(e.chat_id, JSON.parse(e.text));
        } catch {
          /* ignore */
        }
        break;
      }
      const s = streams.get(e.chat_id) ?? { text: "", thinking: "", activity: "" };
      if (e.kind === "text") s.text += e.text;
      else if (e.kind === "thinking") s.thinking += e.text;
      else s.activity = e.text;
      setStream(e.chat_id, { ...s });
      break;
    }
    case "chat_context":
      qc.setQueryData(["context", e.chat_id], (old: any) => ({ live: old?.live ?? true, context: e.context }));
      break;
    case "run_queued":
    case "run_started":
    case "run_finished":
      if (e.type === "run_finished") {
        setStream(e.run.chat_id, null);
        // The prompt may have changed files.
        qc.invalidateQueries({ queryKey: ["changes", e.run.chat_id] });
      }
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
      qc.invalidateQueries({ queryKey: ["deploy-history", e.deploy.project_id] });
      qc.invalidateQueries({ queryKey: ["active-deploys"] });
      if (e.deploy.chat_id) {
        // Chat lists show each chat's latest deploy.
        qc.invalidateQueries({ queryKey: ["recent-chats"] });
        qc.invalidateQueries({ queryKey: ["chats", e.deploy.project_id] });
      }
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

/** Browser online/offline state. */
export function useOnline() {
  return useSyncExternalStore(
    (cb) => {
      window.addEventListener("online", cb);
      window.addEventListener("offline", cb);
      return () => {
        window.removeEventListener("online", cb);
        window.removeEventListener("offline", cb);
      };
    },
    () => navigator.onLine,
  );
}
