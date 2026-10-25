//! Live browser: a headless Chromium on the server, driven over the Chrome
//! DevTools Protocol. Each project gets one tab; its screen is streamed to the
//! panel as JPEG frames and mouse/keyboard input is sent back. Elements can be
//! picked (selector, HTML, styles) and screenshotted for the AI.
use crate::auth::AuthUser;
use crate::settings;
use crate::state::St;
use crate::store::can_access_project;
use crate::util::{now, ApiError, ApiResult};
use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::{Message as WsMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as CdpMsg;

// ---------- CDP connection ----------

struct Cdp {
    tx: mpsc::UnboundedSender<CdpMsg>,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    events: broadcast::Sender<Value>,
}

impl Cdp {
    async fn connect(url: &str) -> Result<Arc<Cdp>> {
        let (ws, _) = tokio_tungstenite::connect_async(url).await.context("connecting to Chromium")?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<CdpMsg>();
        let (events, _) = broadcast::channel(512);
        let cdp = Arc::new(Cdp { tx, next: AtomicU64::new(1), pending: Mutex::new(HashMap::new()), events });
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                if sink.send(m).await.is_err() {
                    break;
                }
            }
        });
        let c = cdp.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let CdpMsg::Text(text) = msg else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(id) = v["id"].as_u64() {
                    if let Some(tx) = c.pending.lock().unwrap().remove(&id) {
                        let r = if v.get("error").is_some() {
                            Err(v["error"]["message"].as_str().unwrap_or("CDP error").to_string())
                        } else {
                            Ok(v["result"].clone())
                        };
                        let _ = tx.send(r);
                    }
                } else {
                    let _ = c.events.send(v);
                }
            }
            for (_, tx) in c.pending.lock().unwrap().drain() {
                let _ = tx.send(Err("browser closed".into()));
            }
        });
        Ok(cdp)
    }

    async fn call(&self, session: Option<&str>, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.tx.send(CdpMsg::Text(msg.to_string().into())).map_err(|_| anyhow!("browser closed"))?;
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{method}: {e}")),
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("{method}: no answer from the browser"))
            }
        }
    }
}

// ---------- browser + pages ----------

pub struct Page {
    session: String,
    target: String,
    frames: broadcast::Sender<Arc<Vec<u8>>>,
    notices: broadcast::Sender<Value>,
    url: Mutex<String>,
    title: Mutex<String>,
    viewport: Mutex<(u32, u32, bool)>,
    viewers: AtomicUsize,
    console: Mutex<VecDeque<Value>>,
}

struct Browser {
    child: Mutex<Option<Child>>,
    cdp: Arc<Cdp>,
    pages: Mutex<HashMap<i64, Arc<Page>>>,
    last_used: Mutex<Instant>,
}

#[derive(Default)]
pub struct BrowserState {
    inner: tokio::sync::Mutex<Option<Arc<Browser>>>,
}

pub fn router() -> Router<St> {
    Router::new().route("/api/projects/{id}/browser/ws", get(ws_handler))
}

