import { useQuery, useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useCallback, useEffect } from "react";
import { Navigate, Route, Routes } from "react-router";
import Layout from "./components/Layout";
import { ErrorState } from "./components/Loadable";
import { Spinner, useToast } from "./components/ui";
import { api, setUnauthorizedHandler } from "./lib/api";
import { AuthCtx } from "./lib/auth";
import { useLiveUpdates } from "./lib/live";
import type { User } from "./lib/types";
import Dashboard from "./pages/Dashboard";

// Pages load on first visit; the dashboard ships with the main bundle.
const Account = lazy(() => import("./pages/Account"));
const Chat = lazy(() => import("./pages/Chat"));
const Login = lazy(() => import("./pages/Login"));
const Project = lazy(() => import("./pages/Project"));
const Projects = lazy(() => import("./pages/Projects"));
const Schedules = lazy(() => import("./pages/Schedules"));
const Terminal = lazy(() => import("./pages/Terminal"));
const Cli = lazy(() => import("./pages/admin/Cli"));
const Events = lazy(() => import("./pages/admin/Events"));
const Extensions = lazy(() => import("./pages/admin/Extensions"));
const Servers = lazy(() => import("./pages/admin/Servers"));
const Settings = lazy(() => import("./pages/admin/Settings"));
const Users = lazy(() => import("./pages/admin/Users"));

type Me = { user: User | null; version?: string; stage?: string; setup_needed?: boolean };

function PageSpinner() {
  return (
    <div className="h-full flex items-center justify-center">
      <Spinner inline className="size-6" />
    </div>
  );
}

export default function App() {
  const qc = useQueryClient();
  const me = useQuery({ queryKey: ["me"], queryFn: () => api.get<Me>("/api/auth/me"), staleTime: 60_000 });
  const refresh = useCallback(() => qc.invalidateQueries({ queryKey: ["me"] }), [qc]);

  useEffect(() => {
    setUnauthorizedHandler(() => qc.setQueryData(["me"], { user: null }));
  }, [qc]);

  if (me.data === undefined && !me.isError) return <PageSpinner />;
  if (me.data === undefined) {
    return (
      <div className="h-full flex items-center justify-center">
        <ErrorState error={me.error} onRetry={me.refetch} retrying={me.isFetching} />
      </div>
    );
  }
  const data = me.data;
  if (!data?.user) {
    return (
      <Suspense fallback={<PageSpinner />}>
        <Login setupNeeded={!!data?.setup_needed} pendingStage={data?.stage} onDone={refresh} />
      </Suspense>
    );
  }
  const logout = async () => {
    await api.post("/api/auth/logout").catch(() => {});
    qc.clear();
    qc.setQueryData(["me"], { user: null });
  };
  return (
    <AuthCtx.Provider value={{ user: data.user, version: data.version ?? "", refresh, logout }}>
      <LoggedIn />
    </AuthCtx.Provider>
  );
}

function LoggedIn() {
  const toast = useToast();
  const onNotice = useCallback((text: string, level: string) => toast(text, level === "error" ? "danger" : "info"), [toast]);
  useLiveUpdates(onNotice);
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Dashboard />} />
        <Route path="projects" element={<Projects />} />
        <Route path="projects/:id" element={<Project />} />
        <Route path="chats/:id" element={<Chat />} />
        <Route path="schedules" element={<Schedules />} />
        <Route path="terminal" element={<Terminal />} />
        <Route path="account" element={<Account />} />
        <Route path="admin/users" element={<Users />} />
        <Route path="admin/servers" element={<Servers />} />
        <Route path="admin/settings" element={<Settings />} />
        <Route path="admin/cli" element={<Cli />} />
        <Route path="extensions" element={<Extensions />} />
        <Route path="admin/events" element={<Events />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
