import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Archive, MessageSquare } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { AgentPicker, type AgentConfig } from "../../components/AgentPicker";
import { Composer } from "../../components/Composer";
import { Button, Card, Empty, Toggle, useToast } from "../../components/ui";
import { api } from "../../lib/api";
import type { Chat, Project } from "../../lib/types";
import { ChatRow } from "../Dashboard";

const LAST = "cloud-ai:last-agent";

function loadLast(): AgentConfig {
  try {
    const v = JSON.parse(localStorage.getItem(LAST) || "");
    if (v.tool) return v;
  } catch {
    /* ignore */
  }
  return { tool: "claude", model: "", effort: "", permission_mode: "bypass" };
}

export default function ChatsTab({ project }: { project: Project }) {
  const [cfg, setCfg] = useState<AgentConfig>(loadLast);
  const [worktree, setWorktree] = useState(false);
  const [autoDeploy, setAutoDeploy] = useState(false);
  const [archived, setArchived] = useState(false);
  const chats = useQuery({
    queryKey: ["chats", project.id, archived],
    queryFn: () => api.get<Chat[]>(`/api/projects/${project.id}/chats${archived ? "?archived=true" : ""}`),
  });
  const nav = useNavigate();
  const toast = useToast();
  const qc = useQueryClient();

  const start = async (prompt: string, attachments: { id: number }[]) => {
    try {
      localStorage.setItem(LAST, JSON.stringify(cfg));
    } catch {
      /* ignore */
    }
    try {
      const chat = await api.post<Chat>(`/api/projects/${project.id}/chats`, {
        ...cfg,
        prompt,
        attachments: attachments.map((a) => a.id),
        worktree,
        auto_deploy: autoDeploy,
      });
      qc.invalidateQueries({ queryKey: ["chats", project.id] });
      nav(`/chats/${chat.id}`);
      return true;
    } catch (e: any) {
      toast(e.message, "danger");
      return false;
    }
  };

  return (
    <div className="space-y-5">
      <Card className="p-4 space-y-3">
        <div className="text-sm font-medium">New chat</div>
        <AgentPicker v={cfg} set={setCfg} />
        <Composer onSend={start} draftKey={`draft:new:${project.id}`} placeholder={`What should ${cfg.tool === "codex" ? "Codex" : "Claude"} do in ${project.name}?`} />
        <div className="flex flex-wrap gap-x-6">
          {project.is_git && (
            <Toggle checked={worktree} onChange={setWorktree} label="Separate git worktree" hint="Work on its own branch; merge when done" />
          )}
          {project.deployable && <Toggle checked={autoDeploy} onChange={setAutoDeploy} label="Deploy when finished" />}
        </div>
      </Card>
      <Card>
        <div className="flex items-center justify-between px-4 py-2.5 border-b border-border">
          <div className="text-sm font-medium">{archived ? "Archived chats" : "Chats"}</div>
          <Button size="sm" variant="ghost" icon={<Archive className="size-4" />} onClick={() => setArchived(!archived)}>
            {archived ? "Show active" : "Archived"}
          </Button>
        </div>
        {chats.data?.length ? (
          <div className="divide-y divide-border">
            {chats.data.map((c) => (
              <ChatRow key={c.id} c={c} />
            ))}
          </div>
        ) : (
          <Empty icon={<MessageSquare className="size-8" />} title={archived ? "No archived chats" : "No chats yet"} />
        )}
      </Card>
    </div>
  );
}
