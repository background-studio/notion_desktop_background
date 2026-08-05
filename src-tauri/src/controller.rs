use std::{
    fs,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wait_timeout::ChildExt;

use crate::{
    injector::{read_browser_identity, InjectorEngine},
    models::RuntimeStatus,
    settings::write_json_transaction,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PREFERRED_DEBUG_PORT: u16 = 9226;

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
    run_powershell(&script, Duration::from_secs(15))
        .unwrap_or_else(|_| "unknown".to_string())
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
    Err("未找到官方 Notion 桌面应用（预期路径：%LOCALAPPDATA%\\Programs\\Notion\\Notion.exe）。"
        .to_string())
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

fn stop_verified_notion(install: &NotionInstall) -> Result<(), String> {
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
    let deadline = Instant::now() + Duration::from_secs(15);
    while !process_ids_for(install)?.is_empty() {
        if Instant::now() >= deadline {
            return Err("Notion 未能在 15 秒内完全退出。".to_string());
        }
        thread::sleep(Duration::from_millis(300));
    }
    Ok(())
}

fn launch_notion(install: &NotionInstall, arguments: &[String]) -> Result<(), String> {
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
        .map_err(|error| format!("启动 Notion 失败：{error}"))?;
    Ok(())
}

fn select_port(preferred: u16) -> Result<u16, String> {
    for port in preferred..=preferred.saturating_add(100) {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    Err("无法为 Notion 分配本机调试端口。".to_string())
}

pub struct NotionController {
    state_path: PathBuf,
    engine: Option<InjectorEngine>,
    state: Option<RuntimeState>,
    status: RuntimeStatus,
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

    pub fn reconnect_saved(
        &mut self,
        payload: String,
        revision: String,
    ) -> Result<bool, String> {
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
            .start(payload, revision);
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
        payload: String,
        revision: String,
        restart_existing: bool,
    ) -> Result<RuntimeStatus, String> {
        self.status.phase = "starting".to_string();
        self.status.message = "正在连接 Notion".to_string();
        self.status.last_error = None;
        let result: Result<RuntimeStatus, String> = (|| {
            let install = discover_notion()?;
            if let Some(engine) = &self.engine {
                engine.update(payload, revision)?;
                self.status.phase = "active".to_string();
                self.status.message = "背景已实时应用".to_string();
                self.status.notion_version = Some(install.version);
                return Ok(self.status());
            }
            if self.try_attach_saved(&install) {
                self.engine
                    .as_mut()
                    .expect("engine set after attach")
                    .start(payload, revision)?;
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
                engine.start(payload.clone(), revision.clone())?;
                self.engine = Some(engine);
                self.status.phase = "active".to_string();
                self.status.message = "已重新连接背景会话".to_string();
                self.status.notion_version = Some(install.version);
                return Ok(self.status());
            }
            let running = process_ids_for(&install)?;
            if !running.is_empty() && !restart_existing {
                return Err("Notion 需要重启一次以启用背景。".to_string());
            }
            if !running.is_empty() {
                stop_verified_notion(&install)?;
            }
            let port = select_port(PREFERRED_DEBUG_PORT)?;
            launch_notion(
                &install,
                &[
                    "--remote-debugging-address=127.0.0.1".to_string(),
                    format!("--remote-debugging-port={port}"),
                ],
            )?;
            let deadline = Instant::now() + Duration::from_secs(45);
            let browser_id = loop {
                if let Ok(identity) = read_browser_identity(port) {
                    break identity;
                }
                if Instant::now() >= deadline {
                    return Err("Notion 未能在 45 秒内打开安全的本机调试端口。".to_string());
                }
                thread::sleep(Duration::from_millis(400));
            };
            self.write_state(Some(RuntimeState {
                schema_version: 1,
                port,
                browser_id: browser_id.clone(),
                package_full_name: install.package_full_name,
                executable: install.executable,
                created_at: Utc::now().to_rfc3339(),
            }))?;
            let mut engine = InjectorEngine::new(port, browser_id);
            engine.start(payload, revision)?;
            self.engine = Some(engine);
            self.status.phase = "active".to_string();
            self.status.message = "背景已应用".to_string();
            self.status.notion_version = Some(install.version);
            Ok(self.status())
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
        result
    }

    pub fn pause(&mut self) -> Result<RuntimeStatus, String> {
        if let Some(engine) = &self.engine {
            engine.pause()?;
        }
        self.status.phase = "paused".to_string();
        self.status.message = "背景已暂停".to_string();
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
            if !process_ids_for(&install)?.is_empty() {
                stop_verified_notion(&install)?;
                launch_notion(&install, &[])?;
            }
            self.write_state(None)?;
            self.status.phase = "idle".to_string();
            self.status.message = "已恢复官方外观".to_string();
            self.status.notion_version = Some(install.version);
            self.status.active_targets = 0;
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