fn find_chromium(st: &St) -> Option<PathBuf> {
    let configured = settings::get(&st.db, "browser_path");
    if !configured.trim().is_empty() {
        return Some(PathBuf::from(configured.trim()));
    }
    for name in ["chromium", "chromium-browser", "google-chrome-stable", "google-chrome", "chrome-headless-shell"] {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    // Playwright downloads (`npx playwright install chromium`); the headless shell uses the least RAM.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    for base in [format!("{home}/.cache/ms-playwright"), "/opt/pw-browsers".to_string()] {
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        dirs.sort();
        dirs.reverse();
        for d in &dirs {
            let p = d.join("chrome-headless-shell-linux64/chrome-headless-shell");
            if p.is_file() {
                return Some(p);
            }
        }
        for d in &dirs {
            for sub in ["chrome-linux64/chrome", "chrome-linux/chrome"] {
                let p = d.join(sub);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

async fn launch(st: &St) -> Result<Arc<Browser>> {
    let bin = find_chromium(st).ok_or_else(|| {
        anyhow!("Chromium not found. Install it with `npx -y playwright install --with-deps chromium` or set its path in Settings.")
    })?;
    let profile = st.cfg.data_dir.join("browser-profile");
    std::fs::create_dir_all(&profile)?;
    let mut child = Command::new(&bin)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-sandbox",
            "--disable-gpu",
            "--disable-dev-shm-usage",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-sync",
            "--mute-audio",
            "--hide-scrollbars",
            "--renderer-process-limit=4",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    let stderr = child.stderr.take().unwrap();
    let mut lines = BufReader::new(stderr).lines();
    let ws_url = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(Some(l)) = lines.next_line().await {
            if let Some(i) = l.find("ws://") {
                return Some(l[i..].trim().to_string());
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| anyhow!("Chromium did not start (no DevTools URL)"))?;
    // Keep draining stderr so the pipe never fills up.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    let cdp = Cdp::connect(&ws_url).await?;
    let b = Arc::new(Browser { child: Mutex::new(Some(child)), cdp, pages: Mutex::new(HashMap::new()), last_used: Mutex::new(Instant::now()) });
    spawn_event_loop(b.clone());
    Ok(b)
}

fn spawn_event_loop(b: Arc<Browser>) {
    let mut rx = b.cdp.events.subscribe();
    tokio::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(session) = ev["sessionId"].as_str() else { continue };
            let page = b.pages.lock().unwrap().values().find(|p| p.session == session).cloned();
            let Some(page) = page else { continue };
            let params = &ev["params"];
            match ev["method"].as_str().unwrap_or("") {
                "Page.screencastFrame" => {
                    let ack = params["sessionId"].clone();
                    let cdp = b.cdp.clone();
                    let s = session.to_string();
                    tokio::spawn(async move {
                        let _ = cdp.call(Some(&s), "Page.screencastFrameAck", json!({"sessionId": ack})).await;
                    });
                    if let Some(data) = params["data"].as_str() {
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                            let _ = page.frames.send(Arc::new(bytes));
                        }
                    }
                }
                "Page.frameNavigated" => {
                    if params["frame"]["parentId"].is_null() {
                        let url = params["frame"]["url"].as_str().unwrap_or("").to_string();
                        *page.url.lock().unwrap() = url.clone();
                        let _ = page.notices.send(json!({"type": "url", "url": url}));
                    }
                }
                "Page.loadEventFired" => {
                    let cdp = b.cdp.clone();
                    let page = page.clone();
                    tokio::spawn(async move {
                        if let Ok(r) = cdp.call(Some(&page.session), "Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await {
                            let t = r["result"]["value"].as_str().unwrap_or("").to_string();
                            *page.title.lock().unwrap() = t.clone();
                            let _ = page.notices.send(json!({"type": "title", "title": t}));
                        }
                        let _ = page.notices.send(json!({"type": "loaded"}));
                    });
                }
                "Runtime.consoleAPICalled" | "Runtime.exceptionThrown" | "Log.entryAdded" => {
                    let entry = console_entry(ev["method"].as_str().unwrap_or(""), params);
                    {
                        let mut c = page.console.lock().unwrap();
                        c.push_back(entry.clone());
                        while c.len() > 200 {
                            c.pop_front();
                        }
                    }
                    let _ = page.notices.send(json!({"type": "console", "entry": entry}));
                }
                _ => {}
            }
        }
    });
}

fn console_entry(method: &str, p: &Value) -> Value {
    match method {
        "Runtime.consoleAPICalled" => {
            let text = p["args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|x| x["value"].as_str().map(String::from).unwrap_or_else(|| x["description"].as_str().unwrap_or(&x["value"].to_string()).to_string()))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            json!({"level": p["type"], "text": crate::util::truncate(&text, 2000)})
        }
        "Runtime.exceptionThrown" => {
            let d = &p["exceptionDetails"];
            let text = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception");
            json!({"level": "error", "text": crate::util::truncate(text, 2000)})
        }
        _ => json!({"level": p["entry"]["level"], "text": crate::util::truncate(p["entry"]["text"].as_str().unwrap_or(""), 2000), "url": p["entry"]["url"]}),
    }
}

async fn browser(st: &St) -> Result<Arc<Browser>> {
    let mut g = st.browser.inner.lock().await;
    if let Some(b) = g.as_ref() {
        let alive = b.child.lock().unwrap().as_mut().map(|c| c.try_wait().ok().flatten().is_none()).unwrap_or(false);
        if alive {
            *b.last_used.lock().unwrap() = Instant::now();
            return Ok(b.clone());
        }
    }
    let b = launch(st).await?;
    *g = Some(b.clone());
    let st2 = st.clone();
    // Close the browser when nobody has looked at it for a while.
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let idle = Duration::from_secs(settings::get_i64(&st2.db, "browser_idle_minutes").max(1) as u64 * 60);
            let mut g = st2.browser.inner.lock().await;
            let Some(b) = g.as_ref() else { return };
            let viewers: usize = b.pages.lock().unwrap().values().map(|p| p.viewers.load(Ordering::SeqCst)).sum();
            if viewers == 0 && b.last_used.lock().unwrap().elapsed() >= idle {
                if let Some(mut c) = b.child.lock().unwrap().take() {
                    let _ = c.start_kill();
                }
                *g = None;
                return;
            }
        }
    });
    Ok(b)
}

