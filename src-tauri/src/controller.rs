use std::{
    collections::HashSet,
    fs,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wait_timeout::ChildExt;

use crate::{
    injector::{probe_browser_identity, read_browser_identity, InjectorEngine},
    managed_launch::{
        debug_ports_from_records, has_remote_debugging_arg, now_ms, snapshot_matching_executable,
        ManagedObservation, ProcessKey, ProcessTracker,
    },
    models::RuntimeStatus,
    payload::ActivePayload,
    settings::write_json_transaction,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PREFERRED_DEBUG_PORT: u16 = 9226;
/// 强制结束后先同步等这么久；多数情况下 Notion 在此期间就已彻底退出。
const EXIT_QUICK_WAIT: Duration = Duration::from_secs(15);
const DEBUG_PORT_WAIT: Duration = Duration::from_secs(45);
/// 官方外观重启后观察这么久，确认新进程没有因单实例锁被立即放弃。
const OFFICIAL_LAUNCH_SETTLE: Duration = Duration::from_secs(20);
const RELAUNCH_RETRY_INTERVAL: Duration = Duration::from_secs(3);
const RELAUNCH_MAX_ATTEMPTS: u8 = 10;
/// 等旧进程退出的总时长上限；超过后报错，避免无限期停在"等待退出"。
const RELAUNCH_MAX_WAIT: Duration = Duration::from_secs(600);

pub const MSG_WAIT_EXIT_MANAGED: &str = "等待旧的 Notion 进程完全退出，随后自动重启并应用背景";
pub const MSG_WAIT_EXIT_OFFICIAL: &str = "等待旧的 Notion 进程完全退出，随后自动以官方外观重启";
const MSG_RELAUNCH_FAILED: &str =
    "旧的 Notion 进程长时间未释放，自动重启多次失败，请稍后手动启动 Notion。";

/// 强制结束后 Notion 进程可能在进程表中滞留数十秒到数分钟（期间仍占着单实例锁，
/// 新启动的进程会直接退出）。这类情况下不再报错放弃，而是登记一次待重启，由
/// watcher 在旧进程真正消失后重新启动，保证被插件关掉的 Notion 一定会被拉起来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelaunchKind {
    /// 带本机调试端口重启并注入背景。
    Managed,
    /// 以官方外观（无调试参数）重启。
    Official,
}

struct RelaunchPlan {
    kind: RelaunchKind,
    /// 登记时仍滞留的旧进程；出现不在其中的新进程说明用户已自行启动了 Notion。
    lingering: HashSet<ProcessKey>,
    /// 本插件为重启而拉起的主进程 PID，不能被当成"用户自行启动"。
    own_pids: HashSet<u32>,
    attempts: u8,
    not_before: Instant,
    /// 首次登记待重启的时间，重试时沿用。
    since: Instant,
    /// 官方外观重启后正在观察的子进程：若它很快自行退出说明旧实例仍占着锁，需重试。
    watch: Option<(Child, Instant)>,
}

pub enum RelaunchPoll {
    /// 旧进程仍在退出，继续等待。
    Waiting,
    /// 旧进程已全部消失，可以重启。
    Due,
    /// 用户在等待期间自行启动了 Notion，待重启已取消。
    Superseded(RelaunchKind),
}

pub enum RelaunchOutcome {
    Attached,
    Official,
    Retrying,
    Idle,
}

enum DebugLaunch {
    Ready {
        port: u16,
        browser_id: String,
    },
    /// 新进程没打开调试端口就退出了：旧实例仍持有单实例锁。
    ExitedEarly {
        pid: u32,
    },
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NotionInstall {
    package_root: String,
    executable: String,
    version: String,
    package_full_name: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeState {
    schema_version: u8,
    port: u16,
    browser_id: String,
    package_full_name: String,
    executable: String,
    created_at: String,
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn encoded_powershell(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    STANDARD.encode(bytes)
}

fn run_powershell(script: &str, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded_powershell(script),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| error.to_string())?;
    if child
        .wait_timeout(timeout)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err("PowerShell 操作超时。".to_string());
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            if stdout.is_empty() {
                "PowerShell 操作失败。".to_string()
            } else {
                stdout
            }
        } else {
            stderr
        })
    }
}

fn normalized_path(path: &str) -> String {
    Path::new(path)
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn candidate_executables() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("Programs")
                .join("Notion")
                .join("Notion.exe"),
        );
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Notion")
                .join("Notion.exe"),
        );
    }
    candidates
}

