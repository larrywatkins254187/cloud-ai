import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlarmClock, CalendarClock, X } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { api } from "../lib/api";
import { toolName } from "../lib/format";
import type { Chat, Msg } from "../lib/types";
import { Button, Field, Input, Modal, Textarea, useAction } from "./ui";

export const CONTINUE_PROMPT = "Continue where you left off. The previous turn was interrupted because the usage limit was reached.";

export type LimitInfo = {
  tool: string;
  resets_at: string;
  resume_at: string;
  resume_local: string;
  estimated: boolean;
};

function untilText(iso: string) {
  const mins = Math.round((new Date(iso).getTime() - Date.now()) / 60000);
  if (mins <= 0) return "now";
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return h ? `in ${h}h ${m}m` : `in ${m}m`;
}

function localTime(iso: string) {
  return new Date(iso).toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" });
}

/**
 * Shown under a "usage limit reached" error: suggests scheduling this chat to
 * continue automatically when the plan's limit window resets.
 */
export function LimitCard({ chat, msg, lastPrompt, didWork }: { chat: Chat; msg: Msg; lastPrompt: string; didWork: boolean }) {
  const limit: LimitInfo = msg.data.limit;
  const pending = chat.resume_schedule;
  const [open, setOpen] = useState(false);
  const qc = useQueryClient();
  const { busy, run } = useAction();
  // If the limit hit before the agent did anything, resending the prompt is what the user wants.
  const defaultPrompt = !didWork && lastPrompt ? lastPrompt : CONTINUE_PROMPT;
  const resumeIsPast = new Date(limit.resume_at).getTime() <= Date.now();

  const schedule = async (runAt: string, prompt: string) => {
    const r = await run(() => api.post(`/api/chats/${chat.id}/resume-schedule`, { run_at: runAt, prompt }), "Scheduled");
    if (r) {
      qc.invalidateQueries({ queryKey: ["chat", chat.id] });
      qc.invalidateQueries({ queryKey: ["schedules"] });
    }
    return !!r;
  };

  const cancel = async () => {
    if (!pending) return;
    const r = await run(() => api.del(`/api/schedules/${pending.id}`), "Scheduled continuation cancelled");
    if (r) {
      qc.invalidateQueries({ queryKey: ["chat", chat.id] });
      qc.invalidateQueries({ queryKey: ["schedules"] });
    }
  };

  return (
    <div className="rounded-xl border border-warn/40 bg-panel p-3.5 space-y-2.5">
      {pending ? (
        <>
          <div className="flex items-start gap-2.5 text-sm">
            <CalendarClock className="size-4 mt-0.5 text-accent shrink-0" />
            <div>
              This chat will continue automatically at <b>{localTime(pending.next_run_at)}</b> ({untilText(pending.next_run_at)}).
              <div className="text-xs text-muted mt-0.5">
                You'll find it under <Link to="/schedules" className="text-accent">Schedules</Link> too.
              </div>
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
              Change
            </Button>
            <Button size="sm" variant="ghost" loading={busy} icon={<X className="size-3.5" />} onClick={cancel}>
              Cancel
            </Button>
          </div>
        </>
      ) : (
        <>
          <div className="flex items-start gap-2.5 text-sm">
            <AlarmClock className="size-4 mt-0.5 text-warn shrink-0" />
            <div>
              {toolName(limit.tool)} hit its usage limit.{" "}
              {limit.estimated ? (
                <>The reset time wasn't reported, so we suggest trying again in about 5 hours.</>
              ) : (
                <>
                  The limit resets at <b>{localTime(limit.resets_at)}</b> ({untilText(limit.resets_at)}).
                </>
              )}{" "}
              Want this chat to continue automatically then?
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            {!resumeIsPast && (
              <Button
                size="sm"
                variant="primary"
                loading={busy}
                icon={<CalendarClock className="size-3.5" />}
                onClick={() => schedule(limit.resume_local, defaultPrompt)}
              >
                Continue at {localTime(limit.resume_at)}
              </Button>
            )}
            <Button size="sm" variant={resumeIsPast ? "primary" : "outline"} onClick={() => setOpen(true)}>
              {resumeIsPast ? "Schedule continuation…" : "Customize…"}
            </Button>
          </div>
        </>
      )}
      <ScheduleModal
        open={open}
        onClose={() => setOpen(false)}
        initialAt={limit.resume_local}
        initialPrompt={defaultPrompt}
        lastPrompt={lastPrompt}
        busy={busy}
        onSave={async (at, p) => (await schedule(at, p)) && setOpen(false)}
      />
    </div>
  );
}

function ScheduleModal({
  open,
  onClose,
  initialAt,
  initialPrompt,
  lastPrompt,
  busy,
  onSave,
}: {
  open: boolean;
  onClose: () => void;
  initialAt: string;
  initialPrompt: string;
  lastPrompt: string;
  busy: boolean;
  onSave: (at: string, prompt: string) => void;
}) {
  const [at, setAt] = useState(initialAt);
  const [prompt, setPrompt] = useState(initialPrompt);
  const sys = useQuery({ queryKey: ["system"], queryFn: () => api.get("/api/system"), enabled: open });
  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Continue after the usage limit"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={busy} disabled={!at || !prompt.trim()} onClick={() => onSave(at, prompt)}>
            Schedule
          </Button>
        </>
      }
    >
      <Field label="When" hint={sys.data?.timezone ? `Panel timezone: ${sys.data.timezone}` : undefined}>
        <Input type="datetime-local" value={at} onChange={(e) => setAt(e.target.value)} />
      </Field>
      <Field label="Prompt to send">
        <Textarea rows={4} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
      </Field>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="ghost" onClick={() => setPrompt(CONTINUE_PROMPT)}>
          Use "continue"
        </Button>
        {lastPrompt && (
          <Button size="sm" variant="ghost" onClick={() => setPrompt(lastPrompt)}>
            Resend my last prompt
          </Button>
        )}
      </div>
      <p className="text-xs text-muted">The prompt is sent to this same chat, so the agent keeps its conversation history.</p>
    </Modal>
  );
}
