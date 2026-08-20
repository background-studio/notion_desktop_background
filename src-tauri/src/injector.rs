use std::{
    collections::{HashMap, HashSet},
    net::TcpStream,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;

use crate::payload::{ActivePayload, PENDING_MEDIA_URL_KEY};

const MEDIA_CHUNK_BYTES: usize = 192 * 1024;
const PENDING_MEDIA_PARTS_KEY: &str = "__BACKGROUND_STUDIO_PENDING_MEDIA_PARTS__";

const REMOVE_RENDERER_PAYLOAD: &str = r#"(() => {
  // 推进运行序号：让仍在 decode 路上的注入轮醒来后自杀，避免移除后又装回来。
  window.__NOTION_BACKGROUND_RUN_SEQ__ = (Number(window.__NOTION_BACKGROUND_RUN_SEQ__) || 0) + 1;
  document.getElementById("notion-background-early-transparency")?.remove();
  const state = window.__NOTION_BACKGROUND_STUDIO__;
  if (state?.cleanup) return state.cleanup();
  document.getElementById("notion-background-layer")?.remove();
  document.getElementById("notion-background-style")?.remove();
  document.documentElement?.classList.remove(
    "notion-background-active", "notion-background-home", "notion-background-task",
    "notion-background-home-disabled", "notion-background-task-disabled", "notion-background-fit-tile"
  );
  delete window.__NOTION_BACKGROUND_STUDIO__;
  return true;
})()"#;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTarget {
    id: String,
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    web_socket_debugger_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpVersion {
    web_socket_debugger_url: String,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

fn validate_websocket_url(
    value: &str,
    port: u16,
    kind: &str,
    id: Option<&str>,
) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "CDP WebSocket 地址无效。".to_string())?;
    let hostname = url.host_str().unwrap_or_default();
    let loopback = matches!(hostname, "127.0.0.1" | "localhost" | "::1");
    let expected = id
        .map(|id| format!("/devtools/{kind}/{id}"))
        .unwrap_or_else(|| format!("/devtools/{kind}/"));
    let valid_path = id.map_or_else(
        || url.path().starts_with(&expected),
        |_| url.path() == expected,
    );
    if url.scheme() != "ws"
        || !loopback
        || url.port() != Some(port)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !valid_path
    {
        return Err("CDP WebSocket 地址未通过本机回环校验。".to_string());
    }
    Ok(url.to_string())
}

const ATTACH_IDENTITY_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_IDENTITY_TIMEOUT: Duration = Duration::from_millis(400);

fn fetch_json<T: for<'de> Deserialize<'de>>(port: u16, resource: &str) -> Result<T, String> {
    fetch_json_with_timeout(port, resource, ATTACH_IDENTITY_TIMEOUT)
}

fn fetch_json_with_timeout<T: for<'de> Deserialize<'de>>(
    port: u16,
    resource: &str,
    timeout: Duration,
) -> Result<T, String> {
    let response = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?
        .get(format!("http://127.0.0.1:{port}{resource}"))
        .header("Cache-Control", "no-store")
        .send()
        .map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("CDP 返回 HTTP {}", status.as_u16()));
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("CDP HTTP 响应超过大小上限。".to_string());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn browser_identity_from_version(port: u16, version: CdpVersion) -> Result<String, String> {
    let websocket =
        validate_websocket_url(&version.web_socket_debugger_url, port, "browser", None)?;
    let url = Url::parse(&websocket).map_err(|error| error.to_string())?;
    let id = url
        .path()
        .strip_prefix("/devtools/browser/")
        .filter(|id| valid_id(id))
        .ok_or_else(|| "CDP 浏览器身份无效。".to_string())?;
    Ok(id.to_string())
}

pub fn read_browser_identity(port: u16) -> Result<String, String> {
    let version: CdpVersion = fetch_json(port, "/json/version")?;
    browser_identity_from_version(port, version)
}

pub fn probe_browser_identity(port: u16) -> Result<String, String> {
    let version: CdpVersion =
        fetch_json_with_timeout(port, "/json/version", PROBE_IDENTITY_TIMEOUT)?;
    browser_identity_from_version(port, version)
}

