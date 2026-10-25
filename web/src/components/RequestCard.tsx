import { Check, HelpCircle, ShieldAlert, X } from "lucide-react";
import { useState } from "react";
import { api } from "../lib/api";
import type { Question, Req } from "../lib/types";
import { Button, cx, Input, useAction } from "./ui";

function answerText(r: Req): string {
  const a = r.answer;
  if (!a) return "";
  if (r.kind === "approval") return a.allow ? "Allowed" : "Denied";
  if (a.allow === false) return "Declined";
  return Object.values(a.answers ?? {})
    .map((x: any) => (Array.isArray(x) ? x.join(", ") : String(x)))
    .join(" | ");
}

export function RequestCard({ r }: { r: Req }) {
  const { busy, run } = useAction();
  const [picked, setPicked] = useState<Record<string, string[]>>({});
  const [other, setOther] = useState<Record<string, string>>({});
  const [note, setNote] = useState("");

  if (r.status !== "pending") {
    return (
      <div className="flex items-center gap-2 text-xs text-muted px-1">
        {r.kind === "approval" ? <ShieldAlert className="size-3.5" /> : <HelpCircle className="size-3.5" />}
        {r.kind === "approval" ? `${r.data.tool}: ` : `${r.data.questions?.[0]?.header || "Question"}: `}
        <span className="text-text">{r.status === "expired" ? "expired" : answerText(r)}</span>
        {r.answered_by && <span>· by {r.answered_by}</span>}
      </div>
    );
  }

  const send = (answer: any) => run(() => api.post(`/api/requests/${r.id}/answer`, answer));

  if (r.kind === "approval") {
    return (
      <div className="rounded-xl border border-warn/50 bg-warn-soft/50 p-3.5 space-y-2.5">
        <div className="flex items-center gap-2 text-sm font-medium">
          <ShieldAlert className="size-4 text-warn" /> {r.data.title || `Allow ${r.data.tool}?`}
        </div>
        {r.data.reason && <div className="text-xs text-muted">{r.data.reason}</div>}
        <pre className="text-[12px] font-mono bg-panel rounded-lg p-2.5 overflow-x-auto scroll-thin max-h-60 whitespace-pre-wrap break-all border border-border">
          {r.data.summary}
        </pre>
        <Input value={note} onChange={(e) => setNote(e.target.value)} placeholder="Optional note for the AI if you deny" className="h-9" />
        <div className="flex gap-2">
          <Button variant="primary" size="sm" icon={<Check className="size-4" />} loading={busy} onClick={() => send({ allow: true })}>
            Allow
          </Button>
          <Button variant="outline" size="sm" icon={<X className="size-4" />} loading={busy} onClick={() => send({ allow: false, message: note || "Denied by the user" })}>
            Deny
          </Button>
        </div>
      </div>
    );
  }

  const qs: Question[] = r.data.questions ?? [];
  const final = () => {
    const answers: Record<string, string[]> = {};
    for (const q of qs) {
      const o = other[q.id]?.trim();
      answers[q.id] = o ? [...(q.multi ? picked[q.id] ?? [] : []), o] : picked[q.id] ?? [];
    }
    return answers;
  };
  const complete = qs.every((q) => (picked[q.id]?.length ?? 0) > 0 || other[q.id]?.trim());
  return (
    <div className="rounded-xl border border-accent/50 bg-accent-soft/40 p-3.5 space-y-4">
      {qs.map((q) => (
        <div key={q.id} className="space-y-2">
          <div className="flex items-start gap-2">
            <HelpCircle className="size-4 text-accent mt-0.5 shrink-0" />
            <div>
              {q.header && <div className="text-[11px] uppercase tracking-wide text-accent font-semibold">{q.header}</div>}
              <div className="text-sm font-medium">{q.question}</div>
            </div>
          </div>
          <div className="grid gap-1.5">
            {q.options.map((o) => {
              const on = picked[q.id]?.includes(o.label);
              return (
                <button
                  key={o.label}
                  onClick={() =>
                    setPicked((p) => {
                      const cur = p[q.id] ?? [];
                      if (q.multi) return { ...p, [q.id]: on ? cur.filter((x) => x !== o.label) : [...cur, o.label] };
                      setOther((x) => ({ ...x, [q.id]: "" }));
                      return { ...p, [q.id]: [o.label] };
                    })
                  }
                  className={cx(
                    "text-left rounded-lg border px-3 py-2 transition",
                    on ? "border-accent bg-panel ring-1 ring-accent" : "border-border bg-panel hover:border-accent/50",
                  )}
                >
                  <div className="text-sm font-medium">{o.label}</div>
                  {o.description && <div className="text-xs text-muted">{o.description}</div>}
                </button>
              );
            })}
          </div>
          {q.allow_other && (
            <Input
              value={other[q.id] ?? ""}
              onChange={(e) => {
                setOther((x) => ({ ...x, [q.id]: e.target.value }));
                if (!q.multi && e.target.value) setPicked((p) => ({ ...p, [q.id]: [] }));
              }}
              placeholder="Or type your own answer"
              className="h-9"
            />
          )}
        </div>
      ))}
      <div className="flex gap-2">
        <Button variant="primary" size="sm" disabled={!complete} loading={busy} onClick={() => send({ allow: true, answers: final() })}>
          Send answer
        </Button>
        <Button variant="ghost" size="sm" loading={busy} onClick={() => send({ allow: false, message: "The user declined to answer" })}>
          Skip
        </Button>
      </div>
    </div>
  );
}
