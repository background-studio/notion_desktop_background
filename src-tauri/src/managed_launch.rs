use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessKey {
    pub pid: u32,
    pub created_at: u64,
}

#[derive(Clone, Debug)]
pub struct ProcessRecord {
    pub key: ProcessKey,
    pub command_line: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedPhase {
    Waiting,
    ExistingUnmanaged,
    AttachPending,
    Takeover,
    Active,
    Suspended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedAction {
    Wait,
    HoldExisting,
    WaitForDebug,
    Attach,
    Takeover,
    ReleaseStale,
    Stay,
}

#[derive(Clone, Debug, Default)]
pub struct ManagedObservation {
    pub processes: Vec<ProcessKey>,
    pub has_live_debug_session: bool,
    pub has_debug_launch: bool,
    pub engine_connected: bool,
    pub now_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub struct ManagedLaunchMachine {
    phase: ManagedPhase,
    paused: bool,
    initialized: bool,
    baseline: HashSet<ProcessKey>,
    known: HashSet<ProcessKey>,
    confirm_ticks: u8,
    debug_wait_started_ms: Option<u64>,
    debug_wait_failed: bool,
    payload_error: Option<String>,
}

impl Default for ManagedLaunchMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl ManagedLaunchMachine {
    pub const TAKEOVER_CONFIRM_TICKS: u8 = 2;
    pub const WAIT_FOR_DEBUG_LIMIT_MS: u64 = 45_000;

    pub fn new() -> Self {
        Self {
            phase: ManagedPhase::Waiting,
            paused: false,
            initialized: false,
            baseline: HashSet::new(),
            known: HashSet::new(),
            confirm_ticks: 0,
            debug_wait_started_ms: None,
            debug_wait_failed: false,
            payload_error: None,
        }
    }

    #[allow(dead_code)]
    pub fn phase(&self) -> ManagedPhase {
        self.phase
    }

    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn status_copy(&self) -> (&'static str, &'static str) {
        match self.phase {
            ManagedPhase::Waiting => ("idle", "已启用，等待 Notion 启动"),
            ManagedPhase::ExistingUnmanaged => ("idle", "Notion 已在运行，点立即接管可重启"),
            ManagedPhase::AttachPending | ManagedPhase::Takeover => ("starting", "正在接管 Notion"),
            ManagedPhase::Active => ("active", "背景已自动应用"),
            ManagedPhase::Suspended => ("paused", "暂停托管"),
        }
    }

    pub fn watch_status(&self) -> (String, String) {
        if let Some(error) = &self.payload_error {
            return ("error".to_string(), error.clone());
        }
        if self.debug_wait_failed {
            return (
                "error".to_string(),
                "调试端口未能在 45 秒内就绪，等待 Notion 退出后重新接管".to_string(),
            );
        }
        let (phase, message) = self.status_copy();
        (phase.to_string(), message.to_string())
    }

    pub fn payload_failed(&self) -> bool {
        self.payload_error.is_some()
    }

    pub fn arm(&mut self, preexisting: &[ProcessKey]) {
        self.initialized = true;
        self.paused = false;
        self.confirm_ticks = 0;
        self.debug_wait_started_ms = None;
        self.debug_wait_failed = false;
        self.payload_error = None;
        self.baseline = preexisting.iter().copied().collect();
        self.known = self.baseline.clone();
        self.phase = if self.baseline.is_empty() {
            ManagedPhase::Waiting
        } else {
            ManagedPhase::ExistingUnmanaged
        };
    }

    pub fn ensure_armed(&mut self, preexisting: &[ProcessKey], engine_connected: bool) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if self.paused {
            return;
        }
        if engine_connected {
            self.resume_and_rearm(preexisting, true);
        } else {
            self.arm(preexisting);
        }
    }

    pub fn pause(&mut self) {
        self.paused = true;
        self.phase = ManagedPhase::Suspended;
    }

    pub fn resume_and_rearm(&mut self, current: &[ProcessKey], engine_connected: bool) {
        self.initialized = true;
        self.paused = false;
        self.confirm_ticks = 0;
        self.debug_wait_started_ms = None;
        self.debug_wait_failed = false;
        self.payload_error = None;
        self.baseline.clear();
        self.known = current.iter().copied().collect();
        self.phase = if engine_connected {
            ManagedPhase::Active
        } else if current.is_empty() {
            ManagedPhase::Waiting
        } else {
            ManagedPhase::AttachPending
        };
    }

    pub fn mark_active(&mut self, current: &[ProcessKey]) {
        self.initialized = true;
        self.paused = false;
        self.confirm_ticks = 0;
        self.debug_wait_started_ms = None;
        self.debug_wait_failed = false;
        self.payload_error = None;
        self.baseline.clear();
        self.known = current.iter().copied().collect();
        self.phase = ManagedPhase::Active;
    }

    pub fn fail_takeover(&mut self, current: &[ProcessKey]) {
        self.confirm_ticks = 0;
        self.debug_wait_started_ms = None;
        if current.is_empty() {
            self.baseline.clear();
            self.known.clear();
            self.debug_wait_failed = false;
            self.payload_error = None;
            self.phase = if self.paused {
                ManagedPhase::Suspended
            } else {
                ManagedPhase::Waiting
            };
            return;
        }
        self.baseline = current.iter().copied().collect();
        self.known = self.baseline.clone();
        self.phase = if self.paused {
            ManagedPhase::Suspended
        } else {
            ManagedPhase::ExistingUnmanaged
        };
    }

    pub fn mark_payload_failed(&mut self, current: &[ProcessKey], message: impl Into<String>) {
        self.fail_takeover(current);
        self.payload_error = Some(message.into());
        self.debug_wait_failed = false;
    }

    pub fn retry_after_payload_ready(&mut self) {
        if self.payload_error.is_none() {
            return;
        }
        self.payload_error = None;
        self.confirm_ticks = 0;
        self.baseline.clear();
        if !self.paused && self.phase == ManagedPhase::ExistingUnmanaged {
            self.phase = ManagedPhase::Waiting;
        }
    }

    pub fn decide(&mut self, observation: &ManagedObservation) -> ManagedAction {
        if self.paused {
            self.phase = ManagedPhase::Suspended;
            return ManagedAction::Stay;
        }

        if observation.processes.is_empty() {
            let should_release = matches!(
                self.phase,
                ManagedPhase::Active | ManagedPhase::AttachPending | ManagedPhase::Takeover
            ) || observation.engine_connected
                || self.debug_wait_failed;
            self.confirm_ticks = 0;
            self.known.clear();
            self.baseline.clear();
            self.debug_wait_started_ms = None;
            self.debug_wait_failed = false;
            self.payload_error = None;
            self.phase = ManagedPhase::Waiting;
            return if should_release {
                ManagedAction::ReleaseStale
            } else {
                ManagedAction::Wait
            };
        }

        if observation.engine_connected {
            self.absorb(&observation.processes);
            self.confirm_ticks = 0;
            self.debug_wait_started_ms = None;
            self.debug_wait_failed = false;
            self.payload_error = None;
            self.phase = ManagedPhase::Active;
            return ManagedAction::Stay;
        }

        if observation.has_live_debug_session {
            self.absorb(&observation.processes);
            self.confirm_ticks = 0;
            self.debug_wait_started_ms = None;
            self.debug_wait_failed = false;
            self.phase = ManagedPhase::AttachPending;
            return ManagedAction::Attach;
        }

        if self.phase == ManagedPhase::Active {
            self.fail_takeover(&observation.processes);
            return ManagedAction::ReleaseStale;
        }

        if observation.has_debug_launch && !self.debug_wait_failed {
            self.absorb(&observation.processes);
            self.confirm_ticks = 0;
            let started = *self.debug_wait_started_ms.get_or_insert(observation.now_ms);
            if observation.now_ms.saturating_sub(started) >= Self::WAIT_FOR_DEBUG_LIMIT_MS {
                self.debug_wait_failed = true;
                self.fail_takeover(&observation.processes);
                return ManagedAction::HoldExisting;
            }
            self.phase = ManagedPhase::AttachPending;
            return ManagedAction::WaitForDebug;
        }

        self.debug_wait_started_ms = None;

        let overlaps_baseline = overlaps(&self.baseline, &observation.processes);
        let overlaps_known = overlaps(&self.known, &observation.processes);

        match self.phase {
            ManagedPhase::Waiting => {
                if overlaps_baseline {
                    self.absorb(&observation.processes);
                    self.confirm_ticks = 0;
                    self.phase = ManagedPhase::ExistingUnmanaged;
                    return ManagedAction::HoldExisting;
                }
                self.confirm_ticks = self.confirm_ticks.saturating_add(1);
                if self.confirm_ticks >= Self::TAKEOVER_CONFIRM_TICKS {
                    self.phase = ManagedPhase::Takeover;
                    self.known = observation.processes.iter().copied().collect();
                    self.baseline.clear();
                    self.confirm_ticks = 0;
                    return ManagedAction::Takeover;
                }
                ManagedAction::Wait
            }
            ManagedPhase::ExistingUnmanaged => {
                if overlaps_baseline || overlaps_known {
                    self.absorb(&observation.processes);
                    self.confirm_ticks = 0;
                    return ManagedAction::HoldExisting;
                }
                self.confirm_ticks = self.confirm_ticks.saturating_add(1);
                if self.confirm_ticks >= Self::TAKEOVER_CONFIRM_TICKS {
                    self.phase = ManagedPhase::Takeover;
                    self.known = observation.processes.iter().copied().collect();
                    self.baseline.clear();
                    self.confirm_ticks = 0;
                    return ManagedAction::Takeover;
                }
                ManagedAction::HoldExisting
            }
            ManagedPhase::AttachPending => {
                if overlaps_known {
                    self.absorb(&observation.processes);
                    self.confirm_ticks = 0;
                    return ManagedAction::Stay;
                }
                self.begin_fresh_confirmation(&observation.processes);
                ManagedAction::Wait
            }
            ManagedPhase::Takeover => {
                if overlaps_known {
                    self.absorb(&observation.processes);
                    self.confirm_ticks = 0;
                    return ManagedAction::Takeover;
                }
                self.begin_fresh_confirmation(&observation.processes);
                ManagedAction::Wait
            }
            ManagedPhase::Active => {
                if overlaps_known {
                    self.absorb(&observation.processes);
                    self.confirm_ticks = 0;
                    return ManagedAction::Stay;
                }
                self.confirm_ticks = self.confirm_ticks.saturating_add(1);
                if self.confirm_ticks >= Self::TAKEOVER_CONFIRM_TICKS {
                    self.phase = ManagedPhase::Takeover;
                    self.known = observation.processes.iter().copied().collect();
                    self.confirm_ticks = 0;
                    return ManagedAction::Takeover;
                }
                ManagedAction::Stay
            }
            ManagedPhase::Suspended => ManagedAction::Stay,
        }
    }

    fn absorb(&mut self, processes: &[ProcessKey]) {
        self.known.extend(processes.iter().copied());
    }

    fn begin_fresh_confirmation(&mut self, processes: &[ProcessKey]) {
        self.phase = ManagedPhase::Waiting;
        self.baseline.clear();
        self.known.clear();
        self.confirm_ticks = 1;
        self.absorb(processes);
    }
}

fn overlaps(set: &HashSet<ProcessKey>, processes: &[ProcessKey]) -> bool {
    processes.iter().any(|process| set.contains(process))
}

pub fn normalize_executable_path(path: &str) -> String {
    let mut value = path.replace('/', "\\");
    if let Some(stripped) = value
        .strip_prefix(r"\\?\")
        .or_else(|| value.strip_prefix(r"\\?\"))
    {
        value = stripped.to_string();
    }
    value.to_ascii_lowercase()
}

pub fn remote_debugging_ports(command_line: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    let marker = "--remote-debugging-port=";
    let mut rest = command_line;
    while let Some(index) = rest.find(marker) {
        let after = &rest[index + marker.len()..];
        let digits: String = after
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if let Ok(port) = digits.parse::<u16>() {
            if port != 0 {
                ports.push(port);
            }
        }
        rest = after;
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

pub fn has_remote_debugging_arg(command_line: &str) -> bool {
    !remote_debugging_ports(command_line).is_empty()
}

pub fn debug_ports_from_records(records: &[ProcessRecord]) -> Vec<u16> {
    let mut ports = Vec::new();
    for record in records {
        if let Some(command_line) = &record.command_line {
            ports.extend(remote_debugging_ports(command_line));
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

#[cfg(windows)]
pub fn snapshot_matching_executable(executable: &str) -> Result<Vec<ProcessRecord>, String> {
    snapshot_matching_executable_windows(executable)
}

#[cfg(not(windows))]
pub fn snapshot_matching_executable(_executable: &str) -> Result<Vec<ProcessRecord>, String> {
    Err("进程快照仅支持 Windows。".to_string())
}

#[cfg(windows)]
fn snapshot_matching_executable_windows(executable: &str) -> Result<Vec<ProcessRecord>, String> {
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct SafeHandle(HANDLE);

    impl SafeHandle {
        fn new(handle: HANDLE) -> Option<Self> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                None
            } else {
                Some(Self(handle))
            }
        }
    }

    impl Drop for SafeHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut core::ffi::c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    fn query_command_line(handle: HANDLE) -> Option<String> {
        const PROCESS_COMMAND_LINE_INFORMATION: u32 = 60;
        unsafe {
            let mut buffer = vec![0u8; 4096];
            let mut return_length = 0u32;
            let mut status = NtQueryInformationProcess(
                handle,
                PROCESS_COMMAND_LINE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut return_length,
            );
            if status != 0 {
                if return_length as usize > buffer.len() && return_length < 64 * 1024 {
                    buffer.resize(return_length as usize, 0);
                    status = NtQueryInformationProcess(
                        handle,
                        PROCESS_COMMAND_LINE_INFORMATION,
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        &mut return_length,
                    );
                }
                if status != 0 {
                    return None;
                }
            }
            if buffer.len() < size_of::<UnicodeString>() {
                return None;
            }
            let unicode = &*(buffer.as_ptr() as *const UnicodeString);
            if unicode.buffer.is_null() || unicode.length == 0 {
                return None;
            }
            let chars = usize::from(unicode.length) / 2;
            let start = buffer.as_ptr() as usize;
            let end = start + buffer.len();
            let pointer = unicode.buffer as usize;
            if pointer < start || pointer.saturating_add(chars.saturating_mul(2)) > end {
                return None;
            }
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                unicode.buffer,
                chars,
            )))
        }
    }

    fn query_record(pid: u32, target: &str) -> Option<ProcessRecord> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let handle = SafeHandle::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))?;
            let mut image = [0u16; 1024];
            let mut size = image.len() as u32;
            if QueryFullProcessImageNameW(handle.0, 0, image.as_mut_ptr(), &mut size) == 0 {
                return None;
            }
            let path = String::from_utf16_lossy(&image[..size as usize]);
            if normalize_executable_path(&path) != target {
                return None;
            }
            let mut creation = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let mut exit_time = creation;
            let mut kernel = creation;
            let mut user = creation;
            if GetProcessTimes(
                handle.0,
                &mut creation,
                &mut exit_time,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            let created_at =
                (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            Some(ProcessRecord {
                key: ProcessKey { pid, created_at },
                command_line: query_command_line(handle.0),
            })
        }
    }

    let target = normalize_executable_path(executable);
    let file_name = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Notion.exe")
        .to_ascii_lowercase();

    unsafe {
        let snapshot = SafeHandle::new(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0))
            .ok_or_else(|| "无法创建进程快照。".to_string())?;
        let mut entry: PROCESSENTRY32W = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot.0, &mut entry) == 0 {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        loop {
            let name = {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                String::from_utf16_lossy(&entry.szExeFile[..end]).to_ascii_lowercase()
            };
            if name == file_name {
                if let Some(record) = query_record(entry.th32ProcessID, &target) {
                    records.push(record);
                }
            }
            if Process32NextW(snapshot.0, &mut entry) == 0 {
                break;
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(pid: u32, created_at: u64) -> ProcessKey {
        ProcessKey { pid, created_at }
    }

    fn observation(processes: &[ProcessKey]) -> ManagedObservation {
        ManagedObservation {
            processes: processes.to_vec(),
            ..ManagedObservation::default()
        }
    }

    #[test]
    fn waits_when_no_processes() {
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(machine.decide(&observation(&[])), ManagedAction::Wait);
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_eq!(machine.status_copy(), ("idle", "已启用，等待 Notion 启动"));
    }

    #[test]
    fn does_not_kill_preexisting_unmanaged() {
        let old = key(11, 100);
        let child = key(12, 110);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[old]);
        assert_eq!(
            machine.decide(&observation(&[old])),
            ManagedAction::HoldExisting
        );
        assert_eq!(
            machine.decide(&observation(&[old, child])),
            ManagedAction::HoldExisting
        );
        assert_eq!(machine.phase(), ManagedPhase::ExistingUnmanaged);
        assert_eq!(
            machine.status_copy(),
            ("idle", "Notion 已在运行，点立即接管可重启")
        );
    }

    #[test]
    fn takeovers_after_preexisting_exits_and_new_starts() {
        let old = key(21, 200);
        let next = key(31, 300);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[old]);
        assert_eq!(machine.decide(&observation(&[])), ManagedAction::Wait);
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_eq!(machine.decide(&observation(&[next])), ManagedAction::Wait);
        assert_eq!(
            machine.decide(&observation(&[next])),
            ManagedAction::Takeover
        );
        assert_eq!(machine.phase(), ManagedPhase::Takeover);
    }

    #[test]
    fn does_not_kill_debug_launch_or_live_session() {
        let process = key(41, 400);
        let mut debug_launch = ManagedLaunchMachine::new();
        debug_launch.arm(&[]);
        let mut launching = observation(&[process]);
        launching.has_debug_launch = true;
        assert_eq!(debug_launch.decide(&launching), ManagedAction::WaitForDebug);
        assert_ne!(debug_launch.decide(&launching), ManagedAction::Takeover);

        let mut live = ManagedLaunchMachine::new();
        live.arm(&[process]);
        let mut session = observation(&[process]);
        session.has_live_debug_session = true;
        assert_eq!(live.decide(&session), ManagedAction::Attach);
        assert_ne!(live.decide(&session), ManagedAction::Takeover);
    }

    #[test]
    fn rearms_after_target_exit() {
        let first = key(51, 500);
        let second = key(61, 600);
        let mut machine = ManagedLaunchMachine::new();
        machine.mark_active(&[first]);
        assert_eq!(
            machine.decide(&observation(&[])),
            ManagedAction::ReleaseStale
        );
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_eq!(machine.decide(&observation(&[second])), ManagedAction::Wait);
        assert_eq!(
            machine.decide(&observation(&[second])),
            ManagedAction::Takeover
        );
    }

    #[test]
    fn paused_does_not_takeover() {
        let process = key(71, 700);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        machine.pause();
        let mut fresh = observation(&[process]);
        fresh.has_debug_launch = false;
        assert_eq!(machine.decide(&fresh), ManagedAction::Stay);
        assert_eq!(machine.phase(), ManagedPhase::Suspended);
        assert!(machine.is_paused());
        assert_eq!(machine.status_copy(), ("paused", "暂停托管"));
    }

    #[test]
    fn electron_child_processes_do_not_retrigger_takeover() {
        let main = key(81, 800);
        let gpu = key(82, 810);
        let renderer = key(83, 820);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(machine.decide(&observation(&[main])), ManagedAction::Wait);
        assert_eq!(
            machine.decide(&observation(&[main])),
            ManagedAction::Takeover
        );
        assert_eq!(
            machine.decide(&observation(&[main, gpu])),
            ManagedAction::Takeover
        );
        machine.mark_active(&[main, gpu]);
        let mut active = observation(&[main, gpu, renderer]);
        active.engine_connected = true;
        assert_eq!(machine.decide(&active), ManagedAction::Stay);
        assert_eq!(machine.phase(), ManagedPhase::Active);
    }

    #[test]
    fn wait_for_debug_times_out_without_killing() {
        let process = key(91, 900);
        let next = key(92, 980);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        let mut launching = observation(&[process]);
        launching.has_debug_launch = true;
        launching.now_ms = 1_000;
        assert_eq!(machine.decide(&launching), ManagedAction::WaitForDebug);
        launching.now_ms = 1_000 + ManagedLaunchMachine::WAIT_FOR_DEBUG_LIMIT_MS - 1;
        assert_eq!(machine.decide(&launching), ManagedAction::WaitForDebug);
        launching.now_ms = 1_000 + ManagedLaunchMachine::WAIT_FOR_DEBUG_LIMIT_MS;
        assert_eq!(machine.decide(&launching), ManagedAction::HoldExisting);
        assert_ne!(machine.decide(&launching), ManagedAction::Takeover);
        assert_eq!(machine.phase(), ManagedPhase::ExistingUnmanaged);
        assert_eq!(
            machine.watch_status(),
            (
                "error".to_string(),
                "调试端口未能在 45 秒内就绪，等待 Notion 退出后重新接管".to_string()
            )
        );
        assert_eq!(
            machine.decide(&observation(&[])),
            ManagedAction::ReleaseStale
        );
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_eq!(machine.decide(&observation(&[next])), ManagedAction::Wait);
        assert_eq!(
            machine.decide(&observation(&[next])),
            ManagedAction::Takeover
        );
    }

    #[test]
    fn active_with_dead_engine_blocks_for_manual_takeover() {
        let process = key(101, 1000);
        let child = key(102, 1010);
        let mut machine = ManagedLaunchMachine::new();
        machine.mark_active(&[process]);
        let mut dead = observation(&[process, child]);
        dead.engine_connected = false;
        dead.has_live_debug_session = false;
        dead.has_debug_launch = true;
        assert_eq!(machine.decide(&dead), ManagedAction::ReleaseStale);
        assert_eq!(machine.phase(), ManagedPhase::ExistingUnmanaged);
        assert_eq!(
            machine.decide(&observation(&[process, child])),
            ManagedAction::HoldExisting
        );
        assert_eq!(
            machine.status_copy(),
            ("idle", "Notion 已在运行，点立即接管可重启")
        );
    }

    #[test]
    fn payload_failure_blocks_until_retry() {
        let process = key(111, 1100);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Wait
        );
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Takeover
        );
        machine.mark_payload_failed(&[process], "请先从媒体库选择一张图片或一个视频。");
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::HoldExisting
        );
        assert_eq!(
            machine.watch_status().1,
            "请先从媒体库选择一张图片或一个视频。"
        );
        machine.retry_after_payload_ready();
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Wait
        );
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Takeover
        );
    }

    fn classify_then_revalidate(
        machine: &mut ManagedLaunchMachine,
        processes: &[ProcessKey],
    ) -> (ManagedAction, ManagedAction) {
        let classified = machine.decide(&observation(processes));
        let revalidated = machine.decide(&observation(processes));
        (classified, revalidated)
    }

    #[test]
    fn revalidate_after_classify_still_takeovers_same_generation() {
        let main = key(121, 1200);
        let child = key(122, 1210);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(machine.decide(&observation(&[main])), ManagedAction::Wait);
        let (classified, revalidated) = classify_then_revalidate(&mut machine, &[main]);
        assert_eq!(classified, ManagedAction::Takeover);
        assert_eq!(revalidated, ManagedAction::Takeover);
        assert_eq!(machine.phase(), ManagedPhase::Takeover);
        assert_eq!(
            machine.decide(&observation(&[main, child])),
            ManagedAction::Takeover
        );
    }

    #[test]
    fn revalidate_does_not_restart_after_target_exits() {
        let process = key(131, 1300);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Wait
        );
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Takeover
        );
        assert_eq!(
            machine.decide(&observation(&[])),
            ManagedAction::ReleaseStale
        );
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_ne!(machine.decide(&observation(&[])), ManagedAction::Takeover);
    }

    #[test]
    fn generation_change_during_encode_reconfirms() {
        let first = key(141, 1400);
        let replacement = key(142, 1410);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(machine.decide(&observation(&[first])), ManagedAction::Wait);
        assert_eq!(
            machine.decide(&observation(&[first])),
            ManagedAction::Takeover
        );
        assert_eq!(
            machine.decide(&observation(&[replacement])),
            ManagedAction::Wait
        );
        assert_eq!(machine.phase(), ManagedPhase::Waiting);
        assert_eq!(
            machine.decide(&observation(&[replacement])),
            ManagedAction::Takeover
        );
    }

    #[test]
    fn takeover_phase_recovers_after_temporary_probe_error() {
        let process = key(151, 1500);
        let mut machine = ManagedLaunchMachine::new();
        machine.arm(&[]);
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Wait
        );
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Takeover
        );
        assert_eq!(machine.phase(), ManagedPhase::Takeover);
        assert_eq!(machine.status_copy().0, "starting");
        assert_eq!(
            machine.decide(&observation(&[process])),
            ManagedAction::Takeover
        );
        assert_eq!(machine.phase(), ManagedPhase::Takeover);
    }

    #[test]
    fn parses_remote_debugging_ports() {
        assert_eq!(
            remote_debugging_ports(
                r#""C:\Notion.exe" --remote-debugging-address=127.0.0.1 --remote-debugging-port=9226"#
            ),
            vec![9226]
        );
        assert!(has_remote_debugging_arg(
            r#"Notion.exe "--remote-debugging-port=9333""#
        ));
        assert!(!has_remote_debugging_arg(r#""C:\Notion.exe""#));
    }

    #[test]
    fn normalizes_extended_windows_paths() {
        assert_eq!(
            normalize_executable_path(r"\\?\C:\Users\Me\AppData\Local\Programs\Notion\Notion.exe"),
            r"c:\users\me\appdata\local\programs\notion\notion.exe"
        );
    }
}