async fn page_for(st: &St, project_id: i64) -> Result<(Arc<Browser>, Arc<Page>)> {
    let b = browser(st).await?;
    let existing = b.pages.lock().unwrap().get(&project_id).cloned();
    if let Some(p) = existing {
        return Ok((b, p));
    }
    let t = b.cdp.call(None, "Target.createTarget", json!({"url": "about:blank"})).await?;
    let target = t["targetId"].as_str().ok_or_else(|| anyhow!("no target id"))?.to_string();
    let a = b.cdp.call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true})).await?;
    let session = a["sessionId"].as_str().ok_or_else(|| anyhow!("no session id"))?.to_string();
    let (frames, _) = broadcast::channel(4);
    let (notices, _) = broadcast::channel(64);
    let page = Arc::new(Page {
        session: session.clone(),
        target,
        frames,
        notices,
        url: Mutex::new(String::new()),
        title: Mutex::new(String::new()),
        viewport: Mutex::new((1280, 800, false)),
        viewers: AtomicUsize::new(0),
        console: Mutex::new(VecDeque::new()),
    });
    b.pages.lock().unwrap().insert(project_id, page.clone());
    for m in ["Page.enable", "Runtime.enable", "Log.enable"] {
        b.cdp.call(Some(&session), m, json!({})).await?;
    }
    set_viewport(&b, &page, 1280, 800, false).await?;
    if let Some(url) = default_url(st, project_id) {
        let _ = b.cdp.call(Some(&session), "Page.navigate", json!({"url": url})).await;
    }
    Ok((b, page))
}

/// The project's dev server when it runs, otherwise its production site.
fn default_url(st: &St, project_id: i64) -> Option<String> {
    let p = crate::store::get_project(&st.db, project_id).ok()?;
    let url = match p.dev_port {
        Some(port) if crate::devserver::is_running(st, project_id) => format!("http://127.0.0.1:{port}"),
        _ if !p.prod_url.is_empty() => p.prod_url.clone(),
        Some(port) => format!("http://127.0.0.1:{port}"),
        None => return None,
    };
    Some(url)
}

