import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { Navigate, Route, Routes } from "react-router";
import Layout from "./components/Layout";
import { Spinner, useToast } from "./components/ui";
import { api, setUnauthorizedHandler } from "./lib/api";
import { AuthCtx } from "./lib/auth";
import { useLiveUpdates } from "./lib/live";
import type { User } from "./lib/types";
import Account from "./pages/Account";
import Chat from "./pages/Chat";
import Dashboard from "./pages/Dashboard";
import Login from "./pages/Login";
import Project from "./pages/Project";
import Projects from "./pages/Projects";
import Schedules from "./pages/Schedules";
import Terminal from "./pages/Terminal";
import Cli from "./pages/admin/Cli";
import Events from "./pages/admin/Events";
import Extensions from "./pages/admin/Extensions";
import Servers from "./pages/admin/Servers";
import Settings from "./pages/admin/Settings";
import Users from "./pages/admin/Users";

type Me = { user: User | null; version?: string; stage?: string; setup_needed?: boolean };

export default function App() {
  const qc = useQueryClient();
  const me = useQuery({ queryKey: ["me"], queryFn: () => api.get<Me>("/api/auth/me"), staleTime: 60_000 });
  const refresh = useCallback(() => qc.invalidateQueries({ queryKey: ["me"] }), [qc]);

  useEffect(() => {
    setUnauthorizedHandler(() => qc.setQueryData(["me"], { user: null }));
  }, [qc]);

  if (me.isLoading) {
    return (
      <div className="h-full flex items-center justify-center">
        <Spinner className="size-6" />
      </div>
    );
  }
  const data = me.data;
  if (!data?.user) {
    return <Login setupNeeded={!!data?.setup_needed} pendingStage={data?.stage} onDone={refresh} />;
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
        <Route path="admin/extensions" element={<Extensions />} />
        <Route path="admin/events" element={<Events />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Route>
    </Routes>
  );
}
