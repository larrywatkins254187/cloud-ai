import { LogOut, Monitor, Moon, Sun } from "lucide-react";
import { useState } from "react";
import { Badge, Button, Card, cx, Field, Input, PageHeader, useAction } from "../components/ui";
import { api } from "../lib/api";
import { useAuth } from "../lib/auth";
import { useTheme, type ThemePref } from "../lib/theme";

export default function Account() {
  const { user, logout, version } = useAuth();
  const [theme, setTheme] = useTheme();
  const [oldPw, setOldPw] = useState("");
  const [newPw, setNewPw] = useState("");
  const { busy, run } = useAction();
  const opts: { id: ThemePref; label: string; icon: any }[] = [
    { id: "light", label: "Light", icon: Sun },
    { id: "dark", label: "Dark", icon: Moon },
    { id: "system", label: "System", icon: Monitor },
  ];
  return (
    <>
      <PageHeader
        title={user.username}
        subtitle={
          <span className="flex items-center gap-2">
            <Badge tone="accent">{user.role}</Badge> 2FA enabled · cloud-ai v{version}
          </span>
        }
      />
      <div className="space-y-4 max-w-xl">
        <Card className="p-5 space-y-3">
          <div className="font-medium">Appearance</div>
          <div className="grid grid-cols-3 gap-2">
            {opts.map((o) => (
              <button
                key={o.id}
                onClick={() => setTheme(o.id)}
                className={cx(
                  "flex flex-col items-center gap-1.5 rounded-xl border py-3 text-sm",
                  theme === o.id ? "border-accent bg-accent-soft text-accent" : "border-border text-muted",
                )}
              >
                <o.icon className="size-5" />
                {o.label}
              </button>
            ))}
          </div>
        </Card>
        <Card className="p-5 space-y-3">
          <div className="font-medium">Change password</div>
          <Field label="Current password">
            <Input type="password" value={oldPw} onChange={(e) => setOldPw(e.target.value)} autoComplete="current-password" />
          </Field>
          <Field label="New password">
            <Input type="password" value={newPw} onChange={(e) => setNewPw(e.target.value)} autoComplete="new-password" />
          </Field>
          <Button
            variant="primary"
            loading={busy}
            disabled={!oldPw || newPw.length < 8}
            onClick={async () => {
              const r = await run(() => api.post("/api/auth/password", { old_password: oldPw, new_password: newPw }), "Password changed");
              if (r) {
                setOldPw("");
                setNewPw("");
              }
            }}
          >
            Update password
          </Button>
        </Card>
        <Button variant="outline" icon={<LogOut className="size-4" />} onClick={logout}>
          Log out
        </Button>
      </div>
    </>
  );
}