async fn set_viewport(b: &Browser, page: &Page, w: u32, h: u32, mobile: bool) -> Result<()> {
    let w = w.clamp(240, 2560);
    let h = h.clamp(240, 2560);
    *page.viewport.lock().unwrap() = (w, h, mobile);
    let s = Some(page.session.as_str());
    b.cdp
        .call(s, "Emulation.setDeviceMetricsOverride", json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": mobile}))
        .await?;
    b.cdp.call(s, "Emulation.setTouchEmulationEnabled", json!({"enabled": mobile, "maxTouchPoints": if mobile { 5 } else { 1 }})).await?;
    let ua = if mobile {
        "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"
    } else {
        ""
    };
    b.cdp.call(s, "Emulation.setUserAgentOverride", json!({"userAgent": ua})).await.ok();
    if page.viewers.load(Ordering::SeqCst) > 0 {
        start_screencast(b, page).await?;
    }
    Ok(())
}

async fn start_screencast(b: &Browser, page: &Page) -> Result<()> {
    let (w, h, _) = *page.viewport.lock().unwrap();
    // A tab nobody "looks at" is not painted, so screencast would send nothing.
    let _ = b.cdp.call(Some(&page.session), "Page.bringToFront", json!({})).await;
    let _ = b.cdp.call(Some(&page.session), "Emulation.setFocusEmulationEnabled", json!({"enabled": true})).await;
    b.cdp
        .call(Some(&page.session), "Page.startScreencast", json!({"format": "jpeg", "quality": 70, "maxWidth": w, "maxHeight": h, "everyNthFrame": 1}))
        .await?;
    Ok(())
}

const PICK_JS: &str = r#"(() => {
  const el = document.elementFromPoint(__X__, __Y__);
  if (!el) return null;
  const sel = (e) => {
    if (e.id) return '#' + CSS.escape(e.id);
    const parts = [];
    while (e && e.nodeType === 1 && e !== document.documentElement && parts.length < 7) {
      if (e.id) { parts.unshift('#' + CSS.escape(e.id)); break; }
      let p = e.tagName.toLowerCase();
      const cls = [...e.classList].filter(c => !/^(hover|active|focus)/.test(c)).slice(0, 2);
      if (cls.length) p += '.' + cls.map(c => CSS.escape(c)).join('.');
      const sib = e.parentElement ? [...e.parentElement.children].filter(c => c.tagName === e.tagName) : [];
      if (sib.length > 1) p += ':nth-of-type(' + (sib.indexOf(e) + 1) + ')';
      parts.unshift(p);
      e = e.parentElement;
    }
    return parts.join(' > ');
  };
  const r = el.getBoundingClientRect();
  const cs = getComputedStyle(el);
  return {
    selector: sel(el),
    tag: el.tagName.toLowerCase(),
    text: (el.innerText || el.textContent || '').trim().slice(0, 400),
    html: el.outerHTML.slice(0, 4000),
    rect: { x: r.x, y: r.y, width: r.width, height: r.height },
    styles: { color: cs.color, background: cs.backgroundColor, font: cs.fontSize + ' ' + cs.fontWeight + ' ' + cs.fontFamily.split(',')[0],
              margin: cs.margin, padding: cs.padding, display: cs.display, position: cs.position },
    url: location.href,
  };
})()"#;

async fn pick(b: &Browser, page: &Page, x: f64, y: f64) -> Result<Value> {
    let expr = PICK_JS.replace("__X__", &x.to_string()).replace("__Y__", &y.to_string());
    let r = b.cdp.call(Some(&page.session), "Runtime.evaluate", json!({"expression": expr, "returnByValue": true})).await?;
    Ok(r["result"]["value"].clone())
}

/// Screenshot (a region or the viewport) saved as an attachment the AI can read.
async fn screenshot(st: &St, b: &Browser, page: &Page, user_id: i64, clip: Option<&Value>) -> Result<Value> {
    let mut params = json!({"format": "png"});
    if let Some(c) = clip {
        let pad = 4.0;
        params["clip"] = json!({
            "x": (c["x"].as_f64().unwrap_or(0.0) - pad).max(0.0),
            "y": (c["y"].as_f64().unwrap_or(0.0) - pad).max(0.0),
            "width": c["width"].as_f64().unwrap_or(1.0).max(1.0) + pad * 2.0,
            "height": c["height"].as_f64().unwrap_or(1.0).max(1.0) + pad * 2.0,
            "scale": 1,
        });
    }
    let r = b.cdp.call(Some(&page.session), "Page.captureScreenshot", params).await?;
    let data = base64::engine::general_purpose::STANDARD.decode(r["data"].as_str().unwrap_or(""))?;
    let dir = st.cfg.screenshots_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!("screenshot-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f"));
    let path = dir.join(&name);
    std::fs::write(&path, &data)?;
    let id = {
        let c = st.db.conn();
        c.execute(
            "INSERT INTO attachments(user_id,filename,path,mime,size,created_at) VALUES(?1,?2,?3,'image/png',?4,?5)",
            params![user_id, name, path.to_string_lossy(), data.len() as i64, now()],
        )?;
        c.last_insert_rowid()
    };
    Ok(json!({"id": id, "filename": name, "path": path.to_string_lossy(), "mime": "image/png", "size": data.len()}))
}