fn is_main_notion_target_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    match url.scheme() {
        "https" => matches!(
            url.host_str(),
            Some("app.notion.com") | Some("www.notion.so")
        ),
        // Electron 顶部标签栏是独立 page；也要注入，才能和主页面拼成一整张背景。
        "file" => {
            let path = url.path().to_ascii_lowercase().replace('\\', "/");
            path.contains("/renderer/tabs/index.html") || path.ends_with("/tabs/index.html")
        }
        _ => false,
    }
}

fn list_targets(port: u16, browser_id: &str) -> Result<Vec<CdpTarget>, String> {
    if read_browser_identity(port)? != browser_id {
        return Err("CDP 浏览器身份已变化，拒绝继续注入。".to_string());
    }
    let targets: Vec<CdpTarget> = fetch_json(port, "/json/list")?;
    Ok(targets
        .into_iter()
        .filter(|target| {
            target.target_type == "page"
                && is_main_notion_target_url(&target.url)
                && valid_id(&target.id)
                && validate_websocket_url(
                    &target.web_socket_debugger_url,
                    port,
                    "page",
                    Some(&target.id),
                )
                .is_ok()
        })
        .collect())
}

enum WorkerRequest {
    Send {
        method: String,
        params: Value,
        response: mpsc::Sender<Result<Value, String>>,
    },
    Close,
}

type CdpSocket = WebSocket<MaybeTlsStream<TcpStream>>;

fn socket_command(
    socket: &mut CdpSocket,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = *next_id;
    *next_id += 1;
    socket
        .send(Message::Text(
            serde_json::to_string(&json!({ "id": id, "method": method, "params": params }))
                .map_err(|error| error.to_string())?
                .into(),
        ))
        .map_err(|error| error.to_string())?;
    loop {
        let message = socket.read().map_err(|error| error.to_string())?;
        let Message::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("CDP 命令失败");
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            return Err(format!("{message} ({code})"));
        }
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
}

struct CdpSession {
    sender: mpsc::Sender<WorkerRequest>,
    closed: Arc<AtomicBool>,
}

impl CdpSession {
    fn open(target: &CdpTarget, port: u16) -> Result<Self, String> {
        let websocket_url = validate_websocket_url(
            &target.web_socket_debugger_url,
            port,
            "page",
            Some(&target.id),
        )?;
        let (sender, receiver) = mpsc::channel();
        let (setup_sender, setup_receiver) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        let worker_closed = Arc::clone(&closed);
        thread::Builder::new()
            .name(format!("cdp-{}", target.id))
            .spawn(move || {
                let setup = (|| {
                    let (mut socket, _) =
                        connect(websocket_url.as_str()).map_err(|error| error.to_string())?;
                    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(30)))
                            .map_err(|error| error.to_string())?;
                        // 大图 base64 写入可达数 MB，10s 写超时会把会话打坏并堵死后续命令。
                        stream
                            .set_write_timeout(Some(Duration::from_secs(120)))
                            .map_err(|error| error.to_string())?;
                    }
                    let mut next_id = 1;
                    socket_command(&mut socket, &mut next_id, "Runtime.enable", json!({}))?;
                    socket_command(&mut socket, &mut next_id, "Page.enable", json!({}))?;
                    Ok::<_, String>((socket, next_id))
                })();
                match setup {
                    Ok((mut socket, mut next_id)) => {
                        let _ = setup_sender.send(Ok(()));
                        while let Ok(request) = receiver.recv() {
                            match request {
                                WorkerRequest::Send {
                                    method,
                                    params,
                                    response,
                                } => {
                                    let result =
                                        socket_command(&mut socket, &mut next_id, &method, params);
                                    let _ = response.send(result);
                                }
                                WorkerRequest::Close => {
                                    let _ = socket.close(None);
                                    break;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = setup_sender.send(Err(error));
                    }
                }
                worker_closed.store(true, Ordering::Relaxed);
            })
            .map_err(|error| error.to_string())?;
        setup_receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "CDP 连接超时。".to_string())??;
        Ok(Self { sender, closed })
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn send(&self, method: &str, params: Value) -> Result<Value, String> {
        if self.is_closed() {
            return Err("CDP 会话已关闭。".to_string());
        }
        // 大图 base64 注入可达数 MB；固定 11s 会误杀仍在写 socket 的命令，
        // 并让后续命令堵在 worker 队列里，表现为「正在处理」卡死、退出也无响应。
        let approx_bytes = params.to_string().len() as u64;
        let timeout_secs = (20 + approx_bytes / 80_000).clamp(20, 180);
        let (response, receiver) = mpsc::channel();
        self.sender
            .send(WorkerRequest::Send {
                method: method.to_string(),
                params,
                response,
            })
            .map_err(|_| "CDP 会话已关闭。".to_string())?;
        match receiver.recv_timeout(Duration::from_secs(timeout_secs)) {
            Ok(result) => result,
            Err(_) => {
                // 超时后的 socket 仍可能在处理旧命令，不能继续向同一 worker 排队。
                self.closed.store(true, Ordering::Release);
                Err(format!("CDP 命令超时：{method}（已等待 {timeout_secs}s）"))
            }
        }
    }

    fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let result = self.send(
            "Runtime.evaluate",
            json!({
                "expression": expression,
                "awaitPromise": true,
                "returnByValue": true,
                "userGesture": false
            }),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            let detail = details
                .pointer("/exception/description")
                .and_then(Value::as_str)
                .or_else(|| details.get("text").and_then(Value::as_str))
                .unwrap_or("未知异常");
            return Err(format!(
                "Notion 渲染页执行背景脚本失败：{}",
                detail.chars().take(300).collect::<String>()
            ));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn document_has_revision(&self, revision: &str) -> Result<bool, String> {
        self.evaluate(&document_revision_probe(revision))
            .map(|value| value.as_bool().unwrap_or(false))
    }
}

fn clear_pending_media(session: &CdpSession, revoke_url: bool) {
    let parts_key = serde_json::to_string(PENDING_MEDIA_PARTS_KEY).expect("key is serializable");
    let url_key = serde_json::to_string(PENDING_MEDIA_URL_KEY).expect("key is serializable");
    let revoke = if revoke_url {
        format!(
            "const url=window[{url_key}];if(typeof url==='string'&&url.startsWith('blob:'))URL.revokeObjectURL(url);"
        )
    } else {
        String::new()
    };
    let _ = session.evaluate(&format!(
        "(()=>{{{revoke}delete window[{parts_key}];delete window[{url_key}];return true;}})()"
    ));
}

fn upload_media(session: &CdpSession, payload: &ActivePayload) -> Result<(), String> {
    let parts_key =
        serde_json::to_string(PENDING_MEDIA_PARTS_KEY).map_err(|error| error.to_string())?;
    let url_key =
        serde_json::to_string(PENDING_MEDIA_URL_KEY).map_err(|error| error.to_string())?;
    session.evaluate(&format!(
        "(()=>{{const url=window[{url_key}];if(typeof url==='string'&&url.startsWith('blob:'))URL.revokeObjectURL(url);window[{url_key}]=undefined;window[{parts_key}]=[];return true;}})()"
    ))?;
    for chunk in payload.media_bytes.chunks(MEDIA_CHUNK_BYTES) {
        let encoded =
            serde_json::to_string(&STANDARD.encode(chunk)).map_err(|error| error.to_string())?;
        session.evaluate(&format!(
            "(()=>{{const binary=atob({encoded});const bytes=new Uint8Array(binary.length);for(let i=0;i<binary.length;i+=1)bytes[i]=binary.charCodeAt(i);window[{parts_key}].push(bytes);return bytes.length;}})()"
        ))?;
    }
    let mime =
        serde_json::to_string(&payload.media_mime_type).map_err(|error| error.to_string())?;
    session.evaluate(&format!(
        "(()=>{{const parts=window[{parts_key}];if(!Array.isArray(parts))throw new Error('背景媒体分块状态丢失');window[{url_key}]=URL.createObjectURL(new Blob(parts,{{type:{mime}}}));delete window[{parts_key}];return window[{url_key}];}})()"
    ))?;
    Ok(())
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerRequest::Close);
        self.closed.store(true, Ordering::Relaxed);
    }
}

