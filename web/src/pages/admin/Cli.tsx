import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Download, ExternalLink, LogIn, LogOut, RefreshCw, XCircle } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { Badge, Button, Card, Modal, PageHeader, Spinner, useAction } from "../../components/ui";
import { api } from "../../lib/api";
import { dateTime } from "../../lib/format";

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-4 text-sm py-1">
      <span className="text-muted">{label}</span>
      <span className="text-right">{children}</span>
    </div>
  );
}

export default function Cli() {
  const q = useQuery({ queryKey: ["cli-status"], queryFn: () => api.get<any>("/api/cli/status"), refetchInterval: 15_000 });
  const { busy, run } = useAction();
  const nav = useNavigate();
  const qc = useQueryClient();
  const [device, setDevice] = useState<{ verification_url: string; user_code: string } | null>(null);
  const [report, setReport] = useState<string[] | null>(null);
  const s = q.data;
  const reload = () => qc.invalidateQueries({ queryKey: ["cli-status"] });

  const loginClaude = async () => {
    const r = await run(() => api.post<{ terminal: string }>("/api/cli/login/claude"));
    if (r) nav(`/terminal?open=${r.terminal}`);
  };
  const loginCodex = async () => {
    const r = await run(() => api.post<any>("/api/cli/login/codex"));
    if (r) setDevice(r);
  };

  const claudeIn = s?.claude?.auth?.loggedIn;
  const codexIn = s?.codex?.auth?.loggedIn;
  // Logged-in account email: from the CLI's auth status, else the account it last reported.
  const claudeEmail = claudeIn ? (s?.claude?.auth?.email ?? s?.claude?.account?.email ?? null) : null;
  const codexEmail = codexIn ? (s?.codex?.auth?.email ?? s?.codex?.account?.account?.email ?? null) : null;
  return (
    <>
      <PageHeader
        title="Claude Code & Codex"
        subtitle="Logins, versions and updates of the CLIs on this server"
        actions={
          <>
            <Button variant="outline" icon={<RefreshCw className="size-4" />} loading={busy} onClick={async () => (await run(() => api.post("/api/cli/check")), reload())}>
              Check versions
            </Button>
            <Button
              variant="primary"
              icon={<Download className="size-4" />}
              loading={busy}
              onClick={async () => {
                const r = await run(() => api.post<{ report: string[] }>("/api/cli/update", { app: true }));
                if (r) setReport(r.report);
                reload();
              }}
            >
              Update now
            </Button>
          </>
        }
      />
      {!s ? (
        <Spinner page />
      ) : (
        <div className="grid lg:grid-cols-2 gap-4">
          <Card className="p-5 space-y-3">
            <div className="flex items-center justify-between">
              <div className="font-semibold">Claude Code</div>
              {claudeIn ? (
                <Badge tone="ok">
                  <CheckCircle2 className="size-3" /> logged in
                </Badge>
              ) : (
                <Badge tone="danger">
                  <XCircle className="size-3" /> not logged in
                </Badge>
              )}
            </div>
            <div>
              {claudeIn && <Row label="Account">{claudeEmail ? <span className="font-medium break-all">{claudeEmail}</span> : <span className="text-muted">email not reported</span>}</Row>}
              <Row label="Installed">{s.claude.version ?? "not found"}</Row>
              <Row label="Latest">{s.claude.latest ?? "—"}</Row>
              <Row label="Auth">{s.claude.auth?.authMethod ?? s.claude.auth?.error ?? "—"}</Row>
              {(s.claude.auth?.subscriptionType ?? s.claude.account?.subscriptionType) && <Row label="Plan">{s.claude.auth?.subscriptionType ?? s.claude.account.subscriptionType}</Row>}
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="primary" icon={<LogIn className="size-4" />} onClick={loginClaude}>
                Log in
              </Button>
              {claudeIn && (
                <Button size="sm" variant="outline" icon={<LogOut className="size-4" />} onClick={async () => (await run(() => api.post("/api/cli/logout/claude"), "Logged out"), reload())}>
                  Log out
                </Button>
              )}
            </div>
            <p className="text-xs text-muted">
              Log in opens a terminal running <code>claude auth login</code>: open the link it prints, approve, and paste the code back into the terminal.
            </p>
          </Card>

          <Card className="p-5 space-y-3">
            <div className="flex items-center justify-between">
              <div className="font-semibold">Codex</div>
              {codexIn ? (
                <Badge tone="ok">
                  <CheckCircle2 className="size-3" /> logged in
                </Badge>
              ) : (
                <Badge tone="danger">
                  <XCircle className="size-3" /> not logged in
                </Badge>
              )}
            </div>
            <div>
              {codexIn && <Row label="Account">{codexEmail ? <span className="font-medium break-all">{codexEmail}</span> : <span className="text-muted">email not reported</span>}</Row>}
              <Row label="Installed">{s.codex.version ?? "not found"}</Row>
              <Row label="Latest">{s.codex.latest ?? "—"}</Row>
              <Row label="Status">{s.codex.auth?.detail ?? "—"}</Row>
              {s.codex.account?.account?.planType && <Row label="Plan">{s.codex.account.account.planType}</Row>}
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="primary" icon={<LogIn className="size-4" />} onClick={loginCodex}>
                Log in with ChatGPT
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={async () => {
                  const r = await run(() => api.post<{ terminal: string }>("/api/cli/login/codex-terminal"));
                  if (r) nav(`/terminal?open=${r.terminal}`);
                }}
              >
                Log in via terminal
              </Button>
              {codexIn && (
                <Button size="sm" variant="outline" icon={<LogOut className="size-4" />} onClick={async () => (await run(() => api.post("/api/cli/logout/codex"), "Logged out"), reload())}>
                  Log out
                </Button>
              )}
            </div>
          </Card>

          <Card className="p-5 space-y-1 lg:col-span-2">
            <div className="font-semibold mb-1">Automatic updates</div>
            <Row label="cloud-ai">
              {s.app.version}
              {s.app.latest && s.app.latest !== `v${s.app.version}` && <Badge tone="accent" className="ml-2">{s.app.latest} available</Badge>}
            </Row>
            <Row label="Last update run">{s.last_update_at ? dateTime(s.last_update_at) : "never"}</Row>
            <Row label="Right now">{s.idle ? "idle — updates can run" : "busy — updates wait"}</Row>
            <p className="text-xs text-muted pt-2">The schedule and auto-update switches are in Settings → Updates. A failed CLI update is rolled back to the previous version.</p>
          </Card>
        </div>
      )}

      <Modal open={!!device} onClose={() => (setDevice(null), reload())} title="Log in to Codex">
        {device && (
          <div className="space-y-4 text-sm">
            <p>1. Open this page and sign in with your ChatGPT account:</p>
            <a href={device.verification_url} target="_blank" rel="noreferrer" className="flex items-center gap-1.5 text-accent break-all">
              {device.verification_url} <ExternalLink className="size-3.5 shrink-0" />
            </a>
            <p>2. Enter this code:</p>
            <div className="text-3xl font-mono font-semibold tracking-widest text-center select-all bg-panel-2 rounded-xl py-4">{device.user_code}</div>
            <p className="text-muted">This page updates by itself once the login finishes (you can close this window).</p>
          </div>
        )}
      </Modal>
      <Modal open={!!report} onClose={() => setReport(null)} title="Update result">
        <ul className="text-sm space-y-1">{report?.map((r, i) => <li key={i}>{r}</li>)}</ul>
      </Modal>
    </>
  );
}