fn key_code(key: &str) -> Option<u32> {
    Some(match key {
        "Enter" => 13,
        "Backspace" => 8,
        "Tab" => 9,
        "Escape" => 27,
        "Delete" => 46,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        " " => 32,
        _ => return None,
    })
}

async fn command(st: &St, b: &Browser, page: &Page, user_id: i64, v: &Value) -> Result<Option<Value>> {
    let s = Some(page.session.as_str());
    *b.last_used.lock().unwrap() = Instant::now();
    match v["type"].as_str().unwrap_or("") {
        "navigate" => {
            let mut url = v["url"].as_str().unwrap_or("").trim().to_string();
            if !url.contains("://") && !url.starts_with("about:") {
                url = if url.starts_with("localhost") || url.starts_with("127.") { format!("http://{url}") } else { format!("https://{url}") };
            }
            let r = b.cdp.call(s, "Page.navigate", json!({"url": url})).await?;
            if let Some(err) = r["errorText"].as_str() {
                return Ok(Some(json!({"type": "error", "message": format!("{url}: {err}")})));
            }
        }
        "reload" => {
            b.cdp.call(s, "Page.reload", json!({})).await?;
        }
        "back" | "forward" => {
            let h = b.cdp.call(s, "Page.getNavigationHistory", json!({})).await?;
            let cur = h["currentIndex"].as_i64().unwrap_or(0);
            let idx = if v["type"] == "back" { cur - 1 } else { cur + 1 };
            if let Some(entry) = h["entries"].as_array().and_then(|e| e.get(idx.max(0) as usize)) {
                if idx >= 0 {
                    b.cdp.call(s, "Page.navigateToHistoryEntry", json!({"entryId": entry["id"]})).await?;
                }
            }
        }
        "viewport" => {
            set_viewport(
                b,
                page,
                v["width"].as_u64().unwrap_or(1280) as u32,
                v["height"].as_u64().unwrap_or(800) as u32,
                v["mobile"].as_bool().unwrap_or(false),
            )
            .await?;
        }
        "mouse" => {
            let t = match v["event"].as_str().unwrap_or("") {
                "down" => "mousePressed",
                "up" => "mouseReleased",
                "wheel" => "mouseWheel",
                _ => "mouseMoved",
            };
            let mut p = json!({
                "type": t, "x": v["x"], "y": v["y"],
                "button": v["button"].as_str().unwrap_or(if t == "mouseMoved" { "none" } else { "left" }),
                "clickCount": v["clickCount"].as_i64().unwrap_or(if t == "mouseMoved" { 0 } else { 1 }),
                "modifiers": v["modifiers"].as_i64().unwrap_or(0),
            });
            if t == "mouseWheel" {
                p["deltaX"] = json!(v["deltaX"].as_f64().unwrap_or(0.0));
                p["deltaY"] = json!(v["deltaY"].as_f64().unwrap_or(0.0));
            }
            b.cdp.call(s, "Input.dispatchMouseEvent", p).await?;
        }
        "key" => {
            let key = v["key"].as_str().unwrap_or("");
            let down = v["event"] != "up";
            let printable = key.chars().count() == 1;
            let mut p = json!({
                "type": if down { if printable { "keyDown" } else { "rawKeyDown" } } else { "keyUp" },
                "key": key, "code": v["code"], "modifiers": v["modifiers"].as_i64().unwrap_or(0),
            });
            if let Some(code) = key_code(key) {
                p["windowsVirtualKeyCode"] = json!(code);
            } else if printable {
                p["windowsVirtualKeyCode"] = json!(key.to_ascii_uppercase().chars().next().map(|c| c as u32).unwrap_or(0));
            }
            if down && (printable || key == "Enter") && v["modifiers"].as_i64().unwrap_or(0) & 6 == 0 {
                p["text"] = json!(if key == "Enter" { "\r" } else { key });
            }
            b.cdp.call(s, "Input.dispatchKeyEvent", p).await?;
        }
        "text" => {
            b.cdp.call(s, "Input.insertText", json!({"text": v["text"]})).await?;
        }
        "hover" | "inspect" => {
            let el = pick(b, page, v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0)).await?;
            return Ok(Some(json!({"type": if v["type"] == "hover" { "hover" } else { "element" }, "element": el, "req": v["req"]})));
        }
        "screenshot" => {
            let att = screenshot(st, b, page, user_id, v.get("rect").filter(|r| !r.is_null())).await?;
            return Ok(Some(json!({"type": "screenshot", "attachment": att, "req": v["req"]})));
        }
        "console" => {
            let entries: Vec<Value> = page.console.lock().unwrap().iter().cloned().collect();
            return Ok(Some(json!({"type": "console_all", "entries": entries})));
        }
        "clear_console" => page.console.lock().unwrap().clear(),
        other => bail!("unknown command {other}"),
    }
    Ok(None)
}