fn read_version(executable: &Path) -> String {
    let version_file = executable
        .parent()
        .map(|parent| parent.join("version"))
        .filter(|path| path.is_file());
    if let Some(path) = version_file {
        if let Ok(content) = fs::read_to_string(path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() && trimmed.len() <= 64 {
                return trimmed.to_string();
            }
        }
    }
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$info = [Diagnostics.FileVersionInfo]::GetVersionInfo({})
$version = "$($info.ProductVersion)".Trim()
if (-not $version) {{ $version = "$($info.FileVersion)".Trim() }}
if (-not $version) {{ throw '无法读取 Notion 版本。' }}
$version
"#,
        powershell_quote(&executable.to_string_lossy())
    );
    run_powershell(&script, Duration::from_secs(15)).unwrap_or_else(|_| "unknown".to_string())
}

fn discover_notion() -> Result<NotionInstall, String> {
    for executable in candidate_executables() {
        if !executable.is_file() {
            continue;
        }
        let package_root = executable
            .parent()
            .ok_or_else(|| "Notion 安装路径无效。".to_string())?
            .to_string_lossy()
            .to_string();
        let version = read_version(&executable);
        let package_full_name = format!("Notion.Desktop@{version}");
        let install = NotionInstall {
            package_root,
            executable: executable.to_string_lossy().to_string(),
            version,
            package_full_name,
        };
        let root = format!(
            "{}\\",
            normalized_path(&install.package_root).trim_end_matches('\\')
        );
        if !normalized_path(&install.executable).starts_with(&root) {
            continue;
        }
        if !normalized_path(&install.executable).ends_with("\\notion.exe") {
            continue;
        }
        return Ok(install);
    }
    Err(
        "未找到官方 Notion 桌面应用（预期路径：%LOCALAPPDATA%\\Programs\\Notion\\Notion.exe）。"
            .to_string(),
    )
}

fn process_ids_for(install: &NotionInstall) -> Result<Vec<u32>, String> {
    let script = format!(
        r#"
$target = {}
$ids = @(Get-CimInstance Win32_Process -Filter "Name='Notion.exe'" | Where-Object {{
  $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath).Equals($target, [StringComparison]::OrdinalIgnoreCase)
}} | ForEach-Object {{ [int]$_.ProcessId }})
@($ids) | ConvertTo-Json -Compress
"#,
        powershell_quote(&normalized_path(&install.executable))
    );
    let raw = run_powershell(&script, Duration::from_secs(30))?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    Ok(match value {
        Value::Array(values) => values
            .into_iter()
            .filter_map(|value| value.as_u64())
            .filter_map(|value| u32::try_from(value).ok())
            .collect(),
        Value::Number(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .into_iter()
            .collect(),
        _ => Vec::new(),
    })
}

fn debug_ports_for(install: &NotionInstall) -> Result<Vec<u16>, String> {
    let script = format!(
        r#"
$target = {}
$ports = @(Get-CimInstance Win32_Process -Filter "Name='Notion.exe'" | Where-Object {{
  $_.ExecutablePath -and $_.CommandLine -and
  [IO.Path]::GetFullPath($_.ExecutablePath).Equals($target, [StringComparison]::OrdinalIgnoreCase)
}} | ForEach-Object {{
  $match = [regex]::Match("$($_.CommandLine)", '(?:^|\s)"?--remote-debugging-port=(\d+)"?(?:\s|$)')
  if ($match.Success) {{ [int]$match.Groups[1].Value }}
}})
@($ports | Sort-Object -Unique) | ConvertTo-Json -Compress
"#,
        powershell_quote(&normalized_path(&install.executable))
    );
    let raw = run_powershell(&script, Duration::from_secs(30))?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let values = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    Ok(values
        .into_iter()
        .filter_map(|value| value.as_u64())
        .filter_map(|value| u16::try_from(value).ok())
        .collect())
}

/// 结束所有路径匹配的 Notion 进程。返回 `false` 表示进程在快速等待窗口内
/// 尚未从进程表消失（仍在退出），调用方须登记待重启而不是立即启动。
fn stop_verified_notion(install: &NotionInstall) -> Result<bool, String> {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$target = {}
$processes = @(Get-CimInstance Win32_Process -Filter "Name='Notion.exe'" | Where-Object {{
  $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath).Equals($target, [StringComparison]::OrdinalIgnoreCase)
}})
foreach ($item in $processes) {{ Stop-Process -Id ([int]$item.ProcessId) -Force -ErrorAction SilentlyContinue }}
"#,
        powershell_quote(&normalized_path(&install.executable))
    );
    run_powershell(&script, Duration::from_secs(30))?;
    let deadline = Instant::now() + EXIT_QUICK_WAIT;
    while !process_ids_for(install)?.is_empty() {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(300));
    }
    Ok(true)
}

fn launch_notion(install: &NotionInstall, arguments: &[String]) -> Result<Child, String> {
    let executable = PathBuf::from(&install.executable);
    if !executable.is_file() {
        return Err("Notion 可执行文件不存在。".to_string());
    }
    Command::new(&executable)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("启动 Notion 失败：{error}"))
}

