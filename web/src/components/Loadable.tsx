import { AlertTriangle, Loader2, RefreshCw, WifiOff } from "lucide-react";
import React from "react";
import { ApiError } from "../lib/api";
import { Button, cx } from "./ui";

/** The parts of a React Query result that loading/error rendering needs. */
export interface QueryLike<T> {
  data: T | undefined;
  isError: boolean;
  error: unknown;
  isFetching: boolean;
  fetchStatus?: string;
  refetch: () => unknown;
}

/** A readable message for a failed request. */
export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) {
    if (e.status >= 502 && e.status <= 504) return "The server isn't responding right now.";
    return e.message || `Request failed (${e.status})`;
  }
  if (e instanceof TypeError || (typeof navigator !== "undefined" && !navigator.onLine)) return "Can't reach the server. Check your connection.";
  return (e as Error)?.message || "Something went wrong.";
}

function isNetworkError(e: unknown) {
  return !(e instanceof ApiError) || e.status >= 502;
}

export function Skeleton({ className, style }: { className?: string; style?: React.CSSProperties }) {
  return <div className={cx("skeleton rounded-lg", className)} style={style} />;
}

/** Placeholder rows shaped like a list card. */
export function SkeletonList({ rows = 4, className }: { rows?: number; className?: string }) {
  return (
    <div role="status" aria-label="Loading" className={cx("rounded-xl border border-border bg-panel divide-y divide-border", className)}>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-3 px-4 py-3">
          <Skeleton className="size-2 rounded-full" />
          <div className="flex-1 space-y-1.5">
            <Skeleton className="h-3.5" style={{ width: `${70 - ((i * 13) % 35)}%` }} />
            <Skeleton className="h-2.5 w-1/3" />
          </div>
        </div>
      ))}
    </div>
  );
}

/** Placeholder tiles for stat/card grids. */
export function SkeletonCards({ count = 4, className }: { count?: number; className?: string }) {
  return (
    <div role="status" aria-label="Loading" className={cx("grid grid-cols-2 lg:grid-cols-4 gap-3", className)}>
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="rounded-xl border border-border bg-panel p-4 space-y-2.5">
          <Skeleton className="h-3 w-1/2" />
          <Skeleton className="h-6 w-2/3" />
          <Skeleton className="h-1.5 w-full" />
        </div>
      ))}
    </div>
  );
}

/** Placeholder cards for card grids (projects, servers…). */
export function SkeletonGrid({ count = 6 }: { count?: number }) {
  return (
    <div role="status" aria-label="Loading" className="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
      {Array.from({ length: count }, (_, i) => (
        <div key={i} className="rounded-xl border border-border bg-panel p-4 space-y-2.5">
          <Skeleton className="h-4 w-1/2" />
          <Skeleton className="h-3 w-3/4" />
          <Skeleton className="h-5 w-1/3" />
        </div>
      ))}
    </div>
  );
}

export function ErrorState({
  error,
  onRetry,
  retrying,
  compact,
  title,
}: {
  error: unknown;
  onRetry?: () => unknown;
  retrying?: boolean;
  compact?: boolean;
  title?: string;
}) {
  const net = isNetworkError(error);
  const Icon = net ? WifiOff : AlertTriangle;
  if (compact) {
    return (
      <div role="alert" className="flex items-center gap-2 rounded-lg border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger">
        <Icon className="size-4 shrink-0" />
        <span className="flex-1 min-w-0">{title ? `${title}: ` : ""}{errorMessage(error)}</span>
        {onRetry && (
          <Button size="sm" variant="ghost" loading={retrying} icon={<RefreshCw className="size-3.5" />} onClick={() => onRetry()}>
            Retry
          </Button>
        )}
      </div>
    );
  }
  return (
    <div role="alert" className="flex flex-col items-center justify-center text-center py-12 px-6">
      <div className="mb-3 rounded-full bg-danger-soft p-3 text-danger">
        <Icon className="size-6" />
      </div>
      <div className="font-medium">{title ?? (net ? "Couldn't load this" : "Something went wrong")}</div>
      <div className="mt-1 text-sm text-muted max-w-md">{errorMessage(error)}</div>
      {net && <div className="mt-1 text-xs text-muted">It will retry on its own when the connection is back.</div>}
      {onRetry && (
        <Button className="mt-4" variant="outline" loading={retrying} icon={<RefreshCw className="size-4" />} onClick={() => onRetry()}>
          Try again
        </Button>
      )}
    </div>
  );
}

/**
 * Render a query's data, or a loading placeholder until it exists, or an error
 * with Retry. Data that was already loaded stays visible when a refresh fails,
 * with a small notice. Never shows "empty" while the data is simply missing.
 */