async fn ws_handler(State(st): State<St>, AuthUser(u): AuthUser, Path(id): Path<i64>, ws: WebSocketUpgrade) -> ApiResult<Response> {
    if !can_access_project(&st.db, &u, id) {
        return Err(ApiError::forbidden());
    }
    Ok(ws.on_upgrade(move |socket| async move {
        if let Err(e) = session(st, id, u.id, socket).await {
            tracing::debug!("browser session: {e:#}");
        }
    }))
}

async fn session(st: St, project_id: i64, user_id: i64, mut socket: WebSocket) -> Result<()> {
    let (b, page) = match page_for(&st, project_id).await {
        Ok(x) => x,
        Err(e) => {
            let _ = socket.send(WsMsg::Text(json!({"type": "fatal", "message": format!("{e:#}")}).to_string().into())).await;
            return Ok(());
        }
    };
    let mut frames = page.frames.subscribe();
    let mut notices = page.notices.subscribe();
    // A tab left on an error or blank page tries the project's URL again.
    let current = page.url.lock().unwrap().clone();
    if current.is_empty() || current.starts_with("about:") || current.starts_with("chrome-error:") {
        if let Some(url) = default_url(&st, project_id) {
            let _ = b.cdp.call(Some(&page.session), "Page.navigate", json!({"url": url})).await;
        }
    }
    if page.viewers.fetch_add(1, Ordering::SeqCst) == 0 {
        start_screencast(&b, &page).await?;
    }
    let (w, h, mobile) = *page.viewport.lock().unwrap();
    let hello = json!({"type": "hello", "url": *page.url.lock().unwrap(), "title": *page.title.lock().unwrap(), "width": w, "height": h, "mobile": mobile});
    socket.send(WsMsg::Text(hello.to_string().into())).await?;
    // Ask for a fresh frame even when the page is static.
    let _ = b.cdp.call(Some(&page.session), "Runtime.evaluate", json!({"expression": "void 0"})).await;
    let res: Result<()> = async {
        loop {
            tokio::select! {
                f = frames.recv() => match f {
                    Ok(bytes) => socket.send(WsMsg::Binary(bytes.as_ref().clone().into())).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                n = notices.recv() => match n {
                    Ok(v) => socket.send(WsMsg::Text(v.to_string().into())).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                m = socket.recv() => match m {
                    Some(Ok(WsMsg::Text(t))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                        match command(&st, &b, &page, user_id, &v).await {
                            Ok(Some(reply)) => socket.send(WsMsg::Text(reply.to_string().into())).await?,
                            Ok(None) => {}
                            Err(e) => socket.send(WsMsg::Text(json!({"type": "error", "message": format!("{e:#}"), "req": v["req"]}).to_string().into())).await?,
                        }
                    }
                    Some(Ok(WsMsg::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
        Ok(())
    }
    .await;
    if page.viewers.fetch_sub(1, Ordering::SeqCst) == 1 {
        let _ = b.cdp.call(Some(&page.session), "Page.stopScreencast", json!({})).await;
    }
    *b.last_used.lock().unwrap() = Instant::now();
    let _ = &page.target;
    res
}

pub async fn stop(st: &St) {
    if let Some(b) = st.browser.inner.lock().await.take() {
        if let Some(mut c) = b.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
    }
}