fn launch_debug_session(install: &NotionInstall) -> Result<DebugLaunch, String> {
    let port = select_port(PREFERRED_DEBUG_PORT)?;
    let mut child = launch_notion(
        install,
        &[
            "--remote-debugging-address=127.0.0.1".to_string(),
            format!("--remote-debugging-port={port}"),
        ],
    )?;
    let deadline = Instant::now() + DEBUG_PORT_WAIT;
    loop {
        if let Ok(browser_id) = read_browser_identity(port) {
            return Ok(DebugLaunch::Ready { port, browser_id });
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Ok(DebugLaunch::ExitedEarly { pid: child.id() });
        }
        if Instant::now() >= deadline {
            // 本次启动的进程一直没开调试端口：结束它，避免挡住后续重试（单实例锁）。
            let _ = child.kill();
            let _ = child.wait();
            return Err("Notion 未能在 45 秒内打开安全的本机调试端口。".to_string());
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn lingering_keys(install: &NotionInstall) -> HashSet<ProcessKey> {
    snapshot_matching_executable(&install.executable)
        .map(|records| records.into_iter().map(|record| record.key).collect())
        .unwrap_or_default()
}

fn select_port(preferred: u16) -> Result<u16, String> {
    for port in preferred..=preferred.saturating_add(100) {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("无法为 Notion 分配本机调试端口。".to_string())
}

pub struct TargetProbe {
    pub processes: Vec<ProcessKey>,
    pub has_live_debug_session: bool,
    pub has_debug_launch: bool,
    pub engine_connected: bool,
}

impl TargetProbe {
    pub fn empty() -> Self {
        Self {
            processes: Vec::new(),
            has_live_debug_session: false,
            has_debug_launch: false,
            engine_connected: false,
        }
    }

    pub fn observation(&self) -> ManagedObservation {
        ManagedObservation {
            processes: self.processes.clone(),
            has_live_debug_session: self.has_live_debug_session,
            has_debug_launch: self.has_debug_launch,
            engine_connected: self.engine_connected,
            now_ms: now_ms(),
        }
    }
}

pub struct NotionController {
    state_path: PathBuf,
    engine: Option<InjectorEngine>,
    state: Option<RuntimeState>,
    status: RuntimeStatus,
    install_cache: Option<(Instant, NotionInstall)>,
    tracker: ProcessTracker,
    managed_mode: bool,
    managed_paused: bool,
    rearm_requested: bool,
    relaunch: Option<RelaunchPlan>,
}

impl NotionController {
    pub fn load(data_directory: &Path) -> Self {
        let state_path = data_directory.join("runtime.json");
        let state = fs::read_to_string(&state_path)
            .ok()
            .and_then(|content| serde_json::from_str::<RuntimeState>(&content).ok())
            .filter(|state| state.schema_version == 1 && !state.browser_id.is_empty());
        Self {
            state_path,
            engine: None,
            state,
            status: RuntimeStatus::default(),
            install_cache: None,
            tracker: ProcessTracker::new(),
            managed_mode: false,
            managed_paused: false,
            rearm_requested: false,
            relaunch: None,
        }
    }

    pub fn status(&self) -> RuntimeStatus {
        let mut status = self.status.clone();
        if let Some(engine) = &self.engine {
            status.active_targets = engine.active_targets();
        }
        status
    }

    fn write_state(&mut self, state: Option<RuntimeState>) -> Result<(), String> {
        self.state = state;
        match &self.state {
            Some(state) => write_json_transaction(&self.state_path, state),
            None => match fs::remove_file(&self.state_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            },
        }
    }

    fn discover_cached(&mut self) -> Result<NotionInstall, String> {
        if let Some((at, install)) = &self.install_cache {
            if at.elapsed() < Duration::from_secs(15) && Path::new(&install.executable).is_file() {
                return Ok(install.clone());
            }
        }
        let install = discover_notion()?;
        self.install_cache = Some((Instant::now(), install.clone()));
        Ok(install)
    }

    fn saved_session_matches(&self, identity: Result<String, String>) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        identity.ok().as_deref() == Some(state.browser_id.as_str())
    }

    fn saved_session_live(&self) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        self.saved_session_matches(read_browser_identity(state.port))
    }

    fn saved_session_probe_live(&self) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        self.saved_session_matches(probe_browser_identity(state.port))
    }

    fn drop_dead_engine(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            engine.abandon();
        }
    }

    pub fn relaunch_pending(&self) -> bool {
        self.relaunch.is_some()
    }

    /// 暂停（含恢复官方外观）期间不再注入，待重启统一按官方外观执行。
    fn effective_relaunch_kind(&self, kind: RelaunchKind) -> RelaunchKind {
        if self.managed_paused {
            RelaunchKind::Official
        } else {
            kind
        }
    }

    fn show_relaunch_wait(&mut self, kind: RelaunchKind) {
        let (phase, message) = match self.effective_relaunch_kind(kind) {
            RelaunchKind::Managed => ("starting", MSG_WAIT_EXIT_MANAGED),
            RelaunchKind::Official => ("restoring", MSG_WAIT_EXIT_OFFICIAL),
        };
        self.status.phase = phase.to_string();
        self.status.message = message.to_string();
        self.status.last_error = None;
        self.status.active_targets = 0;
    }

    fn fail_relaunch(&mut self, error: &str) {
        self.relaunch = None;
        self.status.phase = "error".to_string();
        self.status.message = error.to_string();
        self.status.last_error = Some(error.to_string());
    }

    /// 登记待重启。`attempts` 为已经失败的启动次数；`own_pid` 是刚刚自行退出的、
    /// 由本插件启动的主进程（它可能仍滞留在进程表中）。
    fn schedule_relaunch(
        &mut self,
        install: &NotionInstall,
        kind: RelaunchKind,
        attempts: u8,
        own_pid: Option<u32>,
        delay: Duration,
    ) {
        let (mut own_pids, since) = self
            .relaunch
            .take()
            .map(|plan| (plan.own_pids, plan.since))
            .unwrap_or_else(|| (HashSet::new(), Instant::now()));
        own_pids.extend(own_pid);
        self.relaunch = Some(RelaunchPlan {
            kind,
            lingering: lingering_keys(install),
            own_pids,
            attempts,
            not_before: Instant::now() + delay,
            since,
            watch: None,
        });
        self.show_relaunch_wait(kind);
    }

    fn finish_official_relaunch(&mut self, kind: RelaunchKind) {
        let (phase, message) = if self.managed_paused {
            ("paused", "暂停托管")
        } else if kind == RelaunchKind::Official {
            ("idle", "已恢复官方外观")
        } else {
            ("idle", "已重新启动 Notion")
        };
        self.status.phase = phase.to_string();
        self.status.message = message.to_string();
        self.status.last_error = None;
        self.status.active_targets = 0;
    }

    /// 以官方外观启动，并登记观察：新进程若很快自行退出，由 watcher 重试。
    fn launch_official_watched(
        &mut self,
        install: &NotionInstall,
        kind: RelaunchKind,
        attempts: u8,
    ) -> Result<(), String> {
        let child = launch_notion(install, &[])?;
        let (mut own_pids, since) = self
            .relaunch
            .take()
            .map(|plan| (plan.own_pids, plan.since))
            .unwrap_or_else(|| (HashSet::new(), Instant::now()));
        own_pids.insert(child.id());
        self.relaunch = Some(RelaunchPlan {
            kind,
            lingering: HashSet::new(),
            own_pids,
            attempts,
            not_before: Instant::now(),
            since,
            watch: Some((child, Instant::now() + OFFICIAL_LAUNCH_SETTLE)),
        });
        self.finish_official_relaunch(kind);
        Ok(())
    }

    /// watcher 每个 tick 调用：判断旧进程是否已彻底退出、用户是否已自行启动 Notion。
    pub fn poll_relaunch(&mut self) -> Result<RelaunchPoll, String> {
        let install = self.discover_cached()?;
        let Some(plan) = self.relaunch.as_mut() else {
            return Ok(RelaunchPoll::Waiting);
        };
        let kind = plan.kind;
        if let Some((child, deadline)) = plan.watch.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                let attempts = plan.attempts.saturating_add(1);
                if attempts >= RELAUNCH_MAX_ATTEMPTS {
                    self.fail_relaunch(MSG_RELAUNCH_FAILED);
                    return Err(MSG_RELAUNCH_FAILED.to_string());
                }
                self.schedule_relaunch(&install, kind, attempts, None, RELAUNCH_RETRY_INTERVAL);
            } else if Instant::now() >= *deadline {
                self.relaunch = None;
            }
            return Ok(RelaunchPoll::Waiting);
        }
        let records = snapshot_matching_executable(&install.executable)?;
        let user_launched = records.iter().any(|record| {
            !plan.lingering.contains(&record.key)
                && !plan.own_pids.contains(&record.key.pid)
                && record
                    .command_line
                    .as_deref()
                    .is_some_and(|line| !line.contains("--type="))
        });
        if user_launched {
            self.relaunch = None;
            if self.effective_relaunch_kind(kind) == RelaunchKind::Official {
                self.finish_official_relaunch(kind);
            }
            return Ok(RelaunchPoll::Superseded(kind));
        }
        let due = records.is_empty() && Instant::now() >= plan.not_before;
        if !due && plan.since.elapsed() >= RELAUNCH_MAX_WAIT {
            self.fail_relaunch(MSG_RELAUNCH_FAILED);
            return Err(MSG_RELAUNCH_FAILED.to_string());
        }
        self.show_relaunch_wait(kind);
        Ok(if due {
            RelaunchPoll::Due
        } else {
            RelaunchPoll::Waiting
        })
    }

    /// 旧进程已全部退出后由 watcher 调用：按待重启类型重新启动 Notion。
    pub fn run_pending_relaunch(
        &mut self,
        payload: Option<ActivePayload>,
    ) -> Result<RelaunchOutcome, String> {
        let Some(plan) = self.relaunch.as_ref() else {
            return Ok(RelaunchOutcome::Idle);
        };
        if plan.watch.is_some() {
            return Ok(RelaunchOutcome::Idle);
        }
        let kind = plan.kind;
        let attempts = plan.attempts;
        let install = match discover_notion() {
            Ok(install) => install,
            Err(error) => {
                self.fail_relaunch(&error);
                return Err(error);
            }
        };
        if !snapshot_matching_executable(&install.executable)?.is_empty() {
            return Ok(RelaunchOutcome::Retrying);
        }
        let payload = match self.effective_relaunch_kind(kind) {
            RelaunchKind::Managed => payload,
            RelaunchKind::Official => None,
        };
        let Some(payload) = payload else {
            if let Err(error) = self.launch_official_watched(&install, kind, attempts) {
                return self.retry_after_launch_error(&install, kind, attempts, None, error, false);
            }
            return Ok(RelaunchOutcome::Official);
        };
        self.status.phase = "starting".to_string();
        self.status.message = "正在重新启动 Notion".to_string();
        match launch_debug_session(&install) {
            Ok(DebugLaunch::Ready { port, browser_id }) => {
                if let Err(error) = self.attach_launched(&install, port, browser_id, payload) {
                    self.fail_relaunch(&error);
                    return Err(error);
                }
                self.status.message = "背景已自动应用".to_string();
                Ok(RelaunchOutcome::Attached)
            }
            Ok(DebugLaunch::ExitedEarly { pid }) => self.retry_after_launch_error(
                &install,
                kind,
                attempts,
                Some(pid),
                MSG_RELAUNCH_FAILED.to_string(),
                true,
            ),
            Err(error) => {
                self.retry_after_launch_error(&install, kind, attempts, None, error, false)
            }
        }
    }

    /// 被插件关掉的 Notion 必须重新拉起来，启动失败不能直接放弃：
    /// - 仍有滞留进程（含刚失败的这次启动留下的）时继续等待，不计入失败次数，
    ///   总时长由 `RELAUNCH_MAX_WAIT` 兜底；
    /// - 进程已清空仍失败，则计一次失败，间隔后重试，达到上限才报告最后一次错误。
    fn retry_after_launch_error(
        &mut self,
        install: &NotionInstall,
        kind: RelaunchKind,
        attempts: u8,
        own_pid: Option<u32>,
        error: String,
        always_count: bool,
    ) -> Result<RelaunchOutcome, String> {
        let cleared = snapshot_matching_executable(&install.executable)
            .map(|records| records.is_empty())
            .unwrap_or(false);
        let attempts = if always_count || cleared {
            attempts.saturating_add(1)
        } else {
            attempts
        };
        if attempts >= RELAUNCH_MAX_ATTEMPTS {
            let error = format!("{MSG_RELAUNCH_FAILED}（{error}）");
            self.fail_relaunch(&error);
            return Err(error);
        }
        self.schedule_relaunch(install, kind, attempts, own_pid, RELAUNCH_RETRY_INTERVAL);
        Ok(RelaunchOutcome::Retrying)
    }

    /// 记录新启动的调试会话并开始注入。
    fn attach_launched(
        &mut self,
        install: &NotionInstall,
        port: u16,
        browser_id: String,
        payload: ActivePayload,
    ) -> Result<(), String> {
        self.write_state(Some(RuntimeState {
            schema_version: 1,
            port,
            browser_id: browser_id.clone(),
            package_full_name: install.package_full_name.clone(),
            executable: install.executable.clone(),
            created_at: Utc::now().to_rfc3339(),
        }))?;
        let mut engine = InjectorEngine::new(port, browser_id);
        engine.start(payload)?;
        self.engine = Some(engine);
        self.relaunch = None;
        self.status.phase = "active".to_string();
        self.status.message = "背景已应用".to_string();
        self.status.last_error = None;
        self.status.notion_version = Some(install.version.clone());
        Ok(())
    }

    fn mark_managed_applied(&mut self) {
        if self.managed_mode {
            self.managed_paused = false;
            self.rearm_requested = true;
        }
    }

    pub fn enable_managed_mode(&mut self) {
        self.managed_mode = true;
        if self.engine.is_none() && self.status.phase == "idle" {
            self.status.message = "已启用，等待 Notion 启动".to_string();
        }
    }

    pub fn managed_paused(&self) -> bool {
        self.managed_paused
    }

    pub fn take_rearm_request(&mut self) -> bool {
        let requested = self.rearm_requested;
        self.rearm_requested = false;
        requested
    }

    pub fn engine_is_live(&self) -> bool {
        self.engine.is_some() && self.saved_session_live()
    }

    pub fn set_watch_status(&mut self, phase: &str, message: &str) -> bool {
        if self.managed_paused && phase != "paused" {
            return false;
        }
        if self.status.phase == phase && self.status.message == message {
            return false;
        }
        self.status.phase = phase.to_string();
        self.status.message = message.to_string();
        if phase == "error" {
            self.status.last_error = Some(message.to_string());
        } else {
            self.status.last_error = None;
        }
        true
    }

    pub fn probe_target(&mut self) -> Result<TargetProbe, String> {
        let install = self.discover_cached()?;
        let records = self.tracker.scan(&install.executable)?;
        let processes = records.iter().map(|record| record.key).collect();
        let has_debug_launch = records.iter().any(|record| {
            record
                .command_line
                .as_deref()
                .is_some_and(has_remote_debugging_arg)
        });
        let live_saved = self.saved_session_probe_live();
        let engine_connected = self.engine.is_some() && live_saved;
        if self.engine.is_some() && !live_saved {
            self.drop_dead_engine();
        }
        let live_from_ports = !live_saved
            && debug_ports_from_records(&records)
                .into_iter()
                .any(|port| probe_browser_identity(port).is_ok());
        Ok(TargetProbe {
            processes,
            has_live_debug_session: live_saved || live_from_ports,
            has_debug_launch,
            engine_connected,
        })
    }

    pub fn release_stale_session(&mut self) -> Result<(), String> {
        self.drop_dead_engine();
        self.write_state(None)?;
        self.status.active_targets = 0;
        Ok(())
    }

    fn try_attach_inner(&mut self, payload: ActivePayload) -> Result<bool, String> {
        let install = discover_notion()?;
        if let Some(mut engine) = self.engine.take() {
            if self.saved_session_live() {
                match engine.update(payload.clone()) {
                    Ok(()) => {
                        self.engine = Some(engine);
                        self.status.phase = "active".to_string();
                        self.status.message = "背景已自动应用".to_string();
                        self.status.notion_version = Some(install.version);
                        return Ok(true);
                    }
                    Err(_) => engine.abandon(),
                }
            } else {
                engine.abandon();
            }
        }
        if self.try_attach_saved(&install) {
            self.engine
                .as_mut()
                .expect("engine set after attach")
                .start(payload)?;
            self.status.phase = "active".to_string();
            self.status.message = "背景已自动应用".to_string();
            self.status.notion_version = Some(install.version);
            return Ok(true);
        }
        let records = snapshot_matching_executable(&install.executable)?;
        let mut ports = debug_ports_from_records(&records);
        if let Some(state) = &self.state {
            ports.push(state.port);
        }
        if ports.is_empty() {
            if let Ok(extra) = debug_ports_for(&install) {
                ports.extend(extra);
            }
        }
        ports.sort_unstable();
        ports.dedup();
        for port in ports {
            let Ok(browser_id) = read_browser_identity(port) else {
                continue;
            };
            self.write_state(Some(RuntimeState {
                schema_version: 1,
                port,
                browser_id: browser_id.clone(),
                package_full_name: install.package_full_name.clone(),
                executable: install.executable.clone(),
                created_at: Utc::now().to_rfc3339(),
            }))?;
            let mut engine = InjectorEngine::new(port, browser_id);
            engine.start(payload.clone())?;
            self.engine = Some(engine);
            self.status.phase = "active".to_string();
            self.status.message = "背景已自动应用".to_string();
            self.status.notion_version = Some(install.version);
            return Ok(true);
        }
        Ok(false)
    }

    pub fn try_attach_current(&mut self, payload: ActivePayload) -> Result<bool, String> {
        self.status.phase = "starting".to_string();
        self.status.message = "正在接管 Notion".to_string();
        self.status.last_error = None;
        match self.try_attach_inner(payload) {
            Ok(attached) => {
                if attached {
                    self.relaunch = None;
                    self.mark_managed_applied();
                }
                Ok(attached)
            }
            Err(error) => {
                self.status.phase = "error".to_string();
                self.status.message = error.clone();
                self.status.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn takeover_unmanaged(&mut self, payload: ActivePayload) -> Result<RuntimeStatus, String> {
        self.status.phase = "starting".to_string();
        self.status.message = "正在接管 Notion".to_string();
        self.status.last_error = None;
        let install = match discover_notion() {
            Ok(install) => install,
            Err(error) => {
                self.status.phase = "error".to_string();
                self.status.message = error.clone();
                self.status.last_error = Some(error.clone());
                return Err(error);
            }
        };
        let records = snapshot_matching_executable(&install.executable).unwrap_or_default();
        if records.iter().any(|record| {
            record
                .command_line
                .as_deref()
                .is_some_and(has_remote_debugging_arg)
        }) {
            return match self.try_attach_current(payload) {
                Ok(_) => Ok(self.status()),
                Err(error) => Err(error),
            };
        }
        if let Ok(ports) = debug_ports_for(&install) {
            if !ports.is_empty() {
                return match self.try_attach_current(payload) {
                    Ok(_) => Ok(self.status()),
                    Err(error) => Err(error),
                };
            }
        }
        let result = self.apply(payload, true);
        if result.is_ok() && self.managed_mode && self.status.phase == "active" {
            self.status.message = "背景已自动应用".to_string();
        }
        result
    }

    fn try_attach_saved(&mut self, install: &NotionInstall) -> bool {
        let Some(state) = &self.state else {
            return false;
        };
        if state.package_full_name != install.package_full_name
            || normalized_path(&state.executable) != normalized_path(&install.executable)
            || read_browser_identity(state.port).ok().as_deref() != Some(&state.browser_id)
        {
            return false;
        }
        self.engine = Some(InjectorEngine::new(state.port, state.browser_id.clone()));
        true
    }

    pub fn reconnect_saved(&mut self, payload: ActivePayload) -> Result<bool, String> {
        if self.state.is_none() {
            return Ok(false);
        }
        let install = discover_notion()?;
        if !self.try_attach_saved(&install) {
            self.write_state(None)?;
            self.status = RuntimeStatus::default();
            return Ok(false);
        }
        let result = self
            .engine
            .as_mut()
            .expect("engine set after saved session validation")
            .start(payload);
        match result {
            Ok(()) => {
                self.status.phase = "active".to_string();
                self.status.message = "已自动恢复背景会话".to_string();
                self.status.notion_version = Some(install.version);
                self.status.last_error = None;
                Ok(true)
            }
            Err(error) => {
                self.engine = None;
                self.status.phase = "error".to_string();
                self.status.message = error.clone();
                self.status.last_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn apply(
        &mut self,
        payload: ActivePayload,
        restart_existing: bool,
    ) -> Result<RuntimeStatus, String> {
        self.status.phase = "starting".to_string();
        self.status.message = if self.managed_mode {
            "正在接管 Notion".to_string()
        } else {
            "正在连接 Notion".to_string()
        };
        self.status.last_error = None;
        let result: Result<RuntimeStatus, String> = (|| {
            let install = discover_notion()?;
            if let Some(mut engine) = self.engine.take() {
                if self.saved_session_live() {
                    match engine.update(payload.clone()) {
                        Ok(()) => {
                            self.engine = Some(engine);
                            self.status.phase = "active".to_string();
                            self.status.message = "背景已实时应用".to_string();
                            self.status.notion_version = Some(install.version);
                            return Ok(self.status());
                        }
                        Err(_) => engine.abandon(),
                    }
                } else {
                    engine.abandon();
                }
            }
            if self.try_attach_saved(&install) {
                self.engine
                    .as_mut()
                    .expect("engine set after attach")
                    .start(payload)?;
                self.status.phase = "active".to_string();
                self.status.message = "已重新连接背景会话".to_string();
                self.status.notion_version = Some(install.version);
                return Ok(self.status());
            }
            for port in debug_ports_for(&install)? {
                let Ok(browser_id) = read_browser_identity(port) else {
                    continue;
                };
                self.write_state(Some(RuntimeState {
                    schema_version: 1,
                    port,
                    browser_id: browser_id.clone(),
                    package_full_name: install.package_full_name.clone(),
                    executable: install.executable.clone(),
                    created_at: Utc::now().to_rfc3339(),
                }))?;
                let mut engine = InjectorEngine::new(port, browser_id);
                engine.start(payload.clone())?;
                self.engine = Some(engine);
                self.status.phase = "active".to_string();
                self.status.message = "已重新连接背景会话".to_string();
                self.status.notion_version = Some(install.version);
                return Ok(self.status());
            }
            // 正在等待被结束的旧进程退出：不再重复结束/等待，保持待重启即可。
            if let Some(plan) = self.relaunch.as_mut() {
                if plan.watch.is_none() {
                    let records = snapshot_matching_executable(&install.executable)?;
                    if !records.is_empty()
                        && records
                            .iter()
                            .all(|record| plan.lingering.contains(&record.key))
                    {
                        plan.kind = RelaunchKind::Managed;
                        self.show_relaunch_wait(RelaunchKind::Managed);
                        return Ok(self.status());
                    }
                }
            }
            let running = process_ids_for(&install)?;
            if !running.is_empty() && !restart_existing {
                return Err("Notion 需要重启一次以启用背景。".to_string());
            }
            let killed = !running.is_empty();
            if killed && !stop_verified_notion(&install)? {
                // 已被结束但仍滞留在进程表中，此时启动会被单实例锁吞掉。
                self.schedule_relaunch(&install, RelaunchKind::Managed, 0, None, Duration::ZERO);
                return Ok(self.status());
            }
            let launched = match launch_debug_session(&install) {
                Ok(launched) => launched,
                // 刚被本插件结束的 Notion 不能就此停在关闭状态：转入待重启继续尝试。
                Err(error) if killed => {
                    self.retry_after_launch_error(
                        &install,
                        RelaunchKind::Managed,
                        0,
                        None,
                        error,
                        false,
                    )?;
                    return Ok(self.status());
                }
                Err(error) => return Err(error),
            };
            match launched {
                DebugLaunch::Ready { port, browser_id } => {
                    self.attach_launched(&install, port, browser_id, payload)?;
                    Ok(self.status())
                }
                DebugLaunch::ExitedEarly { pid } => {
                    self.schedule_relaunch(
                        &install,
                        RelaunchKind::Managed,
                        1,
                        Some(pid),
                        RELAUNCH_RETRY_INTERVAL,
                    );
                    Ok(self.status())
                }
            }
        })();
        if let Err(error) = &result {
            self.status.phase = if error.contains("需要重启一次") {
                "idle".to_string()
            } else {
                "error".to_string()
            };
            self.status.message = error.clone();
            self.status.last_error = Some(error.clone());
        }
        if result.is_ok() {
            if self.status.phase == "active" {
                self.relaunch = None;
            }
            self.mark_managed_applied();
        }
        result
    }

    pub fn pause(&mut self) -> Result<RuntimeStatus, String> {
        if let Some(engine) = &self.engine {
            engine.pause()?;
        }
        self.status.phase = "paused".to_string();
        if self.managed_mode {
            self.managed_paused = true;
            self.status.message = "暂停托管".to_string();
        } else {
            self.status.message = "背景已暂停".to_string();
        }
        self.status.last_error = None;
        Ok(self.status())
    }

    pub fn restore(&mut self) -> Result<RuntimeStatus, String> {
        self.status.phase = "restoring".to_string();
        self.status.message = "正在恢复官方外观".to_string();
        self.status.last_error = None;
        let result: Result<RuntimeStatus, String> = (|| {
            if let Some(mut engine) = self.engine.take() {
                engine.stop()?;
            }
            let install = discover_notion()?;
            let watching = self
                .relaunch
                .as_ref()
                .is_some_and(|plan| plan.watch.is_some());
            let mut pending = false;
            if !process_ids_for(&install)?.is_empty() {
                if watching {
                    // 刚以官方外观重启过且仍在观察，不再重复结束。
                } else if stop_verified_notion(&install)? {
                    self.launch_official_watched(&install, RelaunchKind::Official, 0)?;
                } else {
                    pending = true;
                }
            } else if self.relaunch.is_some() && !watching {
                // 之前被结束的 Notion 尚未重启：改为以官方外观重启。
                pending = true;
            }
            self.write_state(None)?;
            if self.managed_mode {
                self.managed_paused = true;
                self.status.phase = "paused".to_string();
                self.status.message = "暂停托管".to_string();
            } else {
                self.status.phase = "idle".to_string();
                self.status.message = "已恢复官方外观".to_string();
            }
            self.status.notion_version = Some(install.version.clone());
            self.status.active_targets = 0;
            if pending {
                self.schedule_relaunch(&install, RelaunchKind::Official, 0, None, Duration::ZERO);
            } else if let Some(plan) = self.relaunch.as_mut() {
                plan.kind = RelaunchKind::Official;
            }
            Ok(self.status())
        })();
        if let Err(error) = &result {
            self.status.phase = "error".to_string();
            self.status.message = error.clone();
            self.status.last_error = Some(error.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_powershell_literals() {
        assert_eq!(powershell_quote("a'b"), "'a''b'");
    }

    #[test]
    fn selects_an_available_loopback_port() {
        let port = select_port(39_000).expect("available test port");
        assert!((39_000..=39_100).contains(&port));
    }

    #[test]
    #[ignore = "requires the official Notion desktop installation"]
    fn discovers_installed_notion_and_reads_processes() {
        let install = discover_notion().expect("discover official Notion");
        assert!(normalized_path(&install.executable).ends_with("\\notion.exe"));
        process_ids_for(&install).expect("query verified Notion processes");
        for port in debug_ports_for(&install).expect("query verified Notion debug ports") {
            read_browser_identity(port).expect("verify Notion browser identity");
        }
    }
}
