# cloud-ai

A self-hosted web panel for running **Claude Code** and **Codex CLI** on your own server, from your phone or desktop.
It is one small Rust binary (about 15–30 MB of RAM) with an embedded React PWA and a SQLite database, built for small VPSs that already run other apps.

## Features

- **Chats with Claude Code or Codex** on any project folder. You can pick the model and reasoning effort; the list is read from the installed CLIs and refreshed automatically. Replies, thinking, tool calls and diffs stream live. You can attach images and files, stop a run, or queue follow-ups. Each chat can optionally run in its own git worktree with a one-click merge.
- **Chat side panel** with Sub-agents, Background (shell commands and monitors) and **Files** tabs. Files lists every file the chat added, modified or deleted, with +/− line counts and a diff per file (from git since the chat started, or from the agent's edit tools in folders without git).
- **Context usage**: a ring in the chat header shows how full the context window is. For Claude Code it opens the same breakdown as `/context` (system prompt, system tools, MCP tools, skills, memory files, messages, autocompact buffer, free space); Codex shows used and remaining.
- **Read-only chats**: the agent can read files and run read-only commands, but cannot change files, commit, install or deploy. No worktree, branch or auto-deploy is used. Claude runs with edit tools disabled and every state-changing request refused; Codex runs in its read-only sandbox.
- **Background work is waited for**: a prompt counts as finished (and is reported to Telegram) only when the sub-agents and background shell commands it started are done and the agent has replied, not when it pauses to wait for them. Sending a follow-up or pressing stop ends the wait; there is also a time limit in Settings.
- **Sub-agents**: when Claude Code or Codex hands work to sub-agents, a side panel (a drawer on phones) lists each one with its live status, duration, tool count and the step it is on. Open one to see the task it was given and its own tool calls, or stop just that agent. When the CLI can't stop a single agent, the running prompt is stopped instead, and the panel tells you which happened.
- **Permission modes** per chat: full access (the default), auto-edit, ask before changes, plan only. Approvals and the agents' questions show up as cards in the panel *and* as Telegram messages with buttons. Answering in one place updates the other.
- **Background commands and monitors**: when Claude Code runs a shell command in the background or starts a Monitor, it appears in the chat's side panel (*Background* tab, next to *Sub-agents*) with its command, live output and status. **Stop** asks Claude Code to end it; **Kill** ends its processes right away. Idle chat processes are not stopped while a background command still runs.
- **Usage page**: tokens and cost for today, this week (Saturday to Friday), this month or any date range, in the panel's timezone. It shows input, output, cache read, cache write, reasoning tokens, web searches, runs and cost, with a daily chart and breakdowns by project, user, model, tool, chat, schedule and source, plus CSV export. Admins see the whole system and a *Per user* tab; users see their own usage. Claude Code reports its own cost; Codex cost is estimated from the price table in **Admin → Settings → Usage prices**, which lists every current Claude and OpenAI model and is updated every day from the public LiteLLM price list (or on demand with *Update now*). Rows you add or edit are marked *manual* and kept by the updates.
- **Dashboard**: server load, running and queued prompts, recent chats (a chat whose changes were deployed shows its latest deploy as an icon: deployed, deploying, rolled back or failed), the Claude and Codex accounts the CLIs are logged in with, and **plan usage** (5-hour and weekly limits) for both. Claude usage is read live from Anthropic with the CLI's login, Codex usage from Codex itself; both refresh every few minutes, and switching to another account shows the new account's usage within seconds instead of the old one.
- **Schedules**: run prompts once, daily, on chosen weekdays, or on a cron expression (with a builder and a preview of the next runs). Times are in Asia/Tehran by default. A schedule can be a chain of prompts, can start a new chat each time or keep using one, and can deploy when it finishes. If a project is busy, scheduled prompts wait in the queue.
- **Queue and resource guard**: a limit of parallel sessions per tool (default 3 + 3), a minimum free RAM before a prompt starts, and idle agent processes stopped automatically. Conversations resume transparently.
- **Projects**: add an existing folder, create a new one, clone a git repo, or import from another server over SSH. Admins assign projects to users. Includes a file browser/editor and a git view (status, diff, commit, push, pull, branches, history).
- **Set up deploy with AI** (Project → Settings): Claude Code or Codex reads the project and a read-only snapshot of the server it runs on (services, pm2, containers, ports, nginx) and proposes a deploy command and a health check URL. **Check without deploying** verifies the folder, SSH, git checkout, command syntax, the programs it calls and the health URL; **Save & run test deploy** runs a real deploy with the health check and automatic rollback.
- **Deploys**: run a command on this server, or ship to another server over SSH, either with `git push` + `git pull` there or with rsync. The commit is recorded for every deploy, and an optional health check rolls back automatically on failure. Deploy and rollback buttons are in the panel and in Telegram. Each project has a **Deploy history** tab listing every deploy and rollback with its log, where you can roll back to any earlier successful version. Running deploys show on the dashboard and on the project's chat pages. Claude and Codex can also SSH into your servers to read logs.
- **Live preview**: the panel starts and stops each project's dev server. A headless Chromium on the server opens the dev or production site, streamed into the panel (desktop, tablet and phone sizes). You can click and type in it, read the console, open it fullscreen, and **select elements**: selector, HTML, styles and a screenshot are captured, and you can comment on each and send them all to Claude or Codex in one go.
- **Telegram**: one group with a topic per chat. It reports new chats, prompts, results, errors, approvals, questions, deploys and updates. You answer approvals and questions with buttons (or by replying with text).
  - **Chat from Telegram**: write in a chat's topic and the message becomes that chat's next prompt (photos and files too). The answer comes back in the topic.
  - `/new [prompt]` starts a new chat: pick the project and the agent with buttons.
  - `/stop` stops the running prompt in a topic, and `/deploy` and `/status` work from the group. A deploy started from a topic asks for confirmation and reports its progress in that same topic.
- **Users and security**: admin and user roles, **mandatory TOTP 2FA** for everyone, login rate limiting, and encrypted secrets. The first admin needs a one-time setup token.
- **Maintenance**: Claude Code and Codex are updated every day at 05:00, but only when nothing is running. A failed update is rolled back. The panel updates itself from GitHub releases. You can log the CLIs in from the panel (Codex device code, or a web terminal for `claude auth login`), and there is a web terminal for anything else. In the terminal, **Ctrl+Shift+V** pastes and **Ctrl+Shift+C** copies, links are clickable, and on phones there are Paste and Copy buttons.
- **Skills, plugins, MCP servers and CLAUDE.md / AGENTS.md** for both CLIs, either global or per project.
- Dark and light theme with themed controls everywhere (searchable dropdowns, date, time and range pickers, number steppers, checkboxes), installable as a PWA, works on mobile. Pages show a loading placeholder until their data arrives, and an error with Retry if it can't load (they also retry on their own when the connection comes back), never an empty list by mistake.

## Install

On the VPS (Ubuntu or Debian, as root), with Claude Code and Codex already installed through npm:

```bash
curl -fsSL https://raw.githubusercontent.com/larrywatkins254187/cloud-ai/main/deploy/install.sh \
  | sudo bash -s -- --domain ai.example.com
```

The installer does the following:

- installs `git`, `rsync`, `sshpass`, the SSH client and a headless Chromium (Playwright's *chrome-headless-shell*);
- downloads the latest release binary to `/usr/local/bin/cloud-ai`;
- creates the `cloud-ai` systemd service, which listens on `127.0.0.1:8787` and keeps its data in `/var/lib/cloud-ai`;
- writes an nginx site for your domain, including WebSockets, and requests a Let's Encrypt certificate when `certbot` is available;
- prints the **setup token**.

Open the domain, create the admin with the setup token, scan the QR code with an authenticator app, and you are in.

Installer options: `--version v0.4.0`, `--port 8787`, `--no-browser`, and `--build`, which builds from a checkout instead of downloading.

### After installing

1. **Admin → Claude & Codex**: log both CLIs in, if they aren't already.
2. **Admin → Settings → Telegram**:
   - create a bot with @BotFather;
   - create a group with *Topics* enabled;
   - add the bot as an admin with "Manage topics" (as an admin it also receives the plain messages you write in topics, which is how you chat from Telegram; otherwise turn its privacy mode off in @BotFather);
   - send `/id` in the group, then paste the token and chat id.
3. **Projects → Add project**.
4. **Admin → Servers**, for deploying to other machines: add the panel's public key to `authorized_keys` on the target server, or use your own key or a password.

## Configuration

Most settings are in **Admin → Settings**: limits, RAM guard, timezone, CLI paths, Telegram, updates and browser.
Only a few things come from the environment, which is set in the systemd unit:

| Variable | Default | Meaning |
|---|---|---|
| `CLOUD_AI_DATA` | `/var/lib/cloud-ai` | database, uploads, SSH keys, screenshots |
| `CLOUD_AI_BIND` | `127.0.0.1:8787` | listen address (keep it on localhost behind nginx) |
| `CLOUD_AI_PUBLIC_URL` | – | panel URL for links in Telegram messages |

Recovery commands on the server:

```bash
cloud-ai create-admin <name>     # new admin (asks for a password)
cloud-ai set-password <name>
cloud-ai reset-2fa <name>        # scan a new QR code on next login
journalctl -u cloud-ai -f        # logs
```

## Deploys

Every deploy command runs with `bash -l` in the project folder, or in the remote path on the target server. `$DEPLOY_REF` holds the commit being deployed.

- **This server**: optionally commits pending changes, then runs the command, e.g. `npm ci && npm run build && systemctl restart my-app`.
- **Another server, git**: commits, runs `git push`, then on the server runs `git fetch && git reset --hard <commit> && <command>`.
- **Another server, rsync**: commits, then copies the files with `rsync --delete`. The exclude list (default `node_modules .git .next .env .env.local`) is neither copied nor deleted on the server. Then it runs the command there.
- **Health check**: after a deploy the URL is polled for about a minute. If it fails and auto-rollback is on, the last successful commit is deployed again.
- **Rollbacks**:
  - On this server, the current state is kept on a `cloud-ai/backup-*` branch first.
  - With rsync, the old commit is exported with `git archive`, which needs the project to be a git repository.

## How it works

- **Claude Code** runs as `claude -p --input-format stream-json --output-format stream-json --permission-prompt-tool stdio`. This is the same protocol the Agent SDK uses. There is one process per active chat; it is stopped when idle and resumed with `--resume`.
- **Codex** runs as one shared `codex app-server` (JSON-RPC) that hosts the threads of every Codex chat.
- Everything that happens goes through an internal event bus. One WebSocket per browser tab delivers the live updates, and the Telegram bot listens to the same events.
- The database is SQLite in WAL mode, with a single connection.
- The frontend is embedded in the binary, and nginx only proxies to it.

## Releases and self-update

Push a tag, or run **Actions → Release → Run workflow** with a version:

```bash
git tag v0.4.0 && git push origin v0.4.0
```

The workflow builds `cloud-ai-linux-x86_64` and `cloud-ai-linux-aarch64`, each with a `.sha256` file, and publishes a GitHub release.
Installed panels check every day at the update hour. They download the binary, verify the checksum and run it once as a smoke test, swap it in, and let systemd restart the service. The previous binary is kept as `/usr/local/bin/cloud-ai.old`.

## Security notes

- The service runs as **root** and agents default to **full access**. Anyone with a panel account can effectively run commands on the server, so only give accounts to people you trust with root.
- 2FA is mandatory. Sessions are HttpOnly cookies, and login attempts are rate limited.
- SSH passwords are stored AES-GCM encrypted. The key is in `/var/lib/cloud-ai/secret.key`, outside the database. Private keys are stored as files with `0600` permissions.

## Development

```bash
cd web && npm install && npm run dev       # Vite on :5173, proxies /api to :8787
cargo run -- --data ./data                  # backend (serves web/dist when built)
cargo test
```

The release binary embeds `web/dist`, so run `npm run build` before `cargo build --release`.

## Uninstall

```bash
systemctl disable --now cloud-ai
rm /etc/systemd/system/cloud-ai.service /usr/local/bin/cloud-ai /etc/nginx/sites-enabled/cloud-ai /etc/nginx/sites-available/cloud-ai
rm -r /var/lib/cloud-ai      # deletes chats, settings and keys
```

Your project folders are never touched by uninstalling.
