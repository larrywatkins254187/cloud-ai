import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Save, Send } from "lucide-react";
import { useEffect, useState } from "react";
import { Button, Card, Field, Input, PageHeader, Select, Spinner, Textarea, Toggle, useAction, useToast } from "../../components/ui";
import { api } from "../../lib/api";
import { MODES } from "../../lib/format";

type S = Record<string, string>;

export default function Settings() {
  const q = useQuery({ queryKey: ["settings"], queryFn: () => api.get<S>("/api/settings") });
  const [v, setV] = useState<S>({});
  const { busy, run } = useAction();
  const toast = useToast();
  const qc = useQueryClient();
  useEffect(() => {
    if (q.data) setV(q.data);
  }, [q.data]);
  if (!q.data) return <Spinner />;
  const up = (k: string, val: string) => setV({ ...v, [k]: val });
  const bool = (k: string) => v[k] === "1" || v[k] === "true";
  const save = async () => {
    const r = await run(() => api.put<S>("/api/settings", v), "Settings saved");
    if (r) qc.setQueryData(["settings"], r);
  };
  return (
    <>
      <PageHeader
        title="Settings"
        actions={
          <Button variant="primary" icon={<Save className="size-4" />} loading={busy} onClick={save}>
            Save
          </Button>
        }
      />
      <div className="space-y-4 max-w-3xl">
        <Card className="p-5 space-y-4">
          <div className="font-medium">Resources</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Max parallel Claude Code sessions">
              <Input type="number" min={1} value={v.max_claude ?? ""} onChange={(e) => up("max_claude", e.target.value)} />
            </Field>
            <Field label="Max parallel Codex sessions">
              <Input type="number" min={1} value={v.max_codex ?? ""} onChange={(e) => up("max_codex", e.target.value)} />
            </Field>
            <Field label="Minimum free RAM to start a prompt (MB)" hint="Below this, prompts wait in the queue (swap is not counted)">
              <Input type="number" value={v.ram_guard_mb ?? ""} onChange={(e) => up("ram_guard_mb", e.target.value)} />
            </Field>
            <Field label="Stop idle agent processes after (minutes)" hint="The conversation resumes automatically on the next prompt">
              <Input type="number" min={1} value={v.idle_kill_minutes ?? ""} onChange={(e) => up("idle_kill_minutes", e.target.value)} />
            </Field>
          </div>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="font-medium">Agents</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Default permission mode for new chats">
              <Select value={v.default_permission_mode} onChange={(e) => up("default_permission_mode", e.target.value)}>
                {MODES.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.label} — {m.hint}
                  </option>
                ))}
              </Select>
            </Field>
            <Field label="Timezone for schedules">
              <Input value={v.timezone ?? ""} onChange={(e) => up("timezone", e.target.value)} placeholder="Asia/Tehran" />
            </Field>
            <Field label="claude binary">
              <Input value={v.claude_path ?? ""} onChange={(e) => up("claude_path", e.target.value)} />
            </Field>
            <Field label="codex binary">
              <Input value={v.codex_path ?? ""} onChange={(e) => up("codex_path", e.target.value)} />
            </Field>
          </div>
          <Field label="Extra instructions for every agent" hint="Appended to the system prompt of Claude Code and Codex">
            <Textarea value={v.extra_system_prompt ?? ""} onChange={(e) => up("extra_system_prompt", e.target.value)} />
          </Field>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="flex items-center justify-between">
            <div className="font-medium">Telegram</div>
            <Button
              size="sm"
              variant="outline"
              icon={<Send className="size-4" />}
              onClick={async () => {
                await run(() => api.put("/api/settings", v));
                const r = await run(() => api.post<any>("/api/telegram/test"));
                if (r) r.ok ? toast(`Connected as @${r.bot.username}`, "ok") : toast(r.error, "danger");
              }}
            >
              Save & test
            </Button>
          </div>
          <ol className="text-xs text-muted list-decimal pl-4 space-y-0.5">
            <li>Create a bot with @BotFather and paste its token below.</li>
            <li>Create a group, enable Topics in its settings, add the bot and make it admin (Manage topics).</li>
            <li>Send /id in the group and paste the chat id below (it starts with -100).</li>
          </ol>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Bot token">
              <Input value={v.telegram_token ?? ""} onChange={(e) => up("telegram_token", e.target.value)} placeholder="123456:ABC…" />
            </Field>
            <Field label="Group chat id">
              <Input value={v.telegram_chat_id ?? ""} onChange={(e) => up("telegram_chat_id", e.target.value)} placeholder="-1001234567890" />
            </Field>
          </div>
          <Field label="Telegram user ids allowed to answer and deploy" hint="Comma separated; empty = anyone in the group">
            <Input value={v.telegram_allowed_users ?? ""} onChange={(e) => up("telegram_allowed_users", e.target.value)} />
          </Field>
          <div className="flex flex-wrap gap-x-6">
            <Toggle checked={bool("telegram_send_prompts")} onChange={(b) => up("telegram_send_prompts", b ? "1" : "0")} label="Send every prompt" />
            <Toggle checked={bool("telegram_send_results")} onChange={(b) => up("telegram_send_results", b ? "1" : "0")} label="Include the final answer" />
          </div>
          <Field label="Public URL of this panel" hint="Used for 'Open in panel' links">
            <Input value={v.public_url ?? ""} onChange={(e) => up("public_url", e.target.value)} placeholder="https://ai.example.com" />
          </Field>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="font-medium">Updates</div>
          <Toggle
            checked={bool("auto_update_enabled")}
            onChange={(b) => up("auto_update_enabled", b ? "1" : "0")}
            label="Update Claude Code and Codex automatically"
            hint="Only when no session is running; retried every 15 minutes until idle"
          />
          <Field label="Daily update hour (0–23, panel timezone)">
            <Input type="number" min={0} max={23} value={v.update_hour ?? ""} onChange={(e) => up("update_hour", e.target.value)} className="w-28" />
          </Field>
          <Toggle checked={bool("app_auto_update")} onChange={(b) => up("app_auto_update", b ? "1" : "0")} label="Update cloud-ai itself from GitHub releases" />
          <Field label="Release repository">
            <Input value={v.app_update_repo ?? ""} onChange={(e) => up("app_update_repo", e.target.value)} />
          </Field>
        </Card>

        <Card className="p-5 space-y-4">
          <div className="font-medium">Live browser</div>
          <div className="grid sm:grid-cols-2 gap-4">
            <Field label="Chromium binary" hint="Empty = find chromium / google-chrome automatically">
              <Input value={v.browser_path ?? ""} onChange={(e) => up("browser_path", e.target.value)} />
            </Field>
            <Field label="Close the browser after idle (minutes)">
              <Input type="number" min={1} value={v.browser_idle_minutes ?? ""} onChange={(e) => up("browser_idle_minutes", e.target.value)} />
            </Field>
          </div>
        </Card>
      </div>
    </>
  );
}
