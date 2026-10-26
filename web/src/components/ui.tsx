import { Loader2, X } from "lucide-react";
import React, { createContext, useCallback, useContext, useEffect, useState } from "react";

export function cx(...c: (string | false | null | undefined)[]) {
  return c.filter(Boolean).join(" ");
}

type BtnVariant = "primary" | "secondary" | "ghost" | "danger" | "outline";

export const Button = React.forwardRef<
  HTMLButtonElement,
  React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: BtnVariant; size?: "sm" | "md"; loading?: boolean; icon?: React.ReactNode }
>(({ variant = "secondary", size = "md", loading, icon, className, children, disabled, ...rest }, ref) => {
  const v: Record<BtnVariant, string> = {
    primary: "bg-accent text-accent-fg hover:opacity-90",
    secondary: "bg-panel-2 text-text hover:bg-border/60",
    ghost: "text-text hover:bg-panel-2",
    danger: "bg-danger text-white hover:opacity-90",
    outline: "border border-border text-text hover:bg-panel-2",
  };
  return (
    <button
      ref={ref}
      disabled={disabled || loading}
      className={cx(
        "inline-flex items-center justify-center gap-1.5 rounded-lg font-medium transition select-none whitespace-nowrap",
        "disabled:opacity-50 disabled:pointer-events-none focus-visible:outline-2 focus-visible:outline-accent",
        size === "sm" ? "h-8 px-2.5 text-[13px]" : "h-10 px-3.5 text-sm",
        v[variant],
        className,
      )}
      {...rest}
    >
      {loading ? <Loader2 className="size-4 animate-spin" /> : icon}
      {children}
    </button>
  );
});

export function IconButton({
  className,
  title,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { title: string }) {
  return (
    <button
      title={title}
      aria-label={title}
      className={cx(
        "inline-flex size-9 items-center justify-center rounded-lg text-muted hover:bg-panel-2 hover:text-text transition disabled:opacity-40",
        className,
      )}
      {...rest}
    />
  );
}

// Callers may pass their own width (w-44, w-auto…); otherwise fields fill their container.
const width = (className?: string) => (/(^|\s)(w-|max-w-|flex-1)/.test(className ?? "") ? "" : "w-full");

const fieldCls =
  "rounded-lg border border-border bg-panel px-3 text-sm text-text placeholder:text-muted/70 focus:outline-none focus:ring-2 focus:ring-accent/40 focus:border-accent disabled:opacity-60";

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(({ className, ...rest }, ref) => (
  <input ref={ref} className={cx(fieldCls, width(className), "h-10", className)} {...rest} />
));

export const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  ({ className, ...rest }, ref) => <textarea ref={ref} className={cx(fieldCls, width(className), "py-2 min-h-20", className)} {...rest} />,
);

export function Select({ className, children, ...rest }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select className={cx(fieldCls, width(className), "h-10 pr-8 appearance-auto", className)} {...rest}>
      {children}
    </select>
  );
}

export function Field({ label, hint, children, className }: { label: string; hint?: React.ReactNode; children: React.ReactNode; className?: string }) {
  return (
    <label className={cx("block space-y-1.5", className)}>
      <span className="text-[13px] font-medium text-text">{label}</span>
      {children}
      {hint && <span className="block text-xs text-muted">{hint}</span>}
    </label>
  );
}

export function Toggle({ checked, onChange, label, hint }: { checked: boolean; onChange: (v: boolean) => void; label?: string; hint?: string }) {
  return (
    <label className="flex items-start gap-3 cursor-pointer select-none py-1">
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        onClick={() => onChange(!checked)}
        className={cx("mt-0.5 relative h-5 w-9 shrink-0 rounded-full transition", checked ? "bg-accent" : "bg-border")}
      >
        <span className={cx("absolute top-0.5 size-4 rounded-full bg-white shadow transition-all", checked ? "left-[18px]" : "left-0.5")} />
      </button>
      {label && (
        <span className="text-sm">
          {label}
          {hint && <span className="block text-xs text-muted">{hint}</span>}
        </span>
      )}
    </label>
  );
}