struct ManagedSession {
    session: CdpSession,
    early_script_id: Option<String>,
    revision: Option<String>,
}

struct InjectorInner {
    port: u16,
    browser_id: String,
    sessions: HashMap<String, ManagedSession>,
    payload: Option<ActivePayload>,
    paused: bool,
}

fn early_payload_for(payload: &str, revision: &str) -> String {
    let safe_revision = serde_json::to_string(revision).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(() => {{
  const revision = {safe_revision};
  const run = () => {{
    if (document.documentElement?.localName !== "html") return false;
    try {{ {payload}; return true; }} catch {{ return false; }}
  }};
  if (!run()) {{
    const observer = new MutationObserver(() => {{
      if (run()) observer.disconnect();
    }});
    observer.observe(document.documentElement || document, {{ childList: true, subtree: true }});
    setTimeout(() => observer.disconnect(), 30000);
  }}
  return revision;
}})()"#
    )
}

fn document_revision_probe(revision: &str) -> String {
    let safe_revision = serde_json::to_string(revision).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(() => {{
  const expected = {safe_revision};
  const state = window.__NOTION_BACKGROUND_STUDIO__;
  const style = document.getElementById("notion-background-style");
  return state?.revision === expected && style?.dataset?.cbgRevision === expected;
}})()"#
    )
}

const VIEWPORT_SIZE_PROBE: &str = r#"({
  width: Number(window.innerWidth) || 0,
  height: Number(window.innerHeight) || 0
})"#;

fn usable_viewport(width: f64, height: f64) -> bool {
    width >= 1.0 && height >= 1.0
}

fn session_has_usable_viewport(session: &CdpSession) -> bool {
    let Ok(value) = session.evaluate(VIEWPORT_SIZE_PROBE) else {
        return false;
    };
    let width = value.get("width").and_then(Value::as_f64).unwrap_or(0.0);
    let height = value.get("height").and_then(Value::as_f64).unwrap_or(0.0);
    usable_viewport(width, height)
}

fn should_apply_payload(
    managed_revision: Option<&str>,
    expected_revision: &str,
    document_matches: bool,
    force: bool,
) -> bool {
    force || managed_revision != Some(expected_revision) || !document_matches
}

fn remove_from_session(managed: &mut ManagedSession) {
    if let Some(identifier) = managed.early_script_id.take() {
        let _ = managed.session.send(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({ "identifier": identifier }),
        );
    }
    let _ = managed.session.evaluate(REMOVE_RENDERER_PAYLOAD);
    managed.revision = None;
}

pub(crate) const EARLY_TRANSPARENCY_SCRIPT: &str = r#"(() => {
  try {
    const install = () => {
      const root = document.documentElement;
      if (!root || root.localName !== "html") return false;
      const style = document.createElement("style");
      style.id = "notion-background-early-transparency";
      const isTabChrome = location.protocol === "file:" &&
        location.href.toLowerCase().includes("/tabs/index.html");
      const tabChromeCss = isTabChrome ? `
.root, .root.notion-dark-theme, .root.notion-light-theme, .hide-scrollbar {
  background: transparent !important;
  background-color: transparent !important;
}
.root *, .hide-scrollbar * {
  background-color: transparent !important;
  box-shadow: none !important;
  border-color: transparent !important;
}
[style*="linear-gradient"][style*="--gradient-direction"] {
  background: transparent !important;
  background-image: none !important;
}
` : "";
      style.textContent = "html,body,.notion-app-inner,.notion-cursor-listener,main.notion-frame,header,.notion-topbar,.notion-sidebar,.notion-sidebar-container,.root,.notion-dark-theme{background:transparent!important;background-color:transparent!important}" +
        tabChromeCss;
      root.appendChild(style);
      return true;
    };
    if (!install()) {
      const observer = new MutationObserver(() => {
        if (install()) observer.disconnect();
      });
      observer.observe(document, { childList: true });
      setTimeout(() => observer.disconnect(), 30000);
    }
  } catch {}
})()"#;

