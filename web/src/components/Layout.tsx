import { useQuery } from "@tanstack/react-query";
import {
  Bot,
  CalendarClock,
  ChartColumn,
  FolderGit2,
  LayoutDashboard,
  LogOut,
  Menu,
  Puzzle,
  ScrollText,
  Server,
  Settings,
  SquareTerminal,
  UserRound,
  Users,
  WifiOff,
  X,
} from "lucide-react";
import { Suspense, useEffect, useState } from "react";
import { NavLink, Outlet, useLocation } from "react-router";
import { api } from "../lib/api";
import { useAuth } from "../lib/auth";
import { useLiveStatus, useOnline } from "../lib/live";
import type { Project } from "../lib/types";
import { PageErrorBoundary } from "./Loadable";
import { cx, Spinner, StatusDot } from "./ui";

const main = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard, end: true },
  { to: "/projects", label: "Projects", icon: FolderGit2 },
  { to: "/schedules", label: "Schedules", icon: CalendarClock },
  { to: "/usage", label: "Usage", icon: ChartColumn },
  { to: "/extensions", label: "Skills & plugins", icon: Puzzle },
];

const admin = [
  { to: "/terminal", label: "Terminal", icon: SquareTerminal },
  { to: "/admin/cli", label: "Claude & Codex", icon: Bot },
  { to: "/admin/users", label: "Users", icon: Users },
  { to: "/admin/servers", label: "Servers", icon: Server },
  { to: "/admin/settings", label: "Settings", icon: Settings },
  { to: "/admin/events", label: "Activity log", icon: ScrollText },
];

function NavItem({ to, label, icon: Icon, end, onClick }: { to: string; label: string; icon: any; end?: boolean; onClick?: () => void }) {
  return (
    <NavLink
      to={to}
      end={end}
      onClick={onClick}
      className={({ isActive }) =>
        cx(
          "flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm transition",
          isActive ? "bg-accent-soft text-accent font-medium" : "text-muted hover:bg-panel-2 hover:text-text",
        )
      }
    >
      <Icon className="size-4 shrink-0" />
      <span className="truncate">{label}</span>
    </NavLink>
  );
}

function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  const { user, logout, version } = useAuth();
  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api.get<Project[]>("/api/projects") });
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 px-4 h-14 shrink-0">
        <img src="/favicon.svg" className="size-7" alt="" />
        <span className="font-semibold tracking-tight">cloud-ai</span>
        <span className="text-[11px] text-muted ml-auto">v{version}</span>
      </div>
      <nav className="flex-1 overflow-y-auto scroll-thin px-2 space-y-0.5">
        {main.map((n) => (
          <NavItem key={n.to} {...n} onClick={onNavigate} />
        ))}
        {(projects.data?.length ?? 0) > 0 && (
          <div className="pt-4">
            <div className="px-2.5 pb-1 text-[11px] font-semibold uppercase tracking-wider text-muted">Projects</div>
            {projects.data!.map((p) => (
              <NavLink
                key={p.id}
                to={`/projects/${p.id}`}
                onClick={onNavigate}
                className={({ isActive }) =>
                  cx(
                    "flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm",
                    isActive ? "bg-panel-2 text-text" : "text-muted hover:bg-panel-2 hover:text-text",
                  )
                }
              >
                <StatusDot status={(p.active_count ?? 0) > 0 ? "running" : "idle"} />
                <span className="truncate">{p.name}</span>
              </NavLink>
            ))}
          </div>
        )}
        {user.role === "admin" && (
          <div className="pt-4 space-y-0.5">
            <div className="px-2.5 pb-1 text-[11px] font-semibold uppercase tracking-wider text-muted">Admin</div>
            {admin.map((n) => (
              <NavItem key={n.to} {...n} onClick={onNavigate} />
            ))}
          </div>
        )}
      </nav>
      <div className="border-t border-border p-2 space-y-0.5">
        <NavItem to="/account" label={user.username} icon={UserRound} onClick={onNavigate} />
        <button onClick={logout} className="w-full flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-sm text-muted hover:bg-panel-2 hover:text-text">
          <LogOut className="size-4" /> Log out
        </button>
      </div>
    </div>
  );
}

export default function Layout() {
  const [open, setOpen] = useState(false);
  const loc = useLocation();
  const up = useLiveStatus();
  const online = useOnline();
  // Only show "reconnecting" when it lasts, so a quick reconnect doesn't flash a banner.
  const [showDown, setShowDown] = useState(false);
  useEffect(() => {
    if (up) return setShowDown(false);
    const t = setTimeout(() => setShowDown(true), 2500);
    return () => clearTimeout(t);
  }, [up]);
  const isChat = loc.pathname.startsWith("/chats/") || loc.pathname === "/terminal";
  useEffect(() => setOpen(false), [loc.pathname]);
  return (
    <div className="h-full flex">
      <aside className="hidden lg:block w-60 shrink-0 border-r border-border bg-panel">
        <Sidebar />
      </aside>
      {open && (
        <div className="lg:hidden fixed inset-0 z-40 bg-black/50" onClick={() => setOpen(false)}>
          <aside className="absolute inset-y-0 left-0 w-72 bg-panel border-r border-border" onClick={(e) => e.stopPropagation()}>
            <button className="absolute right-2 top-3 p-2 text-muted" onClick={() => setOpen(false)} aria-label="Close menu">
              <X className="size-5" />
            </button>
            <Sidebar onNavigate={() => setOpen(false)} />
          </aside>
        </div>
      )}
      <div className="flex-1 min-w-0 flex flex-col">
        <header className="lg:hidden flex items-center gap-2 h-12 px-2 border-b border-border bg-panel shrink-0">
          <button className="p-2 text-muted" onClick={() => setOpen(true)} aria-label="Open menu">
            <Menu className="size-5" />
          </button>
          <img src="/favicon.svg" className="size-6" alt="" />
          <span className="font-semibold">cloud-ai</span>
        </header>
        {(!online || showDown) && (
          <div role="status" className="flex items-center gap-2 justify-center bg-warn-soft text-warn text-xs py-1 px-3 text-center">
            <WifiOff className="size-3.5 shrink-0" />
            {!online ? "You're offline. Pages show the last loaded data and refresh when you're back online." : "Reconnecting to the server…"}
          </div>
        )}
        <main className={cx("flex-1 min-h-0", isChat ? "flex flex-col" : "overflow-y-auto scroll-thin")}>
          <PageErrorBoundary resetKey={loc.pathname}>
            {isChat ? (
              <Suspense fallback={<Spinner page />}>
                <Outlet />
              </Suspense>
            ) : (
              <div className="mx-auto max-w-6xl px-4 py-5 lg:px-8 lg:py-7 pb-24 lg:pb-8">
                <Suspense fallback={<Spinner page />}>
                  <Outlet />
                </Suspense>
              </div>
            )}
          </PageErrorBoundary>
        </main>
        {!isChat && (
          <nav className="lg:hidden fixed bottom-0 inset-x-0 z-30 border-t border-border bg-panel safe-bottom">
            <div className="grid grid-cols-4">
              {[...main.slice(0, 3), { to: "/account", label: "Account", icon: UserRound }].map((n) => (
                <NavLink
                  key={n.to}
                  to={n.to}
                  end={(n as any).end}
                  className={({ isActive }) => cx("flex flex-col items-center gap-0.5 py-2 text-[11px]", isActive ? "text-accent" : "text-muted")}
                >
                  <n.icon className="size-5" />
                  {n.label}
                </NavLink>
              ))}
            </div>
          </nav>
        )}
      </div>
    </div>
  );
}
