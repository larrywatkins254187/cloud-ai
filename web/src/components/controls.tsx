// Themed form controls used instead of the browser's native ones: a searchable
// select, date / time / date-time / date-range pickers, a number stepper and a
// checkbox. Popovers render in a portal so they are never clipped by a modal
// or a scrolling container, and flip above the field when there is no room below.
import { Calendar, Check, ChevronDown, ChevronLeft, ChevronRight, Clock, Minus, Plus, Search, X } from "lucide-react";
import React, { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";

function cx(...c: (string | false | null | undefined)[]) {
  return c.filter(Boolean).join(" ");
}

// Callers may pass their own width (w-44, w-auto…); otherwise fields fill their container.
const width = (className?: string) => (/(^|\s)(w-|max-w-|flex-1)/.test(className ?? "") ? "" : "w-full");
const fieldBase =
  "rounded-lg border border-border bg-panel text-sm text-text focus:outline-none focus-visible:ring-2 focus-visible:ring-accent/40 focus-visible:border-accent disabled:opacity-60 disabled:cursor-not-allowed";
/** Heights follow the caller's h-* class (h-8, h-9…), defaulting to h-10. */
const height = (className?: string) => (/(^|\s)h-\d/.test(className ?? "") ? "" : "h-10");

// ---------- popover ----------

type Place = { left: number; top?: number; bottom?: number; width: number; maxHeight: number };

/** Open state, outside-click / Escape handling and fixed positioning next to an anchor. */
function usePopover<T extends HTMLElement>(opts: { minWidth?: number; matchWidth?: boolean; prefHeight?: number } = {}) {
  const anchor = useRef<T>(null);
  const pop = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [place, setPlace] = useState<Place | null>(null);
  const { minWidth = 220, matchWidth = true, prefHeight = 320 } = opts;

  const measure = useCallback(() => {
    const a = anchor.current;
    if (!a) return;
    const r = a.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const w = Math.min(Math.max(matchWidth ? r.width : 0, minWidth), vw - 16);
    const left = Math.max(8, Math.min(r.left, vw - w - 8));
    const below = vh - r.bottom - 8;
    const above = r.top - 8;
    if (below >= Math.min(prefHeight, 240) || below >= above) {
      setPlace({ left, top: r.bottom + 4, width: w, maxHeight: Math.max(160, below - 4) });
    } else {
      setPlace({ left, bottom: vh - r.top + 4, width: w, maxHeight: Math.max(160, above - 4) });
    }
  }, [minWidth, matchWidth, prefHeight]);

  useLayoutEffect(() => {
    if (!open) return;
    measure();
    const on = () => measure();
    window.addEventListener("resize", on);
    window.addEventListener("scroll", on, true);
    return () => {
      window.removeEventListener("resize", on);
      window.removeEventListener("scroll", on, true);
    };
  }, [open, measure]);

  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      const t = e.target as Node;
      if (anchor.current?.contains(t) || pop.current?.contains(t)) return;
      setOpen(false);
    };
    document.addEventListener("pointerdown", down, true);
    return () => document.removeEventListener("pointerdown", down, true);
  }, [open]);

  const close = useCallback((refocus = true) => {
    setOpen(false);
    if (refocus) anchor.current?.focus();
  }, []);

  return { anchor, pop, open, setOpen, close, place };
}

function Popover({
  state,
  children,
  className,
  label,
}: {
  state: { pop: React.RefObject<HTMLDivElement | null>; open: boolean; place: Place | null; close: (refocus?: boolean) => void };
  children: React.ReactNode;
  className?: string;
  label?: string;
}) {
  if (!state.open || !state.place) return null;
  const p = state.place;
  return createPortal(
    <div
      ref={state.pop}
      role="dialog"
      aria-label={label}
      className={cx("fixed z-[70] rounded-xl border border-border bg-panel shadow-2xl flex flex-col overflow-hidden", className)}
      style={{ left: p.left, top: p.top, bottom: p.bottom, width: p.width, maxHeight: p.maxHeight }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          // Keep the Escape inside the popover so it doesn't also close a surrounding modal.
          e.stopPropagation();
          e.nativeEvent.stopImmediatePropagation();
          state.close();
        }
      }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {children}
    </div>,
    document.body,
  );
}

// ---------- select ----------