fn apply_managed_session(managed: &mut ManagedSession, payload: &ActivePayload, force: bool) {
    // Notion 会留一个 0×0 的 /blank 恢复页。那页上 img.decode() 可能一直不返回，
    // 串行注入时会把标题栏已经换完、正文还停在旧图。没可视面积的页先跳过。
    if !session_has_usable_viewport(&managed.session) {
        return;
    }
    let document_matches = !force
        && managed.revision.as_deref() == Some(payload.revision.as_str())
        && managed
            .session
            .document_has_revision(&payload.revision)
            .unwrap_or(false);
    if !should_apply_payload(
        managed.revision.as_deref(),
        &payload.revision,
        document_matches,
        force,
    ) {
        return;
    }
    if managed.revision.is_some() {
        managed.revision = None;
    }
    if let Err(error) = apply_to_session(managed, payload) {
        eprintln!("CDP 注入失败: {error}");
    }
}

fn apply_to_session(managed: &mut ManagedSession, payload: &ActivePayload) -> Result<(), String> {
    let revision = &payload.revision;
    if managed.revision.as_deref() == Some(revision) {
        return Ok(());
    }
    if let Some(identifier) = managed.early_script_id.take() {
        let _ = managed.session.send(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({ "identifier": identifier }),
        );
    }
    managed
        .session
        .send("Page.setBypassCSP", json!({ "enabled": true }))?;
    let early_source = payload
        .early_script
        .as_deref()
        .map(|script| early_payload_for(script, revision))
        .unwrap_or_else(|| EARLY_TRANSPARENCY_SCRIPT.to_string());
    let early = managed.session.send(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": early_source }),
    )?;
    managed.early_script_id = early
        .get("identifier")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Err(error) = upload_media(&managed.session, payload)
        .and_then(|_| managed.session.evaluate(&payload.script).map(|_| ()))
    {
        clear_pending_media(&managed.session, true);
        return Err(error);
    }
    clear_pending_media(&managed.session, false);
    managed.revision = Some(revision.to_string());
    Ok(())
}

fn sync_inner(inner: &Arc<Mutex<InjectorInner>>, target_count: &AtomicUsize, force: bool) {
    let Ok(mut inner) = inner.lock() else {
        return;
    };
    let Ok(targets) = list_targets(inner.port, &inner.browser_id) else {
        target_count.store(inner.sessions.len(), Ordering::Relaxed);
        return;
    };
    let target_ids = targets
        .iter()
        .map(|target| target.id.as_str())
        .collect::<HashSet<_>>();
    inner
        .sessions
        .retain(|id, managed| target_ids.contains(id.as_str()) && !managed.session.is_closed());
    for target in targets {
        if inner.sessions.contains_key(&target.id) {
            continue;
        }
        let Ok(session) = CdpSession::open(&target, inner.port) else {
            continue;
        };
        let probe = session
            .evaluate(
                r#"Boolean(
                  document.querySelector("main.notion-frame") ||
                  document.querySelector(".notion-sidebar") ||
                  document.querySelector(".notion-sidebar-container") ||
                  document.querySelector(".root.notion-dark-theme") ||
                  document.querySelector(".root.notion-light-theme") ||
                  document.documentElement
                )"#,
            )
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if probe {
            inner.sessions.insert(
                target.id,
                ManagedSession {
                    session,
                    early_script_id: None,
                    revision: None,
                },
            );
        }
    }
    if !inner.paused {
        let payload = inner.payload.clone();
        if let Some(payload) = payload {
            let mut list: Vec<(String, ManagedSession)> = inner.sessions.drain().collect();
            thread::scope(|scope| {
                for chunk in list.chunks_mut(1) {
                    let payload = &payload;
                    scope.spawn(move || {
                        apply_managed_session(&mut chunk[0].1, payload, force);
                    });
                }
            });
            inner.sessions.extend(list);
        }
    }
    target_count.store(inner.sessions.len(), Ordering::Relaxed);
}

