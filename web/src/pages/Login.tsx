import { ShieldCheck } from "lucide-react";
import { useState } from "react";
import { Button, Card, Field, Input } from "../components/ui";
import { api } from "../lib/api";

type Stage = "setup" | "password" | "totp" | "totp_setup";

export default function Login({ setupNeeded, pendingStage, onDone }: { setupNeeded: boolean; pendingStage?: string; onDone: () => void }) {
  const [stage, setStage] = useState<Stage>(setupNeeded ? "setup" : pendingStage === "totp" ? "totp" : "password");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [token, setToken] = useState("");
  const [code, setCode] = useState("");
  const [enrol, setEnrol] = useState<{ secret: string; qr_svg: string; otpauth_url: string } | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    setBusy(true);
    try {
      if (stage === "setup") {
        await api.post("/api/setup", { token, username, password });
        setStage("password");
      } else if (stage === "password") {
        const r = await api.post<any>("/api/auth/login", { username, password });
        if (r.stage === "totp_setup") {
          setEnrol(r);
          setStage("totp_setup");
        } else setStage("totp");
      } else {
        await api.post("/api/auth/totp", { code });
        onDone();
      }
    } catch (err: any) {
      setError(err.message);
      if (err.status === 400 && (stage === "totp" || stage === "totp_setup")) setStage("password");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="min-h-full flex items-center justify-center p-4">
      <Card className="w-full max-w-sm p-6">
        <div className="flex items-center gap-2 mb-6">
          <img src="/favicon.svg" className="size-9" alt="" />
          <div>
            <div className="font-semibold text-lg leading-tight">cloud-ai</div>
            <div className="text-xs text-muted">Claude Code & Codex on your server</div>
          </div>
        </div>
        <form onSubmit={submit} className="space-y-4">
          {stage === "setup" && (
            <>
              <p className="text-sm text-muted">
                Create the first admin account. The setup token is in <code>/var/lib/cloud-ai/setup-token</code> (also printed by the installer).
              </p>
              <Field label="Setup token">
                <Input value={token} onChange={(e) => setToken(e.target.value)} autoFocus />
              </Field>
            </>
          )}
          {(stage === "setup" || stage === "password") && (
            <>
              <Field label="Username">
                <Input value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="username" autoFocus={stage === "password"} />
              </Field>
              <Field label="Password">
                <Input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoComplete={stage === "setup" ? "new-password" : "current-password"} />
              </Field>
            </>
          )}
          {stage === "totp_setup" && enrol && (
            <div className="space-y-3">
              <div className="flex items-center gap-2 text-sm font-medium">
                <ShieldCheck className="size-4 text-accent" /> Set up two-factor authentication
              </div>
              <p className="text-sm text-muted">Scan this QR code with Google Authenticator, Authy, 1Password…, then enter the 6-digit code.</p>
              <div className="bg-white rounded-lg p-2 w-fit mx-auto" dangerouslySetInnerHTML={{ __html: enrol.qr_svg }} />
              <div className="text-xs text-muted break-all text-center">
                Key: <code className="select-all">{enrol.secret}</code>
              </div>
            </div>
          )}
          {(stage === "totp" || stage === "totp_setup") && (
            <Field label="Authenticator code">
              <Input
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, "").slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder="123456"
                autoFocus
                className="text-center tracking-[0.4em] text-lg"
              />
            </Field>
          )}
          {error && <div className="text-sm text-danger">{error}</div>}
          <Button variant="primary" className="w-full" loading={busy} type="submit">
            {stage === "setup" ? "Create admin" : stage === "password" ? "Continue" : "Verify"}
          </Button>
        </form>
      </Card>
    </div>
  );
}