export function Loadable<T>({
  q,
  children,
  skeleton,
  compact,
  title,
}: {
  q: QueryLike<T>;
  children: (data: T) => React.ReactNode;
  skeleton?: React.ReactNode;
  compact?: boolean;
  title?: string;
}) {
  if (q.data !== undefined) {
    return (
      <>
        {q.isError && <StaleNotice q={q} />}
        {children(q.data)}
      </>
    );
  }
  if (q.isError) return <ErrorState error={q.error} onRetry={q.refetch} retrying={q.isFetching} compact={compact} title={title} />;
  return (
    <>
      {skeleton ?? <SkeletonList />}
      {q.fetchStatus === "paused" && <div className="mt-2 text-center text-xs text-muted">Waiting for the connection…</div>}
    </>
  );
}

/**
 * For pages that need their data before rendering anything: returns the
 * loading or error view while there is no data, or null once there is.
 */
export function queryGate(q: QueryLike<unknown>, skeleton?: React.ReactNode): React.ReactElement | null {
  if (q.data !== undefined) return null;
  if (q.isError) return <ErrorState error={q.error} onRetry={q.refetch} retrying={q.isFetching} />;
  return (
    <div role="status" aria-label="Loading" className="flex flex-col items-center justify-center min-h-[50dvh] w-full gap-2">
      {skeleton ?? <Loader2 className="size-6 animate-spin text-muted" />}
      {q.fetchStatus === "paused" && <div className="text-xs text-muted">Waiting for the connection…</div>}
    </div>
  );
}

/**
 * Drop-in status for a query whose data is rendered separately: a placeholder
 * while there is no data yet, the error with Retry, or a "couldn't refresh"
 * notice over stale data. Renders nothing once data is loaded and fresh.
 */
export function QueryStatus({ q, skeleton, compact, title }: { q: QueryLike<unknown>; skeleton?: React.ReactNode; compact?: boolean; title?: string }) {
  if (q.data !== undefined) return q.isError ? <StaleNotice q={q} /> : null;
  if (q.isError) return <ErrorState error={q.error} onRetry={q.refetch} retrying={q.isFetching} compact={compact} title={title} />;
  return (
    <>
      {skeleton ?? <SkeletonList rows={3} className="border-0 rounded-none bg-transparent" />}
      {q.fetchStatus === "paused" && <div className="py-2 text-center text-xs text-muted">Waiting for the connection…</div>}
    </>
  );
}

/** "Couldn't refresh" line shown above data that is still the last good copy. */
export function StaleNotice({ q }: { q: QueryLike<unknown> }) {
  return (
    <div role="status" className="mb-3 flex items-center gap-2 rounded-lg bg-warn-soft px-3 py-1.5 text-xs text-warn">
      <WifiOff className="size-3.5 shrink-0" />
      <span className="flex-1">Couldn't refresh — showing the last loaded data. {errorMessage(q.error)}</span>
      <button className="font-medium underline-offset-2 hover:underline" onClick={() => q.refetch()}>
        Retry
      </button>
    </div>
  );
}

/** Treat several queries as one: data once all have data, error if any failed. */
export function combine<T extends unknown[]>(...qs: { [K in keyof T]: QueryLike<T[K]> }): QueryLike<T> {
  const all = qs as QueryLike<unknown>[];
  return {
    data: all.every((q) => q.data !== undefined) ? (all.map((q) => q.data) as T) : undefined,
    isError: all.some((q) => q.isError),
    error: all.find((q) => q.isError)?.error,
    isFetching: all.some((q) => q.isFetching),
    fetchStatus: all.some((q) => q.fetchStatus === "paused") ? "paused" : all.some((q) => q.fetchStatus === "fetching") ? "fetching" : "idle",
    refetch: () => all.forEach((q) => q.isError || q.data === undefined ? q.refetch() : undefined),
  };
}

/**
 * Catches render errors and failed page downloads (offline, or file names that
 * changed after a panel update) so the app shows a way out instead of a blank screen.
 */
export class PageErrorBoundary extends React.Component<{ children: React.ReactNode; resetKey?: string }, { error: unknown }> {
  state = { error: null as unknown };
  static getDerivedStateFromError(error: unknown) {
    return { error };
  }
  componentDidUpdate(prev: { resetKey?: string }) {
    if (prev.resetKey !== this.props.resetKey && this.state.error) this.setState({ error: null });
  }
  render() {
    if (!this.state.error) return this.props.children;
    const msg = String((this.state.error as Error)?.message ?? this.state.error);
    const chunk = /dynamically imported module|Importing a module script failed|Failed to fetch/i.test(msg);
    return (
      <ErrorState
        title={chunk ? "Couldn't load this page" : "This page hit an error"}
        error={chunk ? new TypeError(msg) : this.state.error}
        onRetry={() => (chunk ? location.reload() : this.setState({ error: null }))}
      />
    );
  }
}