pub struct InjectorEngine {
    inner: Arc<Mutex<InjectorInner>>,
    target_count: Arc<AtomicUsize>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InjectorEngine {
    pub fn new(port: u16, browser_id: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(InjectorInner {
                port,
                browser_id,
                sessions: HashMap::new(),
                payload: None,
                paused: false,
            })),
            target_count: Arc::new(AtomicUsize::new(0)),
            stopping: Arc::new(AtomicBool::new(false)),
            thread: None,
        }
    }

    pub fn start(&mut self, payload: ActivePayload) -> Result<(), String> {
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "CDP 注入状态锁已损坏。".to_string())?;
            inner.payload = Some(payload);
            inner.paused = false;
        }
        sync_inner(&self.inner, &self.target_count, false);
        if self.thread.is_none() {
            let inner = Arc::clone(&self.inner);
            let count = Arc::clone(&self.target_count);
            let stopping = Arc::clone(&self.stopping);
            self.thread = Some(
                thread::Builder::new()
                    .name("notion-cdp-sync".to_string())
                    .spawn(move || {
                        while !stopping.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(1200));
                            if !stopping.load(Ordering::Relaxed) {
                                sync_inner(&inner, &count, false);
                            }
                        }
                    })
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(())
    }

    pub fn update(&self, payload: ActivePayload) -> Result<(), String> {
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| "CDP 注入状态锁已损坏。".to_string())?;
            inner.payload = Some(payload);
            inner.paused = false;
        }
        sync_inner(&self.inner, &self.target_count, true);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "CDP 注入状态锁已损坏。".to_string())?;
        inner.paused = true;
        for managed in inner.sessions.values_mut() {
            remove_from_session(managed);
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "CDP 注入状态锁已损坏。".to_string())?;
        inner.paused = true;
        for managed in inner.sessions.values_mut() {
            remove_from_session(managed);
            let _ = managed
                .session
                .send("Page.setBypassCSP", json!({ "enabled": false }));
        }
        inner.sessions.clear();
        self.target_count.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn active_targets(&self) -> u32 {
        self.target_count.load(Ordering::Relaxed) as u32
    }

    pub fn abandon(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.paused = true;
            inner.sessions.clear();
            inner.payload = None;
        }
        self.target_count.store(0, Ordering::Relaxed);
    }
}

