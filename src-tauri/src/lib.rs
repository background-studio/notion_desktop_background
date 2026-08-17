mod controller;
mod injector;
mod managed_launch;
mod models;
mod payload;
mod plugin;
mod plugin_ipc;
mod protocol;
mod settings;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use controller::{NotionController, TargetProbe};
use managed_launch::{ManagedAction, ManagedLaunchMachine};
use models::RuntimeStatus;
use payload::{build_active_payload_from_bytes, ActivePayload};
use plugin::UNCONFIGURED_MESSAGE;
use protocol::{fetch_configured_media, parse_configure, ConfigureCommand};
use serde_json::{json, Value};

pub struct ConfiguredSession {
    pub revision: String,
    pub payload: ActivePayload,
}

pub struct WorkerState {
    pub data_directory: PathBuf,
    pub controller: Arc<Mutex<NotionController>>,
    pub managed_machine: Mutex<ManagedLaunchMachine>,
    pub runtime_status: Mutex<RuntimeStatus>,
    pub configured: Mutex<Option<ConfiguredSession>>,
    pub quitting: AtomicBool,
    pub live_apply_generation: AtomicU64,
}

fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    value.lock().map_err(|_| "应用状态锁已损坏。".to_string())
}

fn data_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("NotionBackgroundStudio")
}

impl WorkerState {
    pub fn load() -> Result<Self, String> {
        Self::load_from(data_directory())
    }

    pub fn load_from(data_directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&data_directory).map_err(|error| error.to_string())?;
        let controller = NotionController::load(&data_directory);
        let runtime_status = controller.status();
        Ok(Self {
            data_directory,
            controller: Arc::new(Mutex::new(controller)),
            managed_machine: Mutex::new(ManagedLaunchMachine::new()),
            runtime_status: Mutex::new(runtime_status),
            configured: Mutex::new(None),
            quitting: AtomicBool::new(false),
            live_apply_generation: AtomicU64::new(0),
        })
    }

    pub fn is_configured(&self) -> bool {
        lock(&self.configured)
            .ok()
            .is_some_and(|session| session.is_some())
    }

    pub fn configured_revision(&self) -> Option<String> {
        lock(&self.configured)
            .ok()
            .and_then(|session| session.as_ref().map(|session| session.revision.clone()))
    }

    pub fn active_payload(&self) -> Result<ActivePayload, String> {
        lock(&self.configured)?
            .as_ref()
            .map(|session| session.payload.clone())
            .ok_or_else(|| UNCONFIGURED_MESSAGE.to_string())
    }

    pub fn runtime_status(&self) -> Result<RuntimeStatus, String> {
        if let Ok(controller) = self.controller.try_lock() {
            let status = controller.status();
            *lock(&self.runtime_status)? = status.clone();
            return Ok(status);
        }
        Ok(lock(&self.runtime_status)?.clone())
    }

    pub fn refresh_runtime_status(&self) -> Result<RuntimeStatus, String> {
        let status = lock(&self.controller)?.status();
        *lock(&self.runtime_status)? = status.clone();
        Ok(status)
    }

    pub fn status_value(&self) -> Result<Value, String> {
        let status = self.runtime_status()?;
        let configured = self.is_configured();
        let message = if !configured && status.phase != "paused" && status.phase != "error" {
            UNCONFIGURED_MESSAGE.to_string()
        } else {
            status.message
        };
        Ok(json!({
            "pluginProtocol": plugin::PLUGIN_PROTOCOL,
            "pluginId": plugin::PLUGIN_ID,
            "version": env!("CARGO_PKG_VERSION"),
            "phase": status.phase,
            "message": message,
            "activeTargets": status.active_targets,
            "paused": status.phase == "paused",
            "configured": configured,
            "revision": self.configured_revision(),
        }))
    }

    pub async fn configure(&self, params: Option<&Value>) -> Result<Value, String> {
        let command = parse_configure(params)?;
        let bytes = fetch_configured_media(&command.media).await?;
        let payload = build_active_payload_from_bytes(
            bytes,
            command.media.mime_type.clone(),
            &command.media.kind,
            &command.display,
        )?;
        self.store_configuration(command, payload)?;
        self.status_value()
    }

    fn store_configuration(
        &self,
        command: ConfigureCommand,
        payload: ActivePayload,
    ) -> Result<(), String> {
        let was_active = self.runtime_status()?.phase == "active";
        *lock(&self.configured)? = Some(ConfiguredSession {
            revision: command.revision,
            payload: payload.clone(),
        });
        if let Ok(mut machine) = lock(&self.managed_machine) {
            if machine.payload_failed() {
                machine.retry_after_payload_ready();
            }
        }
        if was_active {
            lock(&self.controller)?.apply(payload, false)?;
            let _ = self.refresh_runtime_status();
        }
        Ok(())
    }

    pub fn apply(&self) -> Result<Value, String> {
        let payload = self.active_payload()?;
        let mut controller = lock(&self.controller)?;
        match controller.apply(payload.clone(), false) {
            Ok(_) => {}
            Err(error) if error.contains("需要重启一次") => {
                controller.apply(payload, true)?;
            }
            Err(error) => return Err(error),
        }
        drop(controller);
        rearm_managed_machine(self)?;
        let _ = self.refresh_runtime_status();
        self.status_value()
    }

    pub fn pause(&self) -> Result<Value, String> {
        self.live_apply_generation.fetch_add(1, Ordering::AcqRel);
        lock(&self.controller)?.pause()?;
        pause_managed_machine(self);
        let _ = self.refresh_runtime_status();
        self.status_value()
    }

    pub fn restore(&self) -> Result<Value, String> {
        self.live_apply_generation.fetch_add(1, Ordering::AcqRel);
        lock(&self.controller)?.restore()?;
        pause_managed_machine(self);
        let _ = self.refresh_runtime_status();
        self.status_value()
    }

    pub fn shutdown(&self) -> Value {
        self.quitting.store(true, Ordering::SeqCst);
        json!({ "shutdown": true, "keptTarget": true })
    }
}

