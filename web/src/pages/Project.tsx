import { useQuery } from "@tanstack/react-query";
import { FileCode2, GitBranch, Globe, History, MessageSquare, Rocket, Settings } from "lucide-react";
import { useParams, useSearchParams } from "react-router";
import { Badge, PageHeader, Spinner, Tabs } from "../components/ui";
import { api } from "../lib/api";
import { useIsAdmin } from "../lib/auth";
import type { Project as P } from "../lib/types";
import ChatsTab from "./project/ChatsTab";
import DeployTab from "./project/DeployTab";
import FilesTab from "./project/FilesTab";
import GitTab from "./project/GitTab";
import HistoryTab from "./project/HistoryTab";
import PreviewTab from "./project/PreviewTab";
import SettingsTab from "./project/SettingsTab";

type Tab = "chats" | "files" | "git" | "deploy" | "history" | "preview" | "settings";

export default function Project() {
  const id = Number(useParams().id);
  const [sp, setSp] = useSearchParams();
  const tab = (sp.get("tab") as Tab) || "chats";
  const admin = useIsAdmin();
  const q = useQuery({ queryKey: ["project", id], queryFn: () => api.get<P>(`/api/projects/${id}`) });
  if (q.isLoading) return <Spinner page />;
  if (q.error || !q.data) return <div className="text-danger text-sm">{(q.error as Error)?.message ?? "Not found"}</div>;
  const p = q.data;
  const tabs: { id: Tab; label: string; icon: React.ReactNode }[] = [
    { id: "chats", label: "Chats", icon: <MessageSquare className="size-4" /> },
    { id: "files", label: "Files", icon: <FileCode2 className="size-4" /> },
    { id: "git", label: "Git", icon: <GitBranch className="size-4" /> },
    { id: "deploy", label: "Deploy", icon: <Rocket className="size-4" /> },
    { id: "history", label: "Deploy history", icon: <History className="size-4" /> },
    { id: "preview", label: "Preview", icon: <Globe className="size-4" /> },
  ];
  if (admin) tabs.push({ id: "settings", label: "Settings", icon: <Settings className="size-4" /> });
  return (
    <>
      <PageHeader
        title={p.name}
        subtitle={
          <span className="flex flex-wrap items-center gap-2">
            <code className="text-xs">{p.path}</code>
            {!p.exists && <Badge tone="danger">folder missing</Badge>}
            {p.is_git && <Badge>git</Badge>}
            {p.deploy_target === "remote" && <Badge tone="ok">deploys over SSH</Badge>}
          </span>
        }
      />
      <div className="mb-4">
        <Tabs<Tab> tabs={tabs} value={tab} onChange={(t) => setSp(t === "chats" ? {} : { tab: t }, { replace: true })} />
      </div>
      {tab === "chats" && <ChatsTab project={p} />}
      {tab === "files" && <FilesTab project={p} />}
      {tab === "git" && <GitTab project={p} />}
      {tab === "deploy" && <DeployTab project={p} />}
      {tab === "history" && <HistoryTab project={p} />}
      {tab === "preview" && <PreviewTab project={p} />}
      {tab === "settings" && admin && <SettingsTab project={p} />}
    </>
  );
}