export type Option<V extends string | number = string> = {
  value: V;
  label: string;
  /** Second line in the list (and searched too). */
  hint?: string;
  /** Group heading shown above the first option of each group. */
  group?: string;
  icon?: React.ReactNode;
  disabled?: boolean;
};

/** Themed dropdown with search. Search shows automatically for longer lists. */
export function Select<V extends string | number>({
  value,
  onChange,
  options,
  placeholder = "Choose…",
  searchable,
  disabled,
  className,
  "aria-label": ariaLabel,
  id,
  title,
  emptyText = "No matches",
}: {
  value: V | null | undefined;
  onChange: (v: V) => void;
  options: Option<V>[];
  placeholder?: string;
  searchable?: boolean;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
  id?: string;
  title?: string;
  emptyText?: string;
}) {
  const po = usePopover<HTMLButtonElement>({ minWidth: 220 });
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listId = useId();
  const withSearch = searchable ?? options.length > 7;
  const selected = options.find((o) => o.value === value);

  const filtered = useMemo(() => {
    const s = q.trim().toLowerCase();
    if (!s) return options;
    const words = s.split(/\s+/);
    return options.filter((o) => {
      const hay = `${o.label} ${o.hint ?? ""} ${o.group ?? ""}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  }, [options, q]);

  const openList = (query = "") => {
    if (disabled) return;
    setQ(query);
    const i = query ? 0 : Math.max(0, options.findIndex((o) => o.value === value));
    setActive(i);
    po.setOpen(true);
  };

  useEffect(() => {
    if (!po.open) return;
    if (withSearch) requestAnimationFrame(() => searchRef.current?.focus());
    else requestAnimationFrame(() => listRef.current?.focus());
  }, [po.open, withSearch]);

  useEffect(() => {
    if (!po.open) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [active, po.open, filtered]);

  const pick = (o: Option<V> | undefined) => {
    if (!o || o.disabled) return;
    if (o.value !== value) onChange(o.value);
    po.close();
  };

  const move = (d: number) => {
    if (!filtered.length) return;
    let i = active;
    for (let n = 0; n < filtered.length; n++) {
      i = (i + d + filtered.length) % filtered.length;
      if (!filtered[i].disabled) break;
    }
    setActive(i);
  };

  const onListKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") (e.preventDefault(), move(1));
    else if (e.key === "ArrowUp") (e.preventDefault(), move(-1));
    else if (e.key === "Home") (e.preventDefault(), setActive(0));
    else if (e.key === "End") (e.preventDefault(), setActive(filtered.length - 1));
    else if (e.key === "Enter") (e.preventDefault(), pick(filtered[active]));
    else if (e.key === "Tab") po.close(false);
    else if (!withSearch && e.key.length === 1 && /\S/.test(e.key)) {
      const i = filtered.findIndex((o) => o.label.toLowerCase().startsWith(e.key.toLowerCase()));
      if (i >= 0) setActive(i);
    }
  };

  let lastGroup: string | undefined;
  return (
    <>
      <button
        ref={po.anchor}
        type="button"
        id={id}
        title={title ?? selected?.label}
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={po.open}
        aria-controls={po.open ? listId : undefined}
        onClick={() => (po.open ? po.close() : openList())}
        onKeyDown={(e) => {
          if (["ArrowDown", "ArrowUp", "Enter", " "].includes(e.key)) {
            e.preventDefault();
            openList();
          } else if (withSearch && e.key.length === 1 && /\S/.test(e.key) && !e.metaKey && !e.ctrlKey) {
            e.preventDefault();
            openList(e.key);
          }
        }}
        className={cx(
          fieldBase,
          width(className),
          height(className),
          "inline-flex items-center gap-2 pl-3 pr-2 text-left",
          po.open && "ring-2 ring-accent/40 border-accent",
          className,
        )}
      >
        {selected?.icon}
        <span className={cx("flex-1 min-w-0 truncate", !selected && "text-muted/80")}>{selected?.label ?? placeholder}</span>
        <ChevronDown className={cx("size-4 shrink-0 text-muted transition-transform", po.open && "rotate-180")} />
      </button>
      <Popover state={po} label={ariaLabel ?? placeholder}>
        {withSearch && (
          <div className="flex items-center gap-2 px-3 border-b border-border shrink-0">
            <Search className="size-4 text-muted shrink-0" />
            <input
              ref={searchRef}
              value={q}
              onChange={(e) => {
                setQ(e.target.value);
                setActive(0);
              }}
              onKeyDown={onListKey}
              placeholder="Search…"
              aria-label="Search options"
              aria-controls={listId}
              aria-activedescendant={filtered[active] ? `${listId}-${active}` : undefined}
              className="h-10 flex-1 min-w-0 bg-transparent text-sm outline-none placeholder:text-muted/70"
            />
            {q && (
              <button type="button" className="p-1 text-muted hover:text-text" aria-label="Clear search" onClick={() => (setQ(""), searchRef.current?.focus())}>
                <X className="size-3.5" />
              </button>
            )}
          </div>
        )}
        <div
          ref={listRef}
          id={listId}
          role="listbox"
          tabIndex={-1}
          aria-activedescendant={filtered[active] ? `${listId}-${active}` : undefined}
          onKeyDown={onListKey}
          className="overflow-y-auto scroll-thin p-1 outline-none"
        >
          {filtered.length === 0 && <div className="px-3 py-6 text-center text-sm text-muted">{emptyText}</div>}
          {filtered.map((o, i) => {
            const head = o.group && o.group !== lastGroup ? o.group : null;
            lastGroup = o.group;
            const isSel = o.value === value;
            return (
              <React.Fragment key={`${o.value}`}>
                {head && <div className="px-2.5 pt-2 pb-1 text-[11px] font-semibold uppercase tracking-wider text-muted">{head}</div>}
                <div
                  id={`${listId}-${i}`}
                  data-index={i}
                  role="option"
                  aria-selected={isSel}
                  aria-disabled={o.disabled || undefined}
                  onMouseMove={() => !o.disabled && active !== i && setActive(i)}
                  onClick={() => pick(o)}
                  className={cx(
                    "flex items-start gap-2 rounded-lg px-2.5 py-2 text-sm cursor-pointer select-none",
                    i === active && !o.disabled && "bg-panel-2",
                    o.disabled && "opacity-50 cursor-not-allowed",
                  )}
                >
                  {o.icon && <span className="mt-0.5 shrink-0">{o.icon}</span>}
                  <span className="flex-1 min-w-0">
                    <span className={cx("block truncate", isSel && "font-medium text-accent")}>{o.label}</span>
                    {o.hint && <span className="block text-xs text-muted leading-snug">{o.hint}</span>}
                  </span>
                  {isSel && <Check className="size-4 mt-0.5 shrink-0 text-accent" />}
                </div>
              </React.Fragment>
            );
          })}
        </div>
      </Popover>
    </>
  );
}

// ---------- number ----------

/** Number field with − / + buttons. The value is kept as text so it can be edited freely. */
export function NumberInput({
  value,
  onChange,
  min,
  max,
  step = 1,
  className,
  placeholder,
  disabled,
  suffix,
  "aria-label": ariaLabel,
  decimals,
  steppers = true,
}: {
  value: string | number | null | undefined;
  onChange: (v: string) => void;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  placeholder?: string;
  disabled?: boolean;
  suffix?: string;
  "aria-label"?: string;
  /** Allow a decimal point (defaults to true when `step` is fractional). */
  decimals?: boolean;
  /** Show the − / + buttons (arrow keys still step when hidden). */
  steppers?: boolean;
}) {
  const allowDot = decimals ?? !Number.isInteger(step);
  const text = value === null || value === undefined ? "" : String(value);
  const num = Number(text);
  const clamp = (n: number) => {
    if (min !== undefined) n = Math.max(min, n);
    if (max !== undefined) n = Math.min(max, n);
    return n;
  };
  const fmt = (n: number) => (allowDot ? String(Math.round(n * 1e6) / 1e6) : String(Math.round(n)));
  const bump = (d: number) => {
    const base = text.trim() === "" || Number.isNaN(num) ? (min ?? 0) - (d > 0 ? step : -step) : num;
    onChange(fmt(clamp(base + d * step)));
  };
  const btn = "flex items-center justify-center w-8 shrink-0 text-muted hover:text-text hover:bg-panel-2 disabled:opacity-40 disabled:pointer-events-none";
  return (
    <div className={cx(fieldBase, width(className), height(className), "inline-flex items-stretch overflow-hidden focus-within:ring-2 focus-within:ring-accent/40 focus-within:border-accent", disabled && "opacity-60", className)}>
      <input
        type="text"
        inputMode={allowDot ? "decimal" : "numeric"}
        value={text}
        disabled={disabled}
        placeholder={placeholder}
        aria-label={ariaLabel}
        role="spinbutton"
        aria-valuemin={min}
        aria-valuemax={max}
        aria-valuenow={Number.isNaN(num) ? undefined : num}
        onChange={(e) => {
          const re = allowDot ? /^-?\d*\.?\d*$/ : /^-?\d*$/;
          const v = e.target.value.replace(/\s/g, "");
          if (re.test(v) && (min === undefined || min < 0 || !v.startsWith("-"))) onChange(v);
        }}
        onBlur={() => {
          if (text.trim() === "" || text === "-" || Number.isNaN(num)) return;
          const c = fmt(clamp(num));
          if (c !== text) onChange(c);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowUp") (e.preventDefault(), bump(1));
          if (e.key === "ArrowDown") (e.preventDefault(), bump(-1));
        }}
        className="order-2 flex-1 min-w-0 bg-transparent px-2 text-center tabular-nums outline-none placeholder:text-muted/70"
      />
      {steppers && <button type="button" tabIndex={-1} className={cx(btn, "order-1 border-r border-border")} aria-label="Decrease" disabled={disabled || (min !== undefined && num <= min)} onClick={() => bump(-1)}>
        <Minus className="size-3.5" />
      </button>}
      {suffix && <span className="order-3 flex items-center pr-2 text-xs text-muted">{suffix}</span>}
      {steppers && <button type="button" tabIndex={-1} className={cx(btn, "order-4 border-l border-border")} aria-label="Increase" disabled={disabled || (max !== undefined && num >= max)} onClick={() => bump(1)}>
        <Plus className="size-3.5" />
      </button>}
    </div>
  );
}

// ---------- checkbox ----------

export function Checkbox({
  checked,
  onChange,
  label,
  hint,
  disabled,
  className,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label?: React.ReactNode;
  hint?: React.ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  const box = (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "mt-0.5 inline-flex size-[18px] shrink-0 items-center justify-center rounded-[5px] border transition",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:opacity-50",
        checked ? "bg-accent border-accent text-accent-fg" : "bg-panel border-border hover:border-muted",
      )}
    >
      {checked && <Check className="size-3.5" strokeWidth={3} />}
    </button>
  );
  if (!label) return box;
  return (
    <label className={cx("flex items-start gap-2.5 cursor-pointer select-none text-sm", disabled && "cursor-not-allowed opacity-70", className)}>
      {box}
      <span>
        {label}
        {hint && <span className="block text-xs text-muted">{hint}</span>}
      </span>
    </label>
  );
}

// ---------- dates ----------

const pad = (n: number) => String(n).padStart(2, "0");
export const ymd = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
export function parseYmd(s: string | null | undefined): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(s ?? "");
  if (!m) return null;
  const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return Number.isNaN(d.getTime()) ? null : d;
}
const addDays = (d: Date, n: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
const sameDay = (a: Date | null, b: Date | null) => !!a && !!b && ymd(a) === ymd(b);
const fmtDate = (d: Date) => d.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });

/** "Sep 20 – Oct 27, 2026", repeating the year only when it differs. */
function rangeText(a: Date, b: Date) {
  if (sameDay(a, b)) return fmtDate(a);
  if (a.getFullYear() !== b.getFullYear()) return `${fmtDate(a)} – ${fmtDate(b)}`;
  return `${a.toLocaleDateString(undefined, { day: "numeric", month: "short" })} – ${fmtDate(b)}`;
}

/** Weeks start on Saturday, like the Usage page's weeks. */
const WEEK_START = 6;
function weekdayLabels() {
  // 2023-12-31 was a Sunday (day 0); offset from there.
  return Array.from({ length: 7 }, (_, i) => new Date(2023, 11, 31 + ((WEEK_START + i) % 7)).toLocaleDateString(undefined, { weekday: "narrow" }));
}

function MonthGrid({
  month,
  onMonth,
  isSelected,
  inRange,
  onPick,
  onHover,
  min,
  max,
}: {
  month: Date;
  onMonth: (d: Date) => void;
  isSelected: (d: Date) => boolean;
  inRange?: (d: Date) => boolean;
  onPick: (d: Date) => void;
  onHover?: (d: Date | null) => void;
  min?: Date | null;
  max?: Date | null;
}) {
  const first = new Date(month.getFullYear(), month.getMonth(), 1);
  const lead = (first.getDay() - WEEK_START + 7) % 7;
  const start = addDays(first, -lead);
  const days = Array.from({ length: 42 }, (_, i) => addDays(start, i));
  const today = new Date();
  const labels = useMemo(weekdayLabels, []);
  const [focus, setFocus] = useState<Date>(() => days.find((d) => isSelected(d)) ?? (first <= today && addDays(first, 31) > today ? today : first));
  const gridRef = useRef<HTMLDivElement>(null);
  const off = (d: Date) => (!!min && ymd(d) < ymd(min)) || (!!max && ymd(d) > ymd(max));

  useEffect(() => {
    if (focus.getMonth() !== month.getMonth() || focus.getFullYear() !== month.getFullYear()) {
      setFocus(new Date(month.getFullYear(), month.getMonth(), Math.min(focus.getDate(), 28)));
    }
  }, [month]); // eslint-disable-line react-hooks/exhaustive-deps

  const key = (e: React.KeyboardEvent) => {
    const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };
    if (e.key in step) {
      e.preventDefault();
      const n = addDays(focus, step[e.key]);
      setFocus(n);
      if (n.getMonth() !== month.getMonth()) onMonth(new Date(n.getFullYear(), n.getMonth(), 1));
      requestAnimationFrame(() => gridRef.current?.querySelector<HTMLElement>(`[data-day="${ymd(n)}"]`)?.focus());
    } else if (e.key === "PageUp" || e.key === "PageDown") {
      e.preventDefault();
      onMonth(new Date(month.getFullYear(), month.getMonth() + (e.key === "PageUp" ? -1 : 1), 1));
    }
  };

  return (
    <div className="p-3 select-none">
      <div className="flex items-center justify-between mb-2">
        <button type="button" className="p-1.5 rounded-lg text-muted hover:bg-panel-2 hover:text-text" aria-label="Previous month" onClick={() => onMonth(new Date(month.getFullYear(), month.getMonth() - 1, 1))}>
          <ChevronLeft className="size-4" />
        </button>
        <div className="text-sm font-medium">{month.toLocaleDateString(undefined, { month: "long", year: "numeric" })}</div>
        <button type="button" className="p-1.5 rounded-lg text-muted hover:bg-panel-2 hover:text-text" aria-label="Next month" onClick={() => onMonth(new Date(month.getFullYear(), month.getMonth() + 1, 1))}>
          <ChevronRight className="size-4" />
        </button>
      </div>
      <div className="grid grid-cols-7 text-center text-[11px] text-muted mb-1">
        {labels.map((l, i) => (
          <div key={i} className="py-1">
            {l}
          </div>
        ))}
      </div>
      <div ref={gridRef} role="grid" className="grid grid-cols-7 gap-y-0.5" onKeyDown={key} onMouseLeave={() => onHover?.(null)}>
        {days.map((d) => {
          const outside = d.getMonth() !== month.getMonth();
          const sel = isSelected(d);
          const range = !sel && inRange?.(d);
          const disabled = off(d);
          return (
            <button
              key={ymd(d)}
              type="button"
              data-day={ymd(d)}
              role="gridcell"
              aria-selected={sel}
              aria-label={d.toLocaleDateString(undefined, { dateStyle: "full" })}
              tabIndex={sameDay(d, focus) ? 0 : -1}
              disabled={disabled}
              onMouseEnter={() => onHover?.(d)}
              onFocus={() => setFocus(d)}
              onClick={() => onPick(d)}
              className={cx(
                "h-9 text-sm tabular-nums transition outline-none focus-visible:ring-2 focus-visible:ring-accent/50",
                sel ? "bg-accent text-accent-fg font-medium rounded-lg" : range ? "bg-accent-soft text-accent" : "rounded-lg hover:bg-panel-2",
                outside && !sel && !range && "text-muted/60",
                sameDay(d, today) && !sel && "font-semibold underline decoration-accent decoration-2 underline-offset-4",
                disabled && "opacity-30 pointer-events-none",
              )}
            >
              {d.getDate()}
            </button>
          );
        })}
      </div>
    </div>
  );
}

function TimeColumns({ value, onChange, minuteStep = 1 }: { value: string; onChange: (hhmm: string) => void; minuteStep?: number }) {
  const [h, m] = (/^\d{1,2}:\d{2}/.test(value) ? value : "09:00").split(":").map(Number);
  const hours = Array.from({ length: 24 }, (_, i) => i);
  const minutes = Array.from({ length: Math.ceil(60 / minuteStep) }, (_, i) => i * minuteStep);
  if (!minutes.includes(m)) minutes.push(m), minutes.sort((a, b) => a - b);
  const hRef = useRef<HTMLDivElement>(null);
  const mRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    hRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: "center" });
    mRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: "center" });
  }, []);
  const col = (ref: React.RefObject<HTMLDivElement | null>, items: number[], cur: number, set: (n: number) => void, label: string) => (
    <div
      ref={ref}
      role="listbox"
      aria-label={label}
      tabIndex={0}
      onKeyDown={(e) => {
        const i = items.indexOf(cur);
        if (e.key === "ArrowDown") (e.preventDefault(), set(items[Math.min(items.length - 1, i + 1)]));
        if (e.key === "ArrowUp") (e.preventDefault(), set(items[Math.max(0, i - 1)]));
      }}
      className="h-56 overflow-y-auto scroll-thin px-1 py-1 outline-none focus-visible:ring-2 focus-visible:ring-accent/40 rounded-lg"
    >
      {items.map((n) => (
        <button
          key={n}
          type="button"
          tabIndex={-1}
          role="option"
          aria-selected={n === cur}
          onClick={() => set(n)}
          className={cx("block w-12 h-8 rounded-lg text-sm tabular-nums", n === cur ? "bg-accent text-accent-fg font-medium" : "hover:bg-panel-2")}
        >
          {pad(n)}
        </button>
      ))}
    </div>
  );
  useEffect(() => {
    hRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
    mRef.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [h, m]);
  return (
    <div className="flex gap-1 p-2">
      {col(hRef, hours, h, (n) => onChange(`${pad(n)}:${pad(m)}`), "Hour")}
      <div className="w-px bg-border my-2" />
      {col(mRef, minutes, m, (n) => onChange(`${pad(h)}:${pad(n)}`), "Minute")}
    </div>
  );
}

function PickerTrigger({
  anchor,
  open,
  onClick,
  icon,
  text,
  placeholder,
  className,
  disabled,
  ariaLabel,
  onClear,
}: {
  anchor: React.RefObject<HTMLButtonElement | null>;
  open: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  text: string | null;
  placeholder: string;
  className?: string;
  disabled?: boolean;
  ariaLabel?: string;
  onClear?: () => void;
}) {
  return (
    <div className={cx("relative", width(className), className?.match(/(^|\s)(w-\S+|max-w-\S+|flex-1)/g)?.join(" "))}>
      <button
        ref={anchor}
        type="button"
        disabled={disabled}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={onClick}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") (e.preventDefault(), onClick());
        }}
        className={cx(
          fieldBase,
          "w-full",
          height(className),
          "inline-flex items-center gap-2 px-3 text-left",
          open && "ring-2 ring-accent/40 border-accent",
          className?.replace(/(^|\s)(w-\S+|max-w-\S+|flex-1)/g, " "),
        )}
      >
        <span className="text-muted shrink-0">{icon}</span>
        <span className={cx("flex-1 min-w-0 truncate tabular-nums", !text && "text-muted/80")}>{text ?? placeholder}</span>
        {!onClear && <ChevronDown className={cx("size-4 shrink-0 text-muted transition-transform", open && "rotate-180")} />}
      </button>
      {onClear && text && !disabled && (
        <button type="button" className="absolute right-2 top-1/2 -translate-y-1/2 p-1 rounded text-muted hover:text-text" aria-label="Clear" onClick={onClear}>
          <X className="size-3.5" />
        </button>
      )}
    </div>
  );
}

function PickerFooter({ children }: { children: React.ReactNode }) {
  return <div className="flex items-center justify-between gap-2 border-t border-border px-3 py-2">{children}</div>;
}
const footBtn = "text-[13px] px-2 py-1 rounded-md hover:bg-panel-2";

/** Calendar date picker. Value is `YYYY-MM-DD` ("" when empty). */
export function DatePicker({
  value,
  onChange,
  min,
  max,
  placeholder = "Pick a date",
  className,
  disabled,
  clearable,
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  min?: string;
  max?: string;
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  clearable?: boolean;
  "aria-label"?: string;
}) {
  const po = usePopover<HTMLButtonElement>({ minWidth: 296, matchWidth: false, prefHeight: 360 });
  const d = parseYmd(value);
  const [month, setMonth] = useState(() => d ?? new Date());
  useEffect(() => {
    if (po.open) setMonth(d ?? new Date());
  }, [po.open]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (po.open) requestAnimationFrame(() => po.pop.current?.querySelector<HTMLElement>('[role="gridcell"][tabindex="0"]')?.focus());
  }, [po.open]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <>
      <PickerTrigger
        anchor={po.anchor}
        open={po.open}
        onClick={() => po.setOpen(!po.open)}
        icon={<Calendar className="size-4" />}
        text={d ? fmtDate(d) : null}
        placeholder={placeholder}
        className={className}
        disabled={disabled}
        ariaLabel={ariaLabel}
        onClear={clearable ? () => onChange("") : undefined}
      />
      <Popover state={po} label={ariaLabel ?? "Choose date"}>
        <MonthGrid
          month={month}
          onMonth={setMonth}
          isSelected={(x) => sameDay(x, d)}
          min={parseYmd(min)}
          max={parseYmd(max)}
          onPick={(x) => {
            onChange(ymd(x));
            po.close();
          }}
        />
        <PickerFooter>
          <button type="button" className={cx(footBtn, "text-accent")} onClick={() => (onChange(ymd(new Date())), po.close())}>
            Today
          </button>
          <button type="button" className={cx(footBtn, "text-muted")} onClick={() => po.close()}>
            Close
          </button>
        </PickerFooter>
      </Popover>
    </>
  );
}

/** 24-hour time picker. Value is `HH:mm`. */
export function TimePicker({
  value,
  onChange,
  minuteStep = 1,
  placeholder = "Pick a time",
  className,
  disabled,
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  minuteStep?: number;
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
}) {
  const po = usePopover<HTMLButtonElement>({ minWidth: 150, matchWidth: false, prefHeight: 300 });
  const ok = /^\d{1,2}:\d{2}/.test(value);
  return (
    <>
      <PickerTrigger
        anchor={po.anchor}
        open={po.open}
        onClick={() => po.setOpen(!po.open)}
        icon={<Clock className="size-4" />}
        text={ok ? value.slice(0, 5) : null}
        placeholder={placeholder}
        className={className}
        disabled={disabled}
        ariaLabel={ariaLabel}
      />
      <Popover state={po} label={ariaLabel ?? "Choose time"}>
        <TimeColumns value={value} onChange={onChange} minuteStep={minuteStep} />
        <PickerFooter>
          <button
            type="button"
            className={cx(footBtn, "text-accent")}
            onClick={() => {
              const n = new Date();
              onChange(`${pad(n.getHours())}:${pad(n.getMinutes())}`);
            }}
          >
            Now
          </button>
          <button type="button" className={cx(footBtn, "font-medium")} onClick={() => po.close()}>
            Done
          </button>
        </PickerFooter>
      </Popover>
    </>
  );
}

/** Date and time picker. Value is `YYYY-MM-DDTHH:mm` (like a datetime-local input). */
export function DateTimePicker({
  value,
  onChange,
  min,
  placeholder = "Pick a date and time",
  className,
  disabled,
  minuteStep = 1,
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  min?: string;
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  minuteStep?: number;
  "aria-label"?: string;
}) {
  const po = usePopover<HTMLButtonElement>({ minWidth: 300, matchWidth: false, prefHeight: 420 });
  const d = parseYmd(value);
  const time = /T(\d{2}:\d{2})/.exec(value ?? "")?.[1] ?? "09:00";
  const [month, setMonth] = useState(() => d ?? new Date());
  useEffect(() => {
    if (po.open) setMonth(d ?? new Date());
  }, [po.open]); // eslint-disable-line react-hooks/exhaustive-deps
  const text = d ? `${fmtDate(d)}, ${time}` : null;
  const setDate = (x: Date) => onChange(`${ymd(x)}T${time}`);
  return (
    <>
      <PickerTrigger
        anchor={po.anchor}
        open={po.open}
        onClick={() => po.setOpen(!po.open)}
        icon={<Calendar className="size-4" />}
        text={text}
        placeholder={placeholder}
        className={className}
        disabled={disabled}
        ariaLabel={ariaLabel}
      />
      <Popover state={po} label={ariaLabel ?? "Choose date and time"} className="sm:!w-auto">
        <div className="flex flex-col sm:flex-row overflow-y-auto scroll-thin">
          <div className="sm:w-[296px] shrink-0">
            <MonthGrid month={month} onMonth={setMonth} isSelected={(x) => sameDay(x, d)} min={parseYmd(min)} onPick={setDate} />
          </div>
          <div className="border-t sm:border-t-0 sm:border-l border-border flex flex-col items-center">
            <div className="pt-3 text-xs font-medium text-muted flex items-center gap-1">
              <Clock className="size-3.5" /> Time
            </div>
            <TimeColumns value={time} minuteStep={minuteStep} onChange={(t) => onChange(`${ymd(d ?? new Date())}T${t}`)} />
          </div>
        </div>
        <PickerFooter>
          <button
            type="button"
            className={cx(footBtn, "text-accent")}
            onClick={() => {
              const n = new Date();
              onChange(`${ymd(n)}T${pad(n.getHours())}:${pad(n.getMinutes())}`);
            }}
          >
            Now
          </button>
          <button type="button" className={cx(footBtn, "font-medium")} onClick={() => po.close()}>
            Done
          </button>
        </PickerFooter>
      </Popover>
    </>
  );
}

/** Pick a from–to day range (inclusive), as `YYYY-MM-DD` strings. */
export function DateRangePicker({
  from,
  to,
  onChange,
  max,
  className,
  disabled,
  placeholder = "Pick dates",
  "aria-label": ariaLabel,
}: {
  from: string;
  to: string;
  onChange: (from: string, to: string) => void;
  max?: string;
  className?: string;
  disabled?: boolean;
  placeholder?: string;
  "aria-label"?: string;
}) {
  const po = usePopover<HTMLButtonElement>({ minWidth: 296, matchWidth: false, prefHeight: 380 });
  const a = parseYmd(from);
  const b = parseYmd(to);
  const [month, setMonth] = useState(() => b ?? new Date());
  const [pending, setPending] = useState<Date | null>(null);
  const [hover, setHover] = useState<Date | null>(null);
  useEffect(() => {
    if (po.open) {
      setMonth(a ?? new Date());
      setPending(null);
    }
  }, [po.open]); // eslint-disable-line react-hooks/exhaustive-deps
  const lo = pending ? (hover && hover < pending ? hover : pending) : a;
  const hi = pending ? (hover && hover > pending ? hover : pending) : b;
  const text = a && b ? rangeText(a, b) : null;
  return (
    <>
      <PickerTrigger
        anchor={po.anchor}
        open={po.open}
        onClick={() => po.setOpen(!po.open)}
        icon={<Calendar className="size-4" />}
        text={text}
        placeholder={placeholder}
        className={className}
        disabled={disabled}
        ariaLabel={ariaLabel}
      />
      <Popover state={po} label={ariaLabel ?? "Choose date range"}>
        <div className="px-3 pt-3 text-xs text-muted">{pending ? "Now pick the last day" : "Pick the first day"}</div>
        <MonthGrid
          month={month}
          onMonth={setMonth}
          max={parseYmd(max)}
          isSelected={(x) => sameDay(x, lo) || sameDay(x, hi)}
          inRange={(x) => !!lo && !!hi && x > lo && x < hi}
          onHover={setHover}
          onPick={(x) => {
            if (!pending) {
              setPending(x);
              return;
            }
            const [s, e] = x < pending ? [x, pending] : [pending, x];
            onChange(ymd(s), ymd(e));
            setPending(null);
            po.close();
          }}
        />
        <PickerFooter>
          <span className="text-xs text-muted truncate">{lo && hi ? rangeText(lo, hi) : ""}</span>
          <button type="button" className={cx(footBtn, "text-muted")} onClick={() => po.close()}>
            Close
          </button>
        </PickerFooter>
      </Popover>
    </>
  );
}