pub fn allows_managed_takeover(configured: bool, action: ManagedAction) -> bool {
    configured && matches!(action, ManagedAction::Attach | ManagedAction::Takeover)
}

pub(crate) fn pause_managed_machine(state: &WorkerState) {
    let _ = lock(&state.managed_machine).map(|mut machine| machine.pause());
}

pub(crate) fn rearm_managed_machine(state: &WorkerState) -> Result<(), String> {
    let mut controller = lock(&state.controller)?;
    let mut machine = lock(&state.managed_machine)?;
    let probe = controller
        .probe_target()
        .unwrap_or_else(|_| TargetProbe::empty());
    machine.resume_and_rearm(&probe.processes, probe.engine_connected);
    let _ = controller.take_rearm_request();
    Ok(())
}

enum ManagedTickOutcome {
    Idle,
    StatusChanged,
    Attach,
    Takeover,
    ReleaseStale,
}

fn prepare_managed_tick(
    controller: &mut NotionController,
    machine: &mut ManagedLaunchMachine,
    state: &WorkerState,
) -> Result<TargetProbe, String> {
    if controller.take_rearm_request() {
        let probe = controller.probe_target()?;
        machine.resume_and_rearm(&probe.processes, probe.engine_connected);
    }
    if controller.managed_paused() {
        machine.pause();
    }
    if machine.payload_failed() && state.is_configured() {
        machine.retry_after_payload_ready();
    }
    let probe = controller.probe_target()?;
    machine.ensure_armed(&probe.processes, probe.engine_connected);
    Ok(probe)
}

fn classify_managed_tick(state: &WorkerState) -> Result<ManagedTickOutcome, String> {
    if !state.is_configured() {
        let mut controller = lock(&state.controller)?;
        return Ok(
            if controller.set_watch_status("idle", UNCONFIGURED_MESSAGE) {
                ManagedTickOutcome::StatusChanged
            } else {
                ManagedTickOutcome::Idle
            },
        );
    }
    let mut controller = lock(&state.controller)?;
    let mut machine = lock(&state.managed_machine)?;
    let probe = match prepare_managed_tick(&mut controller, &mut machine, state) {
        Ok(probe) => probe,
        Err(error) => {
            return Ok(if controller.set_watch_status("error", &error) {
                ManagedTickOutcome::StatusChanged
            } else {
                ManagedTickOutcome::Idle
            });
        }
    };
    let action = machine.decide(&probe.observation());
    match action {
        ManagedAction::Wait | ManagedAction::HoldExisting | ManagedAction::WaitForDebug => {
            let (phase, message) = machine.watch_status();
            Ok(if controller.set_watch_status(&phase, &message) {
                ManagedTickOutcome::StatusChanged
            } else {
                ManagedTickOutcome::Idle
            })
        }
        ManagedAction::Attach => Ok(ManagedTickOutcome::Attach),
        ManagedAction::Takeover => Ok(ManagedTickOutcome::Takeover),
        ManagedAction::ReleaseStale => Ok(ManagedTickOutcome::ReleaseStale),
        ManagedAction::Stay => {
            if controller.status().phase == "error" {
                let (phase, message) = machine.watch_status();
                if phase != "error" {
                    let _ = controller.set_watch_status(&phase, &message);
                    return Ok(ManagedTickOutcome::StatusChanged);
                }
            }
            Ok(ManagedTickOutcome::Idle)
        }
    }
}