impl Drop for InjectorEngine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_websocket_urls() {
        assert!(validate_websocket_url(
            "ws://127.0.0.1:9226/devtools/page/page-1",
            9226,
            "page",
            Some("page-1")
        )
        .is_ok());
        for value in [
            "ws://192.168.1.2:9226/devtools/page/page-1",
            "wss://127.0.0.1:9226/devtools/page/page-1",
            "ws://127.0.0.1:9227/devtools/page/page-1",
            "ws://user@127.0.0.1:9226/devtools/page/page-1",
        ] {
            assert!(validate_websocket_url(value, 9226, "page", Some("page-1")).is_err());
        }
    }

    #[test]
    fn injects_only_notion_app_pages() {
        assert!(is_main_notion_target_url(
            "https://app.notion.com/p/example/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(is_main_notion_target_url(
            "https://www.notion.so/workspace/page"
        ));
        assert!(is_main_notion_target_url(
            "file:///C:/Users/example/AppData/Local/Programs/Notion/resources/app.asar/.webpack/renderer/tabs/index.html"
        ));
        assert!(!is_main_notion_target_url(
            "file:///C:/Users/example/AppData/Local/Programs/Notion/resources/app.asar/.webpack/renderer/other.html"
        ));
        assert!(!is_main_notion_target_url("http://localhost:3000/"));
        assert!(!is_main_notion_target_url(
            "https://evil.example/app.notion.com"
        ));
    }

    #[test]
    fn early_payload_contains_revision_and_cleanup_is_reversible() {
        let payload = early_payload_for("window.test = true", "revision-1");
        assert!(payload.contains("revision-1"));
        assert!(payload.contains("MutationObserver"));
        assert!(REMOVE_RENDERER_PAYLOAD.contains("cleanup"));
        // 移除时先推进运行序号，让还在 decode 的注入轮自杀，避免移除后又装回来。
        assert!(REMOVE_RENDERER_PAYLOAD.contains("__NOTION_BACKGROUND_RUN_SEQ__"));
    }

    #[test]
    fn large_payload_early_script_cleans_tab_chrome() {
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("/tabs/index.html"));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("root.localName !== \"html\""));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("observer.observe(document"));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains(".root *"));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("box-shadow: none !important"));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("[style*="));
        assert!(EARLY_TRANSPARENCY_SCRIPT.contains("background-image: none !important"));
    }

    #[test]
    fn early_transparency_cleanup_runs_before_state_cleanup() {
        let early_style = REMOVE_RENDERER_PAYLOAD
            .find("notion-background-early-transparency")
            .expect("early style cleanup is present");
        let state_lookup = REMOVE_RENDERER_PAYLOAD
            .find("const state =")
            .expect("state cleanup is present");
        assert!(early_style < state_lookup);
    }

    #[test]
    fn full_early_payload_waits_for_the_html_root() {
        let payload = early_payload_for("window.test = true", "revision-1");
        assert!(payload.contains("document.documentElement?.localName !== \"html\""));
    }

    #[test]
    fn document_revision_probe_checks_state_and_style() {
        let probe = document_revision_probe("revision-1");
        assert!(probe.contains("window.__NOTION_BACKGROUND_STUDIO__"));
        assert!(probe.contains("notion-background-style"));
        assert!(probe.contains("cbgRevision"));
        assert!(probe.contains("revision-1"));
    }

    #[test]
    fn matching_document_skips_apply_and_mismatch_reapplies_once() {
        assert!(!should_apply_payload(
            Some("revision-1"),
            "revision-1",
            true,
            false
        ));
        assert!(should_apply_payload(
            Some("revision-1"),
            "revision-1",
            false,
            false
        ));

        let mut managed_revision = Some("revision-1".to_string());
        let mut send_count = 0;
        for document_matches in [false, true] {
            if should_apply_payload(
                managed_revision.as_deref(),
                "revision-1",
                document_matches,
                false,
            ) {
                send_count += 1;
                managed_revision = Some("revision-1".to_string());
            }
        }
        assert_eq!(send_count, 1);
        assert!(!should_apply_payload(
            managed_revision.as_deref(),
            "revision-1",
            true,
            false
        ));
    }

    #[test]
    fn skips_zero_size_restore_pages_but_keeps_tab_bar() {
        assert!(!usable_viewport(0.0, 0.0));
        assert!(!usable_viewport(1920.0, 0.0));
        assert!(!usable_viewport(0.0, 996.0));
        assert!(usable_viewport(1920.0, 36.0));
        assert!(usable_viewport(1920.0, 996.0));
        assert!(VIEWPORT_SIZE_PROBE.contains("innerWidth"));
        assert!(VIEWPORT_SIZE_PROBE.contains("innerHeight"));
    }

    #[test]
    #[ignore = "requires a live Notion CDP endpoint on port 9226"]
    fn reads_live_notion_browser_identity() {
        let identity = read_browser_identity(9226).expect("read live browser identity");
        assert!(valid_id(&identity));
    }

    #[test]
    #[ignore = "requires a live Notion CDP endpoint on port 9226"]
    fn reads_live_notion_with_reqwest() {
        let body = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                reqwest::get("http://127.0.0.1:9226/json/version")
                    .await
                    .expect("connect with reqwest")
                    .text()
                    .await
                    .expect("read response")
            });
        assert!(body.contains("webSocketDebuggerUrl"));
    }

    #[test]
    #[ignore = "requires a live Notion CDP endpoint on port 9226"]
    fn connects_to_live_notion_page_session() {
        let identity = read_browser_identity(9226).expect("read live browser identity");
        let target = list_targets(9226, &identity)
            .expect("list live targets")
            .into_iter()
            .next()
            .expect("at least one Notion page target");
        let session = CdpSession::open(&target, 9226).expect("open CDP page session");
        assert_eq!(
            session
                .evaluate("Boolean(document.documentElement)")
                .expect("evaluate page probe"),
            Value::Bool(true)
        );
        assert_eq!(
            session
                .evaluate("Boolean(window.__NOTION_BACKGROUND_STUDIO__)")
                .expect("verify injected background state"),
            Value::Bool(true)
        );
    }
}