export function Card({ className, children, ...rest }: React.ComponentProps<"div">) {
  return (
    <div className={cx("rounded-xl border border-border bg-panel", className)} {...rest}>
      {children}
    </div>
  );
}

export function Badge({ children, tone = "neutral", className }: { children: React.ReactNode; tone?: "neutral" | "accent" | "ok" | "warn" | "danger"; className?: string }) {
  const t = {
    neutral: "bg-panel-2 text-muted",
    accent: "bg-accent-soft text-accent",
    ok: "bg-ok-soft text-ok",
    warn: "bg-warn-soft text-warn",
    danger: "bg-danger-soft text-danger",
  }[tone];
  return <span className={cx("inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] font-medium", t, className)}>{children}</span>;
}

/** Loading indicator, centered both ways in the space it is given.
 *  `page` reserves a taller area for whole-page loads; `inline` drops the wrapper. */
export function Spinner({ className, page, inline }: { className?: string; page?: boolean; inline?: boolean }) {
  const icon = <Loader2 className={cx("animate-spin text-muted", page ? "size-6" : "size-5", className)} />;
  if (inline) return icon;
  return (
    <div role="status" aria-label="Loading" className={cx("flex flex-1 w-full h-full items-center justify-center", page ? "min-h-[60dvh]" : "min-h-32 py-6")}>
      {icon}
    </div>
  );
}

export function Empty({ icon, title, children }: { icon?: React.ReactNode; title: string; children?: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center text-center py-14 px-6 text-muted">
      {icon && <div className="mb-3 opacity-70">{icon}</div>}
      <div className="font-medium text-text">{title}</div>
      {children && <div className="mt-1 text-sm max-w-md">{children}</div>}
    </div>
  );
}

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  wide,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  footer?: React.ReactNode;
  wide?: boolean;
}) {
  useEffect(() => {
    if (!open) return;
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-end sm:items-center justify-center bg-black/50 sm:p-4" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cx(
          "bg-panel w-full rounded-t-2xl sm:rounded-2xl border border-border shadow-2xl flex flex-col max-h-[92dvh]",
          wide ? "sm:max-w-3xl" : "sm:max-w-lg",
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-3.5 border-b border-border">
          <h2 className="font-semibold">{title}</h2>
          <IconButton title="Close" onClick={onClose}>
            <X className="size-4" />
          </IconButton>
        </div>
        <div className="overflow-y-auto scroll-thin px-5 py-4 space-y-4">{children}</div>
        {footer && <div className="flex justify-end gap-2 px-5 py-3 border-t border-border safe-bottom">{footer}</div>}
      </div>
    </div>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: { id: T; label: string; icon?: React.ReactNode }[]; value: T; onChange: (t: T) => void }) {
  return (
    <div className="flex gap-1 overflow-x-auto scroll-thin border-b border-border">
      {tabs.map((t) => (
        <button
          key={t.id}
          onClick={() => onChange(t.id)}
          className={cx(
            "flex items-center gap-1.5 px-3 py-2.5 text-sm whitespace-nowrap border-b-2 -mb-px transition",
            value === t.id ? "border-accent text-text font-medium" : "border-transparent text-muted hover:text-text",
          )}
        >
          {t.icon}
          {t.label}
        </button>
      ))}
    </div>
  );
}

export function PageHeader({ title, subtitle, actions }: { title: React.ReactNode; subtitle?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-3 mb-5">
      <div className="min-w-0">
        <h1 className="text-xl font-semibold tracking-tight truncate">{title}</h1>
        {subtitle && <div className="text-sm text-muted mt-0.5">{subtitle}</div>}
      </div>
      {actions && <div className="flex flex-wrap gap-2">{actions}</div>}
    </div>
  );
}

// ---------- confirm dialog ----------

export type ConfirmOptions = {
  title: string;
  message?: React.ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
  /** Optional checkbox shown above the buttons; its state is returned. */
  checkbox?: { label: string; defaultChecked?: boolean };
};
type ConfirmResult = { ok: boolean; checked: boolean };
type ConfirmState = ConfirmOptions & { resolve: (r: ConfirmResult) => void };
const ConfirmCtx = createContext<(o: ConfirmOptions) => Promise<ConfirmResult>>(async () => ({ ok: false, checked: false }));

export function ConfirmProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = useState<ConfirmState | null>(null);
  const [checked, setChecked] = useState(false);
  const ask = useCallback(
    (o: ConfirmOptions) =>
      new Promise<ConfirmResult>((resolve) => {
        setChecked(!!o.checkbox?.defaultChecked);
        setState({ ...o, resolve });
      }),
    [],
  );
  const close = useCallback(
    (ok: boolean) => {
      state?.resolve({ ok, checked });
      setState(null);
    },
    [state, checked],
  );
  const cancel = useCallback(() => close(false), [close]);
  return (
    <ConfirmCtx.Provider value={ask}>
      {children}
      <Modal
        open={!!state}
        onClose={cancel}
        title={state?.title ?? ""}
        footer={
          <>
            <Button variant="ghost" onClick={cancel}>
              {state?.cancelLabel ?? "Cancel"}
            </Button>
            <Button variant={state?.danger ? "danger" : "primary"} autoFocus onClick={() => close(true)}>
              {state?.confirmLabel ?? "Confirm"}
            </Button>
          </>
        }
      >
        {state?.message && <div className="text-sm text-muted whitespace-pre-line">{state.message}</div>}
        {state?.checkbox && (
          <label className="flex items-center gap-2 text-sm cursor-pointer select-none">
            <input type="checkbox" className="size-4 accent-accent" checked={checked} onChange={(e) => setChecked(e.target.checked)} />
            {state.checkbox.label}
          </label>
        )}
      </Modal>
    </ConfirmCtx.Provider>
  );
}