fn apply_managed_action(state: &WorkerState, payload: ActivePayload) -> Result<(), String> {
    let mut controller = lock(&state.controller)?;
    let mut machine = lock(&state.managed_machine)?;
    let probe = match prepare_managed_tick(&mut controller, &mut machine, state) {
        Ok(probe) => probe,
        Err(error) => {
            let _ = controller.set_watch_status("error", &error);
            return Err(error);
        }
    };
    if controller.managed_paused() || !state.is_configured() {
        return Ok(());
    }
    if probe.engine_connected {
        machine.mark_active(&probe.processes);
        return Ok(());
    }
    match machine.decide(&probe.observation()) {
        ManagedAction::Attach => match controller.try_attach_current(payload) {
            Ok(true) => {
                let probe = controller
                    .probe_target()
                    .unwrap_or_else(|_| TargetProbe::empty());
                machine.mark_active(&probe.processes);
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(error) => {
                let probe = controller
                    .probe_target()
                    .unwrap_or_else(|_| TargetProbe::empty());
                machine.fail_takeover(&probe.processes);
                Err(error)
            }
        },
        ManagedAction::Takeover => {
            let applied = controller.takeover_unmanaged(payload);
            let probe = controller
                .probe_target()
                .unwrap_or_else(|_| TargetProbe::empty());
            if applied.is_ok() && controller.engine_is_live() {
                machine.mark_active(&probe.processes);
            } else {
                machine.fail_takeover(&probe.processes);
            }
            applied.map(|_| ())
        }
        ManagedAction::ReleaseStale => controller.release_stale_session(),
        ManagedAction::Wait | ManagedAction::HoldExisting | ManagedAction::WaitForDebug => {
            let (phase, message) = machine.watch_status();
            let _ = controller.set_watch_status(&phase, &message);
            Ok(())
        }
        ManagedAction::Stay => Ok(()),
    }
}

fn mark_payload_generation_failed(state: &WorkerState, error: &str) {
    let Ok(mut controller) = lock(&state.controller) else {
        return;
    };
    let Ok(mut machine) = lock(&state.managed_machine) else {
        return;
    };
    let processes = controller
        .probe_target()
        .map(|probe| probe.processes)
        .unwrap_or_default();
    machine.mark_payload_failed(&processes, error);
    let (phase, message) = machine.watch_status();
    let _ = controller.set_watch_status(&phase, &message);
}

async fn run_managed_launch_worker(state: Arc<WorkerState>) {
    if let Ok(mut controller) = lock(&state.controller) {
        controller.enable_managed_mode();
        if !state.is_configured() {
            let _ = controller.set_watch_status("idle", UNCONFIGURED_MESSAGE);
        }
    }
    let _ = state.refresh_runtime_status();

    loop {
        if state.quitting.load(Ordering::SeqCst) {
            break;
        }
        let worker = Arc::clone(&state);
        let outcome =
            match tokio::task::spawn_blocking(move || classify_managed_tick(&worker)).await {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(error)) => {
                    eprintln!("托管探测失败：{error}");
                    ManagedTickOutcome::Idle
                }
                Err(error) => {
                    eprintln!("托管探测任务失败：{error}");
                    ManagedTickOutcome::Idle
                }
            };

        match outcome {
            ManagedTickOutcome::Attach | ManagedTickOutcome::Takeover => {
                let worker = Arc::clone(&state);
                let payload = tokio::task::spawn_blocking(move || worker.active_payload()).await;
                match payload {
                    Ok(Ok(payload)) => {
                        let worker = Arc::clone(&state);
                        if let Err(error) = tokio::task::spawn_blocking(move || {
                            apply_managed_action(&worker, payload)
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string()))
                        {
                            eprintln!("托管接管失败：{error}");
                        }
                    }
                    Ok(Err(error)) => {
                        let worker = Arc::clone(&state);
                        let message = error;
                        let _ = tokio::task::spawn_blocking(move || {
                            mark_payload_generation_failed(&worker, &message);
                        })
                        .await;
                    }
                    Err(error) => eprintln!("托管任务失败：{error}"),
                }
            }
            ManagedTickOutcome::ReleaseStale => {
                let worker = Arc::clone(&state);
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    let mut controller = lock(&worker.controller)?;
                    controller.release_stale_session()?;
                    if let Ok(machine) = lock(&worker.managed_machine) {
                        let (phase, message) = machine.watch_status();
                        let _ = controller.set_watch_status(&phase, &message);
                    }
                    Ok::<(), String>(())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                {
                    eprintln!("清理失效会话失败：{error}");
                }
            }
            _ => {}
        }

        if !matches!(outcome, ManagedTickOutcome::Idle) {
            let _ = state.refresh_runtime_status();
        }

        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
}

pub async fn run() {
    let state = Arc::new(WorkerState::load().expect("加载 Notion worker 状态失败"));
    if let Ok(mut controller) = lock(&state.controller) {
        controller.enable_managed_mode();
        let _ = controller.set_watch_status("idle", UNCONFIGURED_MESSAGE);
    }
    let _ = state.refresh_runtime_status();

    let ipc_state = Arc::clone(&state);
    let watcher_state = Arc::clone(&state);
    let ipc = tokio::spawn(async move {
        if let Err(error) = plugin_ipc::serve(Arc::clone(&ipc_state)).await {
            eprintln!("Background Studio 插件 IPC 失败：{error}");
            ipc_state.quitting.store(true, Ordering::SeqCst);
        }
    });
    let watcher = tokio::spawn(async move {
        run_managed_launch_worker(watcher_state).await;
    });

    while !state.quitting.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    ipc.abort();
    watcher.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DisplaySettings, MediaKind};
    use sha2::{Digest, Sha256};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    use uuid::Uuid;

    fn test_state() -> WorkerState {
        let root = std::env::temp_dir().join(format!("notion-worker-{}", Uuid::new_v4()));
        WorkerState::load_from(root).expect("load worker")
    }

    #[test]
    fn unconfigured_status_does_not_claim_ready() {
        let state = test_state();
        let status = state.status_value().unwrap();
        assert_eq!(status["configured"], false);
        assert_eq!(status["revision"], Value::Null);
        assert_eq!(status["message"], UNCONFIGURED_MESSAGE);
        assert!(state.apply().unwrap_err().contains("尚未配置背景"));
    }

    #[test]
    fn unconfigured_watcher_never_takeovers() {
        assert!(!allows_managed_takeover(false, ManagedAction::Takeover));
        assert!(!allows_managed_takeover(false, ManagedAction::Attach));
        assert!(allows_managed_takeover(true, ManagedAction::Takeover));
        let state = test_state();
        let outcome = classify_managed_tick(&state).unwrap();
        assert!(matches!(
            outcome,
            ManagedTickOutcome::Idle | ManagedTickOutcome::StatusChanged
        ));
        assert_eq!(
            state.status_value().unwrap()["message"],
            UNCONFIGURED_MESSAGE
        );
    }

    #[test]
    fn pause_and_shutdown_keep_target_semantics() {
        let state = test_state();
        lock(&state.controller).unwrap().enable_managed_mode();
        let paused = state.pause().unwrap();
        assert_eq!(paused["paused"], true);
        assert_eq!(paused["phase"], "paused");
        let shutdown = state.shutdown();
        assert_eq!(shutdown["shutdown"], true);
        assert_eq!(shutdown["keptTarget"], true);
        assert!(state.quitting.load(Ordering::SeqCst));
    }

    fn serve_png(body: &[u8]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = body.to_vec();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        });
        port
    }

    #[tokio::test]
    async fn configure_stores_revision_and_enables_hot_update_path() {
        let body = b"worker-png";
        let digest = format!("{:x}", Sha256::digest(body));
        let port = serve_png(body);
        let state = test_state();
        let params = json!({
            "schemaVersion": 1,
            "revision": "rev-hot",
            "media": {
                "url": format!("http://127.0.0.1:{port}/media/1"),
                "kind": "image",
                "mimeType": "image/png",
                "sha256": digest,
                "byteSize": body.len()
            },
            "display": DisplaySettings::default()
        });
        let status = state.configure(Some(&params)).await.unwrap();
        assert_eq!(status["configured"], true);
        assert_eq!(status["revision"], "rev-hot");
        let payload = state.active_payload().unwrap();
        assert_eq!(payload.media_bytes.as_ref(), body);
        assert_eq!(
            build_active_payload_from_bytes(
                body.to_vec(),
                "image/png",
                &MediaKind::Image,
                &DisplaySettings::default()
            )
            .unwrap()
            .revision,
            payload.revision
        );
    }
}
