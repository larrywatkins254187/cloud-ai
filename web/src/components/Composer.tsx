import { FileText, Loader2, Paperclip, Send, Square, X } from "lucide-react";
import { useRef, useState } from "react";
import { api } from "../lib/api";
import { bytes } from "../lib/format";
import type { Attachment } from "../lib/types";
import { cx, IconButton, useToast } from "./ui";

/** Prompt box with file/image attachments (paste, drop or pick). */
export function Composer({
  onSend,
  onStop,
  running,
  placeholder,
  autoFocus,
  draftKey,
  initialAttachments,
}: {
  onSend: (prompt: string, attachments: Attachment[]) => Promise<boolean | void>;
  onStop?: () => void;
  running?: boolean;
  placeholder?: string;
  autoFocus?: boolean;
  draftKey?: string;
  initialAttachments?: Attachment[];
}) {
  const [text, setText] = useState(() => {
    try {
      return (draftKey && localStorage.getItem(draftKey)) || "";
    } catch {
      return "";
    }
  });
  const [atts, setAtts] = useState<Attachment[]>(initialAttachments ?? []);
  const [uploading, setUploading] = useState(false);
  const [sending, setSending] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const toast = useToast();

  const saveDraft = (t: string) => {
    setText(t);
    try {
      if (draftKey) t ? localStorage.setItem(draftKey, t) : localStorage.removeItem(draftKey);
    } catch {
      /* ignore */
    }
  };

  const upload = async (files: File[]) => {
    if (!files.length) return;
    setUploading(true);
    try {
      const res = await api.upload(files);
      setAtts((a) => [...a, ...res]);
    } catch (e: any) {
      toast(e.message, "danger");
    } finally {
      setUploading(false);
    }
  };

  const send = async () => {
    if ((!text.trim() && !atts.length) || sending) return;
    setSending(true);
    try {
      const ok = await onSend(text, atts);
      if (ok !== false) {
        saveDraft("");
        setAtts([]);
        taRef.current?.focus();
      }
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      className="rounded-2xl border border-border bg-panel shadow-sm focus-within:border-accent/60"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        upload(Array.from(e.dataTransfer.files));
      }}
    >
      {atts.length > 0 && (
        <div className="flex flex-wrap gap-2 px-3 pt-3">
          {atts.map((a) => (
            <div key={a.id} className="flex items-center gap-2 rounded-lg border border-border bg-panel-2 pl-1 pr-1.5 py-1 text-xs max-w-[220px]">
              {a.mime.startsWith("image/") ? (
                <img src={`/api/uploads/${a.id}`} className="size-8 rounded object-cover" alt="" />
              ) : (
                <FileText className="size-4 text-muted ml-1" />
              )}
              <div className="min-w-0">
                <div className="truncate">{a.filename}</div>
                <div className="text-muted">{bytes(a.size)}</div>
              </div>
              <button onClick={() => setAtts((x) => x.filter((y) => y.id !== a.id))} className="text-muted hover:text-text" aria-label="Remove">
                <X className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}
      <textarea
        ref={taRef}
        value={text}
        autoFocus={autoFocus}
        onChange={(e) => saveDraft(e.target.value)}
        onPaste={(e) => {
          const files = Array.from(e.clipboardData.files);
          if (files.length) {
            e.preventDefault();
            upload(files);
          }
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            send();
          }
        }}
        rows={Math.min(10, Math.max(2, text.split("\n").length))}
        placeholder={placeholder ?? "Ask Claude or Codex to do something… (Ctrl+Enter to send)"}
        className="block w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[15px] focus:outline-none placeholder:text-muted/70"
      />
      <div className="flex items-center gap-1 px-2 pb-2">
        <input
          ref={fileRef}
          type="file"
          multiple
          hidden
          onChange={(e) => {
            upload(Array.from(e.target.files ?? []));
            e.target.value = "";
          }}
        />
        <IconButton title="Attach files or images" onClick={() => fileRef.current?.click()} disabled={uploading}>
          {uploading ? <Loader2 className="size-4 animate-spin" /> : <Paperclip className="size-4" />}
        </IconButton>
        <div className="flex-1" />
        {running && onStop && (
          <button onClick={onStop} className="inline-flex items-center gap-1.5 h-9 px-3 rounded-lg text-sm bg-panel-2 hover:bg-border/60" title="Stop">
            <Square className="size-3.5 fill-current" /> Stop
          </button>
        )}
        <button
          onClick={send}
          disabled={(!text.trim() && !atts.length) || sending}
          className={cx(
            "inline-flex items-center justify-center size-9 rounded-lg transition",
            text.trim() || atts.length ? "bg-accent text-accent-fg" : "bg-panel-2 text-muted",
          )}
          title={running ? "Queue this prompt" : "Send"}
          aria-label="Send"
        >
          {sending ? <Loader2 className="size-4 animate-spin" /> : <Send className="size-4" />}
        </button>
      </div>
    </div>
  );
}