/** Ask for confirmation in a modal. Resolves to true when confirmed. */
export function useConfirm() {
  const ask = useContext(ConfirmCtx);
  return useCallback((o: ConfirmOptions) => ask(o).then((r) => r.ok), [ask]);
}

/** Like useConfirm, but also returns the checkbox state. */
export function useConfirmWithOption() {
  return useContext(ConfirmCtx);
}

// ---------- toasts ----------

type Toast = { id: number; text: string; tone: "ok" | "danger" | "info" };
const ToastCtx = createContext<(text: string, tone?: Toast["tone"]) => void>(() => {});

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [items, setItems] = useState<Toast[]>([]);
  const push = useCallback((text: string, tone: Toast["tone"] = "info") => {
    const id = Date.now() + Math.random();
    setItems((x) => [...x, { id, text, tone }]);
    setTimeout(() => setItems((x) => x.filter((t) => t.id !== id)), tone === "danger" ? 7000 : 3500);
  }, []);
  return (
    <ToastCtx.Provider value={push}>
      {children}
      <div className="fixed z-[60] bottom-20 lg:bottom-4 right-4 left-4 sm:left-auto flex flex-col gap-2 items-end pointer-events-none">
        {items.map((t) => (
          <div
            key={t.id}
            className={cx(
              "pointer-events-auto max-w-sm w-full sm:w-auto rounded-xl border px-4 py-2.5 text-sm shadow-lg",
              t.tone === "danger" ? "bg-danger-soft border-danger/40 text-danger" : t.tone === "ok" ? "bg-ok-soft border-ok/40 text-ok" : "bg-panel border-border",
            )}
          >
            {t.text}
          </div>
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

export const useToast = () => useContext(ToastCtx);

/** Run an async action, toasting failures. */
export function useAction() {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const run = useCallback(
    async <T,>(fn: () => Promise<T>, ok?: string): Promise<T | undefined> => {
      setBusy(true);
      try {
        const r = await fn();
        if (ok) toast(ok, "ok");
        return r;
      } catch (e: any) {
        toast(e?.message || String(e), "danger");
        return undefined;
      } finally {
        setBusy(false);
      }
    },
    [toast],
  );
  return { busy, run };
}

export function StatusDot({ status }: { status: string }) {
  const c =
    status === "running"
      ? "bg-accent pulse-dot"
      : status === "waiting"
        ? "bg-warn pulse-dot"
        : status === "queued"
          ? "bg-muted pulse-dot"
          : status === "error"
            ? "bg-danger"
            : "bg-border";
  return <span className={cx("inline-block size-2 rounded-full shrink-0", c)} title={status} />;
}
