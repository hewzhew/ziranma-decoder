#[cfg(not(windows))]
fn main() {
    eprintln!("recorderctl is available only on Windows");
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    windows_controller::run()
}

#[cfg(windows)]
mod windows_controller {
    use std::ffi::OsStr;
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::{Component, Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
    use windows::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_HOTKEY};
    use windows::core::{PCWSTR, PWSTR};

    const RECORDER_EXE_NAME: &str = "codex-recorder.exe";
    const STATE_SCHEMA: &str = "ziranma-recorder-slots-v1";
    const BUILD_SCHEMA: &str = "ziranma-recorder-build-v1";
    const ACTIVE_SCHEMA: &str = "ziranma-recorder-active-v1";
    const STOP_HOTKEY_ID: usize = 0x5A59;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const STOP_TIMEOUT: Duration = Duration::from_secs(15);
    const START_SETTLE: Duration = Duration::from_millis(500);

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct SlotState {
        current: Option<String>,
        candidate: Option<String>,
        previous: Option<String>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct BuildMetadata {
        producer_version: String,
        capture_profile: String,
        control_state_schema: Option<String>,
        byte_len: u64,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct RecorderProcess {
        pid: u32,
        path: Option<PathBuf>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ActiveState {
        pid: u32,
        session: String,
        kind: String,
        producer_version: String,
        capture_profile: String,
        started_unix_ms: u64,
        phase: String,
        target: String,
        saved_segments: u64,
        saved_events: u64,
        last_flush_unix_ms: Option<u64>,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum ControllerCommand {
        Status {
            machine: bool,
        },
        Adopt(PathBuf),
        Stage(PathBuf),
        Promote,
        Rollback,
        Drain,
        Run {
            session_kind: String,
            background: bool,
        },
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: This wrapper owns the successful Win32 handle exactly once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let Some(command) = parse_command(std::env::args().skip(1))? else {
            return Ok(());
        };
        match command {
            ControllerCommand::Status { machine } => status(machine),
            ControllerCommand::Adopt(source) => adopt(&source),
            ControllerCommand::Stage(source) => stage(&source),
            ControllerCommand::Promote => promote(),
            ControllerCommand::Rollback => rollback(),
            ControllerCommand::Drain => drain(),
            ControllerCommand::Run {
                session_kind,
                background,
            } => run_current(&session_kind, background),
        }
    }

    fn parse_command(
        arguments: impl IntoIterator<Item = String>,
    ) -> Result<Option<ControllerCommand>, Box<dyn std::error::Error>> {
        let mut args = arguments.into_iter();
        let Some(command) = args.next() else {
            print_usage();
            return Err("a recorderctl command is required".into());
        };
        let parsed = match command.as_str() {
            "status" => {
                let mut machine = false;
                for argument in args {
                    match argument.as_str() {
                        "--machine" => machine = true,
                        _ => return Err("unknown status argument; value was suppressed".into()),
                    }
                }
                ControllerCommand::Status { machine }
            }
            "adopt" => {
                let source = required_path(&mut args, "adopt requires one recorder path")?;
                reject_extra(args)?;
                ControllerCommand::Adopt(source)
            }
            "stage" => {
                let source = required_path(&mut args, "stage requires one recorder path")?;
                reject_extra(args)?;
                ControllerCommand::Stage(source)
            }
            "promote" => {
                reject_extra(args)?;
                ControllerCommand::Promote
            }
            "rollback" => {
                reject_extra(args)?;
                ControllerCommand::Rollback
            }
            "drain" => {
                reject_extra(args)?;
                ControllerCommand::Drain
            }
            "run" => {
                let mut session_kind = "daily".to_owned();
                let mut background = false;
                while let Some(argument) = args.next() {
                    match argument.as_str() {
                        "--session-kind" => {
                            session_kind = args
                                .next()
                                .ok_or("--session-kind requires daily, course, or theme")?;
                            validate_session_kind(&session_kind)?;
                        }
                        "--background" => background = true,
                        _ => return Err("unknown run argument; value was suppressed".into()),
                    }
                }
                ControllerCommand::Run {
                    session_kind,
                    background,
                }
            }
            "--help" | "-h" | "help" => {
                reject_extra(args)?;
                print_usage();
                return Ok(None);
            }
            _ => return Err("unknown recorderctl command; value was suppressed".into()),
        };
        Ok(Some(parsed))
    }

    fn required_path(
        args: &mut impl Iterator<Item = String>,
        error: &'static str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        Ok(PathBuf::from(args.next().ok_or(error)?))
    }

    fn reject_extra(
        mut args: impl Iterator<Item = String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if args.next().is_some() {
            return Err("unexpected argument; value was suppressed".into());
        }
        Ok(())
    }

    fn validate_session_kind(value: &str) -> Result<(), Box<dyn std::error::Error>> {
        if matches!(value, "daily" | "course" | "theme") {
            Ok(())
        } else {
            Err("--session-kind requires daily, course, or theme".into())
        }
    }

    fn print_usage() {
        eprintln!("usage: recorderctl <status|adopt|stage|promote|rollback|drain|run>");
        eprintln!("  recorderctl status [--machine]");
        eprintln!("  recorderctl adopt <codex-recorder.exe>");
        eprintln!("  recorderctl stage <candidate.exe>");
        eprintln!("  recorderctl promote");
        eprintln!("  recorderctl rollback");
        eprintln!("  recorderctl drain");
        eprintln!("  recorderctl run [--session-kind daily|course|theme] [--background]");
    }

    fn status(machine: bool) -> Result<(), Box<dyn std::error::Error>> {
        let root = runtime_root();
        let state = read_state_if_present(&root)?;
        let active = read_active_if_present(&root)?;
        let processes = recorder_processes()?;
        let healthy = processes.len() <= 1
            && processes
                .iter()
                .all(|process| managed_process_role(process, state.as_ref(), &root).is_some());
        if !machine {
            return print_human_status(&root, state.as_ref(), active.as_ref(), &processes, healthy);
        }
        println!(
            "RECORDERCTL_STATUS schema={} configured={} running={} healthy={} \
             network=false reads_capture_data=false writes=false path_disclosed=false \
             contains_behavioral_metadata=true",
            STATE_SCHEMA,
            state.is_some(),
            processes.len(),
            healthy
        );
        if let Some(state) = &state {
            print_slot("current", state.current.as_deref(), &root)?;
            print_slot("candidate", state.candidate.as_deref(), &root)?;
            print_slot("previous", state.previous.as_deref(), &root)?;
        } else {
            for role in ["current", "candidate", "previous"] {
                println!("RECORDERCTL_SLOT role={role} state=unconfigured");
            }
        }
        print_machine_active(active.as_ref(), &processes)?;
        for process in processes {
            let role = managed_process_role(&process, state.as_ref(), &root).unwrap_or("unmanaged");
            println!("{}", machine_process_line(process.pid, role));
        }
        Ok(())
    }

    fn print_human_status(
        root: &Path,
        state: Option<&SlotState>,
        active: Option<&ActiveState>,
        processes: &[RecorderProcess],
        healthy: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let current = human_slot_version(root, state.and_then(|state| state.current.as_deref()))?;
        let candidate =
            human_slot_version(root, state.and_then(|state| state.candidate.as_deref()))?;
        let previous = human_slot_version(root, state.and_then(|state| state.previous.as_deref()))?;
        let headline = match (healthy, processes.len()) {
            (true, 0) => "已停止，可以安全换代或启动",
            (true, 1) => "正常运行",
            _ => "需要检查，暂时不要启动或换代",
        };
        println!("记录器状态：{headline}");
        match processes {
            [] => println!("  正在运行：没有"),
            [process] => {
                let role = managed_process_role(process, state, root).unwrap_or("未受管理的版本");
                let role = match role {
                    "current" | "current-equivalent" => "当前版",
                    "candidate" => "待升级版",
                    "previous" => "可回退版",
                    "official" => "尚未登记的正式版",
                    _ => "未受管理的版本",
                };
                println!("  正在运行：{role}（PID {}）", process.pid);
            }
            many => {
                println!(
                    "  正在运行：{} 个进程（异常，控制器会拒绝操作）",
                    many.len()
                );
            }
        }
        println!("  当前版本：{}", current.as_deref().unwrap_or("尚未配置"));
        println!("  待升级版本：{}", candidate.as_deref().unwrap_or("没有"));
        println!(
            "  可回退版本：{}",
            previous
                .as_deref()
                .unwrap_or("尚无；第一次升级后会自动保留")
        );
        print_human_active(active, processes)?;
        let next = if !healthy || processes.len() > 1 {
            "先不要操作，请检查是否启动了多个或未受管理的记录器。"
        } else if processes.len() == 1 && candidate.is_some() {
            "待升级版已经准备好；想现在切换时依次运行：drain → promote → run --session-kind daily --background → status。"
        } else if processes.len() == 1 {
            "当前版正在正常采集，可以继续使用；暂时不需要操作。"
        } else if candidate.is_some() {
            "记录器已经停止，可以运行 promote；提升后再运行 run --session-kind daily --background。"
        } else if current.is_some() {
            "记录器已经停止，可以运行 run --session-kind daily --background。"
        } else {
            "请先用 adopt 收编一个已知良好版本。"
        };
        println!("  下一步：{next}");
        println!("  隐私：未读取采集内容，未写入文件，未连接网络。");
        Ok(())
    }

    fn print_human_active(
        active: Option<&ActiveState>,
        processes: &[RecorderProcess],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(active) = active else {
            if !processes.is_empty() {
                println!("  会话详情：当前版本尚未启用脱敏运行状态；升级后即可查看");
            } else {
                println!("  会话详情：没有已发布的会话状态");
            }
            return Ok(());
        };
        let is_current = processes.iter().any(|process| process.pid == active.pid);
        let elapsed = unix_ms_now()?.saturating_sub(active.started_unix_ms);
        let ownership = if is_current {
            "当前会话"
        } else {
            "上一次已发布会话"
        };
        println!(
            "  会话详情：{ownership} {}（{}，{}）",
            active.session,
            human_session_kind(&active.kind),
            human_session_age(is_current, elapsed)
        );
        println!(
            "  会话状态：{}",
            human_session_status(is_current, &active.phase, &active.target)
        );
        println!(
            "  已安全保存：{} 个加密分段，{} 个事件",
            active.saved_segments, active.saved_events
        );
        match active.last_flush_unix_ms {
            Some(flush) => println!(
                "  最近刷新：{}前",
                format_duration(unix_ms_now()?.saturating_sub(flush))
            ),
            None => println!("  最近刷新：尚未产生非空分段"),
        }
        Ok(())
    }

    fn human_phase(is_current: bool, phase: &str) -> &'static str {
        match (is_current, phase) {
            (false, "running" | "paused") => {
                "进程已不在；没有正常退出回执（外部结束或不可观测中止）"
            }
            (_, "running") => "运行中",
            (_, "paused") => "已暂停",
            (_, "stopped") => "已正常停止",
            (_, "failed") => "记录器已报告内部错误并停止",
            _ => "未知",
        }
    }

    fn human_session_age(is_current: bool, elapsed_ms: u64) -> String {
        if is_current {
            format!("已运行{}", format_duration(elapsed_ms))
        } else {
            format!("开始于{}前", format_duration(elapsed_ms))
        }
    }

    fn human_session_status(is_current: bool, phase: &str, target: &str) -> String {
        let phase_text = human_phase(is_current, phase);
        if !is_current || !matches!(phase, "running" | "paused") {
            return phase_text.to_owned();
        }
        let target_text = match target {
            "connected" => "Codex 输入框已连接",
            "waiting" => "正在等待 Codex 输入框",
            _ => "目标状态未知",
        };
        format!("{phase_text}；{target_text}")
    }

    fn print_machine_active(
        active: Option<&ActiveState>,
        processes: &[RecorderProcess],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(active) = active else {
            println!(
                "RECORDERCTL_SESSION schema={ACTIVE_SCHEMA} state=unavailable \
                 path_disclosed=false contains_behavioral_metadata=true"
            );
            return Ok(());
        };
        let ownership = if processes.iter().any(|process| process.pid == active.pid) {
            "current"
        } else {
            "last"
        };
        println!(
            "RECORDERCTL_SESSION schema={} state={} pid={} session={} kind={} \
             producer_version={} capture_profile={} phase={} target={} elapsed_ms={} \
             saved_segments={} saved_events={} last_flush_unix_ms={} path_disclosed=false \
             contains_behavioral_metadata=true",
            ACTIVE_SCHEMA,
            ownership,
            active.pid,
            sanitize_token(&active.session),
            sanitize_token(&active.kind),
            sanitize_token(&active.producer_version),
            sanitize_token(&active.capture_profile),
            sanitize_token(&active.phase),
            sanitize_token(&active.target),
            unix_ms_now()?.saturating_sub(active.started_unix_ms),
            active.saved_segments,
            active.saved_events,
            active
                .last_flush_unix_ms
                .map_or_else(|| "none".to_owned(), |value| value.to_string())
        );
        Ok(())
    }

    fn human_session_kind(kind: &str) -> &'static str {
        match kind {
            "daily" => "日常",
            "course" => "课程",
            "theme" => "主题",
            _ => "未知类别",
        }
    }

    fn format_duration(milliseconds: u64) -> String {
        let total_seconds = milliseconds / 1000;
        let days = total_seconds / 86_400;
        let hours = (total_seconds % 86_400) / 3_600;
        let minutes = (total_seconds % 3_600) / 60;
        let seconds = total_seconds % 60;
        if days > 0 {
            format!("{days}天{hours}小时")
        } else if hours > 0 {
            format!("{hours}小时{minutes}分钟")
        } else if minutes > 0 {
            format!("{minutes}分钟{seconds}秒")
        } else {
            format!("{seconds}秒")
        }
    }

    fn unix_ms_now() -> Result<u64, Box<dyn std::error::Error>> {
        Ok(u64::try_from(
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        )?)
    }

    fn human_slot_version(
        root: &Path,
        file_name: Option<&str>,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let Some(file_name) = file_name else {
            return Ok(None);
        };
        validate_slot_name(file_name)?;
        let binary = root.join("builds").join(file_name);
        Ok(Some(read_build_metadata(&binary)?.producer_version))
    }

    fn print_slot(
        role: &str,
        file_name: Option<&str>,
        root: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(file_name) = file_name else {
            println!("RECORDERCTL_SLOT role={role} state=empty");
            return Ok(());
        };
        validate_slot_name(file_name)?;
        let binary = root.join("builds").join(file_name);
        let metadata = read_build_metadata(&binary)?;
        println!(
            "RECORDERCTL_SLOT role={} state=ready producer_version={} capture_profile={} \
             bytes={} file=\"{}\" path_disclosed=false",
            role,
            sanitize_token(&metadata.producer_version),
            sanitize_token(&metadata.capture_profile),
            metadata.byte_len,
            sanitize_field(file_name)
        );
        Ok(())
    }

    fn machine_process_line(pid: u32, role: &str) -> String {
        format!(
            "RECORDERCTL_PROCESS pid={} role={} managed={} path=\"redacted\" \
             path_disclosed=false contains_behavioral_metadata=true",
            pid,
            sanitize_token(role),
            role != "unmanaged"
        )
    }

    fn adopt(source: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let root = prepare_runtime_root()?;
        let mut state = read_state_if_present(&root)?.unwrap_or_default();
        if state.current.is_some() {
            return Err("current is already configured; use stage and promote for upgrades".into());
        }
        let processes = recorder_processes()?;
        if !processes.is_empty()
            && (processes.len() != 1 || !same_process_path(&processes[0], source)?)
        {
            return Err(
                "adopt while running requires exactly one recorder launched from the source path"
                    .into(),
            );
        }
        let installed = install_build(source, &root)?;
        state.current = Some(installed.file_name);
        write_state_atomic(&root, &state)?;
        println!(
            "已收编当前良好版：{}。正在运行的会话没有被改变。",
            installed.metadata.producer_version
        );
        println!(
            "RECORDERCTL_ADOPTED producer_version={} capture_profile={} current_ready=true \
             running_unchanged={} writes_capture_data=false",
            sanitize_token(&installed.metadata.producer_version),
            sanitize_token(&installed.metadata.capture_profile),
            !processes.is_empty()
        );
        Ok(())
    }

    fn stage(source: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let root = prepare_runtime_root()?;
        let mut state = read_state_if_present(&root)?.ok_or(
            "recorder lifecycle is unconfigured; adopt the current known-good recorder first",
        )?;
        let installed = install_build(source, &root)?;
        if state.current.as_deref() == Some(installed.file_name.as_str()) {
            return Err("candidate unexpectedly resolves to the active build file".into());
        }
        state.candidate = Some(installed.file_name);
        write_state_atomic(&root, &state)?;
        println!(
            "待升级版已准备好：{}。当前记录器仍照常运行。",
            installed.metadata.producer_version
        );
        println!(
            "RECORDERCTL_STAGED producer_version={} capture_profile={} candidate_ready=true \
             running_unchanged=true writes_capture_data=false",
            sanitize_token(&installed.metadata.producer_version),
            sanitize_token(&installed.metadata.capture_profile)
        );
        Ok(())
    }

    fn promote() -> Result<(), Box<dyn std::error::Error>> {
        ensure_no_recorders()?;
        let root = prepare_runtime_root()?;
        let mut state =
            read_state_if_present(&root)?.ok_or("recorder lifecycle is unconfigured")?;
        let candidate = state
            .candidate
            .take()
            .ok_or("no staged candidate is available")?;
        validate_installed_build(&root, &candidate)?;
        let old_current = state
            .current
            .replace(candidate)
            .ok_or("current slot is empty")?;
        state.previous = Some(old_current);
        write_state_atomic(&root, &state)?;
        println!(
            "升级完成：当前版本是 {}，上一版本已保留，可在停止状态下回退。",
            metadata_version_for_slot(&root, state.current.as_deref())?
        );
        println!(
            "RECORDERCTL_PROMOTED current={} previous={} candidate=empty \
             running=false capture_data_rewritten=false",
            metadata_version_for_slot(&root, state.current.as_deref())?,
            metadata_version_for_slot(&root, state.previous.as_deref())?
        );
        Ok(())
    }

    fn rollback() -> Result<(), Box<dyn std::error::Error>> {
        ensure_no_recorders()?;
        let root = prepare_runtime_root()?;
        let mut state =
            read_state_if_present(&root)?.ok_or("recorder lifecycle is unconfigured")?;
        let current = state.current.take().ok_or("current slot is empty")?;
        let previous = state.previous.take().ok_or("previous slot is empty")?;
        validate_installed_build(&root, &current)?;
        validate_installed_build(&root, &previous)?;
        state.current = Some(previous);
        state.previous = Some(current);
        write_state_atomic(&root, &state)?;
        println!(
            "回退完成：当前版本恢复为 {}，被替换版本仍保留在 previous 槽。",
            metadata_version_for_slot(&root, state.current.as_deref())?
        );
        println!(
            "RECORDERCTL_ROLLED_BACK current={} previous={} running=false \
             capture_data_rewritten=false",
            metadata_version_for_slot(&root, state.current.as_deref())?,
            metadata_version_for_slot(&root, state.previous.as_deref())?
        );
        Ok(())
    }

    fn run_current(session_kind: &str, background: bool) -> Result<(), Box<dyn std::error::Error>> {
        validate_session_kind(session_kind)?;
        ensure_no_recorders()?;
        let root = runtime_root();
        let state = read_state_if_present(&root)?.ok_or("recorder lifecycle is unconfigured")?;
        let current = state.current.as_deref().ok_or("current slot is empty")?;
        let binary = validate_installed_build(&root, current)?;
        let check = run_preflight(&binary)?;
        if !check.exact_policy || check.candidates == 0 {
            return Err("current recorder failed the exact Codex target preflight".into());
        }
        let mut command = Command::new(&binary);
        command
            .arg("--run")
            .arg("--session-kind")
            .arg(session_kind)
            .current_dir(manifest_dir());
        let control_state_enabled =
            check.metadata.control_state_schema.as_deref() == Some(ACTIVE_SCHEMA);
        if control_state_enabled {
            command.arg("--control-state");
        }
        if background {
            command
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = command.spawn()?;
            thread::sleep(START_SETTLE);
            if let Some(status) = child.try_wait()? {
                return Err(format!("recorder exited during startup with {status}").into());
            }
            println!(
                "后台记录器已启动：版本 {}，PID {}。停止可用 Ctrl+Shift+F12 或 recorderctl drain。",
                check.metadata.producer_version,
                child.id()
            );
            println!(
                "RECORDERCTL_RUNNING pid={} mode=background session_kind={} \
                 producer_version={} capture_profile={} control_state={} stop=Ctrl+Shift+F12",
                child.id(),
                session_kind,
                sanitize_token(&check.metadata.producer_version),
                sanitize_token(&check.metadata.capture_profile),
                control_state_enabled
            );
            return Ok(());
        }
        println!(
            "即将以前台方式启动记录器：版本 {}。此命令会持续占用当前终端。",
            check.metadata.producer_version
        );
        println!(
            "RECORDERCTL_FOREGROUND_START session_kind={} producer_version={} capture_profile={} \
             control_state={}",
            session_kind,
            sanitize_token(&check.metadata.producer_version),
            sanitize_token(&check.metadata.capture_profile),
            control_state_enabled
        );
        let status = command.status()?;
        if !status.success() {
            return Err(format!("recorder exited with {status}").into());
        }
        Ok(())
    }

    fn drain() -> Result<(), Box<dyn std::error::Error>> {
        let root = runtime_root();
        let state = read_state_if_present(&root)?;
        let processes = recorder_processes()?;
        if processes.len() != 1 {
            return Err(format!(
                "drain requires exactly one recorder process, found {}",
                processes.len()
            )
            .into());
        }
        let process = &processes[0];
        let Some(role) = managed_process_role(process, state.as_ref(), &root) else {
            return Err("refusing to drain an unmanaged recorder executable".into());
        };
        let posted = post_stop_to_process_threads(process.pid)?;
        if posted == 0 {
            return Err("the recorder message thread did not accept the stop request".into());
        }
        let deadline = Instant::now() + STOP_TIMEOUT;
        while Instant::now() < deadline {
            if !process_exists(process.pid)? {
                println!(
                    "记录器已安全停止：PID {} 已自行解绑、刷新并退出，没有强制终止。",
                    process.pid
                );
                println!(
                    "RECORDERCTL_DRAINED pid={} role={} flushed_by_recorder=true \
                     forced_termination=false",
                    process.pid, role
                );
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err("recorder did not exit within 15 seconds; no forced termination was attempted".into())
    }

    fn ensure_no_recorders() -> Result<(), Box<dyn std::error::Error>> {
        let processes = recorder_processes()?;
        if processes.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "operation requires no recorder process; found {}",
                processes.len()
            )
            .into())
        }
    }

    #[derive(Debug)]
    struct InstalledBuild {
        file_name: String,
        metadata: BuildMetadata,
    }

    fn install_build(
        source: &Path,
        root: &Path,
    ) -> Result<InstalledBuild, Box<dyn std::error::Error>> {
        let source = validate_source_binary(source)?;
        let builds = root.join("builds");
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let file_name = format!("codex-recorder-build-{stamp}-{}.exe", std::process::id());
        validate_slot_name(&file_name)?;
        let destination = builds.join(&file_name);
        let temporary = builds.join(format!(".{file_name}.copying"));
        copy_new(&source, &temporary)?;
        let result = (|| {
            let check = run_preflight(&temporary)?;
            if !check.exact_policy || check.candidates == 0 {
                return Err::<BuildMetadata, Box<dyn std::error::Error>>(
                    "candidate failed the exact Codex target preflight".into(),
                );
            }
            let byte_len = fs::metadata(&temporary)?.len();
            let metadata = BuildMetadata {
                producer_version: check.metadata.producer_version,
                capture_profile: check.metadata.capture_profile,
                control_state_schema: check.metadata.control_state_schema,
                byte_len,
            };
            fs::rename(&temporary, &destination)?;
            write_build_metadata(&destination, &metadata)?;
            Ok(metadata)
        })();
        if temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        let metadata = result?;
        Ok(InstalledBuild {
            file_name,
            metadata,
        })
    }

    fn validate_source_binary(source: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let metadata = fs::symlink_metadata(source)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("recorder source must be a regular non-symlink file".into());
        }
        if source
            .file_name()
            .and_then(OsStr::to_str)
            .is_none_or(|name| !name.eq_ignore_ascii_case(RECORDER_EXE_NAME))
        {
            return Err("recorder source filename must be codex-recorder.exe".into());
        }
        Ok(fs::canonicalize(source)?)
    }

    fn copy_new(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        Ok(())
    }

    #[derive(Debug)]
    struct Preflight {
        candidates: usize,
        exact_policy: bool,
        metadata: BuildMetadata,
    }

    fn run_preflight(binary: &Path) -> Result<Preflight, Box<dyn std::error::Error>> {
        let output = Command::new(binary)
            .arg("--check")
            .current_dir(manifest_dir())
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "candidate --check failed: {}",
                sanitize_field(&String::from_utf8_lossy(&output.stderr))
            )
            .into());
        }
        let stdout = String::from_utf8(output.stdout)?;
        let line = stdout
            .lines()
            .find(|line| line.starts_with("CODEX_RECORDER_CHECK "))
            .ok_or("candidate --check omitted CODEX_RECORDER_CHECK")?;
        let candidates = field_value(line, "candidates")?.parse()?;
        let exact_policy = field_value(line, "exact_policy")? == "true";
        let producer_version = validate_metadata_token(field_value(line, "producer_version")?)?;
        let capture_profile = validate_metadata_token(field_value(line, "capture_profile")?)?;
        let control_state_schema = field_value_optional(line, "control_state_schema")
            .map(validate_metadata_token)
            .transpose()?;
        Ok(Preflight {
            candidates,
            exact_policy,
            metadata: BuildMetadata {
                producer_version,
                capture_profile,
                control_state_schema,
                byte_len: fs::metadata(binary)?.len(),
            },
        })
    }

    fn field_value<'a>(line: &'a str, key: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
        let prefix = format!("{key}=");
        line.split_ascii_whitespace()
            .find_map(|field| field.strip_prefix(&prefix))
            .ok_or_else(|| format!("missing {key} in recorder output").into())
    }

    fn field_value_optional<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        let prefix = format!("{key}=");
        line.split_ascii_whitespace()
            .find_map(|field| field.strip_prefix(&prefix))
    }

    fn validate_metadata_token(value: &str) -> Result<String, Box<dyn std::error::Error>> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".+-_".contains(&byte))
        {
            return Err("recorder metadata contains an unsafe token".into());
        }
        Ok(value.to_owned())
    }

    fn write_build_metadata(
        binary: &Path,
        metadata: &BuildMetadata,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = build_metadata_path(binary)?;
        let body = format!(
            "schema={BUILD_SCHEMA}\nproducer_version={}\ncapture_profile={}\n\
             control_state_schema={}\nbyte_len={}\n",
            metadata.producer_version,
            metadata.capture_profile,
            metadata.control_state_schema.as_deref().unwrap_or("-"),
            metadata.byte_len
        );
        write_new_synced(&path, body.as_bytes())
    }

    fn read_build_metadata(binary: &Path) -> Result<BuildMetadata, Box<dyn std::error::Error>> {
        let path = build_metadata_path(binary)?;
        let body = read_bounded_text(&path, 4096)?;
        let fields = parse_key_value_lines(&body)?;
        if fields.get("schema").map(String::as_str) != Some(BUILD_SCHEMA) {
            return Err("unsupported recorder build metadata schema".into());
        }
        let producer_version = validate_metadata_token(
            fields
                .get("producer_version")
                .ok_or("build metadata lacks producer_version")?,
        )?;
        let capture_profile = validate_metadata_token(
            fields
                .get("capture_profile")
                .ok_or("build metadata lacks capture_profile")?,
        )?;
        let control_state_schema = fields
            .get("control_state_schema")
            .filter(|value| value.as_str() != "-")
            .map(|value| validate_metadata_token(value))
            .transpose()?;
        let byte_len: u64 = fields
            .get("byte_len")
            .ok_or("build metadata lacks byte_len")?
            .parse()?;
        let actual = fs::metadata(binary)?;
        if !actual.is_file() || actual.len() != byte_len {
            return Err("installed recorder length does not match its metadata".into());
        }
        Ok(BuildMetadata {
            producer_version,
            capture_profile,
            control_state_schema,
            byte_len,
        })
    }

    fn build_metadata_path(binary: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let file_name = binary
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or("installed recorder has an invalid filename")?;
        Ok(binary.with_file_name(format!("{file_name}.meta")))
    }

    fn validate_installed_build(
        root: &Path,
        file_name: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        validate_slot_name(file_name)?;
        let binary = root.join("builds").join(file_name);
        let metadata = fs::symlink_metadata(&binary)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("installed recorder must be a regular non-symlink file".into());
        }
        let _ = read_build_metadata(&binary)?;
        Ok(binary)
    }

    fn metadata_version_for_slot(
        root: &Path,
        file_name: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let file_name = file_name.ok_or("slot is empty")?;
        let binary = validate_installed_build(root, file_name)?;
        Ok(sanitize_token(
            &read_build_metadata(&binary)?.producer_version,
        ))
    }

    fn runtime_root() -> PathBuf {
        manifest_dir().join(".local").join("recorder")
    }

    fn manifest_dir() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    fn prepare_runtime_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let manifest = manifest_dir();
        let root = runtime_root();
        let mut current = manifest.to_path_buf();
        for component in [".local", "recorder", "builds"] {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(
                        "refusing symlink in recorder runtime path; location suppressed".into(),
                    );
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(
                        "recorder runtime component is not a directory; location suppressed".into(),
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(root)
    }

    fn state_path(root: &Path) -> PathBuf {
        root.join("slots-v1.txt")
    }

    fn active_path(root: &Path) -> PathBuf {
        root.join("active-v1.txt")
    }

    fn read_active_if_present(
        root: &Path,
    ) -> Result<Option<ActiveState>, Box<dyn std::error::Error>> {
        let path = active_path(root);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("recorder active state must be a regular non-symlink file".into());
        }
        let body = read_bounded_text(&path, 4096)?;
        Ok(Some(parse_active_state(&body)?))
    }

    fn parse_active_state(body: &str) -> Result<ActiveState, Box<dyn std::error::Error>> {
        let fields = parse_key_value_lines(body)?;
        if fields.len() != 12 || fields.get("schema").map(String::as_str) != Some(ACTIVE_SCHEMA) {
            return Err("unsupported or structurally invalid recorder active state".into());
        }
        let pid: u32 = fields.get("pid").ok_or("active state lacks pid")?.parse()?;
        if pid == 0 {
            return Err("active state pid must be positive".into());
        }
        let session =
            validate_metadata_token(fields.get("session").ok_or("active state lacks session")?)?;
        let kind = fields
            .get("kind")
            .ok_or("active state lacks kind")?
            .to_owned();
        validate_session_kind(&kind)?;
        let producer_version = validate_metadata_token(
            fields
                .get("producer_version")
                .ok_or("active state lacks producer_version")?,
        )?;
        let capture_profile = validate_metadata_token(
            fields
                .get("capture_profile")
                .ok_or("active state lacks capture_profile")?,
        )?;
        let started_unix_ms: u64 = fields
            .get("started_unix_ms")
            .ok_or("active state lacks started_unix_ms")?
            .parse()?;
        if started_unix_ms == 0 {
            return Err("active state start time must be positive".into());
        }
        let phase = fields
            .get("phase")
            .filter(|value| matches!(value.as_str(), "running" | "paused" | "stopped" | "failed"))
            .ok_or("active state has an invalid phase")?
            .to_owned();
        let target = fields
            .get("target")
            .filter(|value| matches!(value.as_str(), "waiting" | "connected"))
            .ok_or("active state has an invalid target")?
            .to_owned();
        let saved_segments = fields
            .get("saved_segments")
            .ok_or("active state lacks saved_segments")?
            .parse()?;
        let saved_events = fields
            .get("saved_events")
            .ok_or("active state lacks saved_events")?
            .parse()?;
        let last_flush_unix_ms = match fields
            .get("last_flush_unix_ms")
            .ok_or("active state lacks last_flush_unix_ms")?
            .as_str()
        {
            "-" => None,
            value => Some(value.parse()?),
        };
        Ok(ActiveState {
            pid,
            session,
            kind,
            producer_version,
            capture_profile,
            started_unix_ms,
            phase,
            target,
            saved_segments,
            saved_events,
            last_flush_unix_ms,
        })
    }

    fn read_state_if_present(root: &Path) -> Result<Option<SlotState>, Box<dyn std::error::Error>> {
        let path = state_path(root);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("recorder state must be a regular non-symlink file".into());
        }
        let body = read_bounded_text(&path, 4096)?;
        Ok(Some(parse_state(&body)?))
    }

    fn parse_state(body: &str) -> Result<SlotState, Box<dyn std::error::Error>> {
        let fields = parse_key_value_lines(body)?;
        if fields.get("schema").map(String::as_str) != Some(STATE_SCHEMA) {
            return Err("unsupported recorder lifecycle schema".into());
        }
        let state = SlotState {
            current: parse_optional_slot(fields.get("current"))?,
            candidate: parse_optional_slot(fields.get("candidate"))?,
            previous: parse_optional_slot(fields.get("previous"))?,
        };
        if state.current.is_none() && (state.candidate.is_some() || state.previous.is_some()) {
            return Err("recorder state cannot have candidate/previous without current".into());
        }
        Ok(state)
    }

    fn parse_optional_slot(
        value: Option<&String>,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let value = value.ok_or("recorder state is missing a slot field")?;
        if value == "-" {
            return Ok(None);
        }
        validate_slot_name(value)?;
        Ok(Some(value.clone()))
    }

    fn write_state_atomic(
        root: &Path,
        state: &SlotState,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let body = format!(
            "schema={STATE_SCHEMA}\ncurrent={}\ncandidate={}\nprevious={}\n",
            state.current.as_deref().unwrap_or("-"),
            state.candidate.as_deref().unwrap_or("-"),
            state.previous.as_deref().unwrap_or("-")
        );
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let temporary = root.join(format!(".slots-{}-{stamp}.tmp", std::process::id()));
        write_new_synced(&temporary, body.as_bytes())?;
        let destination = state_path(root);
        let result = move_replace(&temporary, &destination);
        if result.is_err() && temporary.exists() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let source_wide = wide_path(source)?;
        let destination_wide = wide_path(destination)?;
        // SAFETY: Both NUL-terminated buffers live through the synchronous call.
        unsafe {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )?
        };
        Ok(())
    }

    fn wide_path(path: &Path) -> Result<Vec<u16>, Box<dyn std::error::Error>> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err("path contains an embedded NUL".into());
        }
        wide.push(0);
        Ok(wide)
    }

    fn write_new_synced(path: &Path, body: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(body)?;
        file.sync_all()?;
        Ok(())
    }

    fn read_bounded_text(path: &Path, limit: u64) -> Result<String, Box<dyn std::error::Error>> {
        let metadata = fs::metadata(path)?;
        if metadata.len() > limit {
            return Err("recorder control metadata exceeds its size limit".into());
        }
        let mut body = String::new();
        File::open(path)?
            .take(limit + 1)
            .read_to_string(&mut body)?;
        if body.len() as u64 > limit {
            return Err("recorder control metadata exceeds its size limit".into());
        }
        Ok(body)
    }

    fn parse_key_value_lines(
        body: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
        let mut fields = std::collections::BTreeMap::new();
        for line in body.lines() {
            let (key, value) = line
                .split_once('=')
                .ok_or("recorder control metadata has a malformed line")?;
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                || value.contains(['\r', '\n'])
                || fields.insert(key.to_owned(), value.to_owned()).is_some()
            {
                return Err("recorder control metadata has an invalid or duplicate field".into());
            }
        }
        Ok(fields)
    }

    fn validate_slot_name(value: &str) -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new(value);
        let mut components = path.components();
        let one_normal =
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
        if !one_normal
            || value.len() > 160
            || value.starts_with('.')
            || !value.ends_with(".exe")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte))
        {
            return Err("recorder slot contains an unsafe build filename".into());
        }
        Ok(())
    }

    fn recorder_processes() -> Result<Vec<RecorderProcess>, Box<dyn std::error::Error>> {
        // SAFETY: Snapshot handle is owned by the guard and structures have dwSize set.
        let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_err() {
            return Ok(processes);
        }
        loop {
            let name = nul_terminated_utf16(&entry.szExeFile);
            let lower_name = name.to_ascii_lowercase();
            let official_name = name.eq_ignore_ascii_case(RECORDER_EXE_NAME);
            let managed_build_name = lower_name.starts_with("codex-recorder-build-");
            let legacy_build_name = lower_name.starts_with("build-");
            if official_name || managed_build_name || legacy_build_name {
                let path = query_process_path(entry.th32ProcessID);
                let legacy_managed_build = legacy_build_name
                    && path
                        .as_deref()
                        .and_then(Path::parent)
                        .is_some_and(|parent| same_path(parent, &runtime_root().join("builds")));
                if official_name || managed_build_name || legacy_managed_build {
                    processes.push(RecorderProcess {
                        pid: entry.th32ProcessID,
                        path,
                    });
                }
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
                break;
            }
        }
        processes.sort_by_key(|process| process.pid);
        Ok(processes)
    }

    fn query_process_path(pid: u32) -> Option<PathBuf> {
        // SAFETY: The process handle is query-only and closed by the guard.
        let process = OwnedHandle(
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?,
        );
        let mut buffer = vec![0_u16; 32_768];
        let mut length = buffer.len() as u32;
        // SAFETY: Buffer is writable and length describes its capacity.
        unsafe {
            QueryFullProcessImageNameW(
                process.0,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
            .ok()?
        };
        buffer.truncate(length as usize);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
    }

    fn process_exists(pid: u32) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(recorder_processes()?
            .iter()
            .any(|process| process.pid == pid))
    }

    fn managed_process_role<'a>(
        process: &RecorderProcess,
        state: Option<&'a SlotState>,
        root: &Path,
    ) -> Option<&'a str> {
        let path = process.path.as_deref()?;
        let official = manifest_dir()
            .join("target")
            .join("release")
            .join(RECORDER_EXE_NAME);
        if same_path(path, &official) {
            let Some(state) = state else {
                return Some("official");
            };
            let current = state.current.as_deref()?;
            if files_equal(path, &root.join("builds").join(current)) {
                return Some("current-equivalent");
            }
            return None;
        }
        let state = state?;
        for (role, file_name) in [
            ("current", state.current.as_deref()),
            ("candidate", state.candidate.as_deref()),
            ("previous", state.previous.as_deref()),
        ] {
            if let Some(file_name) = file_name
                && same_path(path, &root.join("builds").join(file_name))
            {
                return Some(role);
            }
        }
        None
    }

    fn same_process_path(
        process: &RecorderProcess,
        expected: &Path,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(actual) = process.path.as_deref() else {
            return Ok(false);
        };
        Ok(same_path(actual, &fs::canonicalize(expected)?))
    }

    fn same_path(left: &Path, right: &Path) -> bool {
        let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
        let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }

    fn files_equal(left: &Path, right: &Path) -> bool {
        let Ok(left_metadata) = fs::metadata(left) else {
            return false;
        };
        let Ok(right_metadata) = fs::metadata(right) else {
            return false;
        };
        if !left_metadata.is_file()
            || !right_metadata.is_file()
            || left_metadata.len() != right_metadata.len()
        {
            return false;
        }
        let Ok(mut left_file) = File::open(left) else {
            return false;
        };
        let Ok(mut right_file) = File::open(right) else {
            return false;
        };
        let mut left_buffer = [0_u8; 64 * 1024];
        let mut right_buffer = [0_u8; 64 * 1024];
        loop {
            let Ok(left_read) = left_file.read(&mut left_buffer) else {
                return false;
            };
            let Ok(right_read) = right_file.read(&mut right_buffer) else {
                return false;
            };
            if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
                return false;
            }
            if left_read == 0 {
                return true;
            }
        }
    }

    fn post_stop_to_process_threads(pid: u32) -> Result<usize, Box<dyn std::error::Error>> {
        // SAFETY: Snapshot handle is owned by the guard and structure has dwSize set.
        let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)? });
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut posted = 0;
        if unsafe { Thread32First(snapshot.0, &mut entry) }.is_err() {
            return Ok(0);
        }
        loop {
            if entry.th32OwnerProcessID == pid
                // SAFETY: WM_HOTKEY is posted only to threads owned by the exact
                // managed recorder process. The recorder consumes this fixed id.
                && unsafe {
                    PostThreadMessageW(
                        entry.th32ThreadID,
                        WM_HOTKEY,
                        WPARAM(STOP_HOTKEY_ID),
                        LPARAM(0),
                    )
                }
                .is_ok()
            {
                posted += 1;
            }
            if unsafe { Thread32Next(snapshot.0, &mut entry) }.is_err() {
                break;
            }
        }
        Ok(posted)
    }

    fn nul_terminated_utf16(value: &[u16]) -> String {
        let length = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..length])
    }

    fn sanitize_field(value: &str) -> String {
        value.replace('"', "'").replace(['\r', '\n'], " ")
    }

    fn sanitize_token(value: &str) -> String {
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || ".+-_".contains(character) {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::{
            ACTIVE_SCHEMA, ActiveState, BUILD_SCHEMA, ControllerCommand, STATE_SCHEMA, SlotState,
            field_value, field_value_optional, format_duration, human_phase, human_session_age,
            human_session_status, machine_process_line, parse_active_state, parse_command,
            parse_key_value_lines, parse_state, sanitize_field, validate_slot_name,
        };

        fn arguments(values: &[&str]) -> Vec<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        }

        #[test]
        fn command_parser_requires_explicit_bounded_operations() {
            assert_eq!(
                parse_command(arguments(&["status"])).unwrap(),
                Some(ControllerCommand::Status { machine: false })
            );
            assert_eq!(
                parse_command(arguments(&["status", "--machine"])).unwrap(),
                Some(ControllerCommand::Status { machine: true })
            );
            assert_eq!(
                parse_command(arguments(&[
                    "run",
                    "--session-kind",
                    "theme",
                    "--background"
                ]))
                .unwrap(),
                Some(ControllerCommand::Run {
                    session_kind: "theme".to_owned(),
                    background: true,
                })
            );
            assert!(parse_command(arguments(&[])).is_err());
            assert!(parse_command(arguments(&["run", "--session-kind", "invalid"])).is_err());
            assert!(parse_command(arguments(&["promote", "extra"])).is_err());
            assert!(parse_command(arguments(&["--help"])).unwrap().is_none());
            assert!(parse_command(arguments(&["--help", "extra"])).is_err());
        }

        #[test]
        fn rejected_controller_arguments_never_echo_their_values() {
            let marker = r"Z:\synthetic-private\PRIVATE_PATH_MARKER";
            for values in [
                vec![marker],
                vec!["status", marker],
                vec!["run", marker],
                vec!["promote", marker],
            ] {
                let error = parse_command(arguments(&values)).unwrap_err().to_string();
                assert!(error.contains("suppressed"));
                assert!(!error.contains("PRIVATE_PATH_MARKER"));
            }
        }

        #[test]
        fn state_parser_rejects_traversal_duplicates_and_orphan_slots() {
            let valid =
                format!("schema={STATE_SCHEMA}\ncurrent=build-1.exe\ncandidate=-\nprevious=-\n");
            assert_eq!(
                parse_state(&valid).unwrap(),
                SlotState {
                    current: Some("build-1.exe".to_owned()),
                    candidate: None,
                    previous: None,
                }
            );
            let traversal =
                format!("schema={STATE_SCHEMA}\ncurrent=../x.exe\ncandidate=-\nprevious=-\n");
            assert!(parse_state(&traversal).is_err());
            let duplicate = format!(
                "schema={STATE_SCHEMA}\ncurrent=build-1.exe\ncurrent=build-2.exe\ncandidate=-\nprevious=-\n"
            );
            assert!(parse_state(&duplicate).is_err());
            let orphan =
                format!("schema={STATE_SCHEMA}\ncurrent=-\ncandidate=build-2.exe\nprevious=-\n");
            assert!(parse_state(&orphan).is_err());
        }

        #[test]
        fn build_filenames_are_single_safe_components() {
            assert!(validate_slot_name("codex-recorder-build-123-4.exe").is_ok());
            for invalid in [
                "../build.exe",
                "nested/build.exe",
                r"nested\build.exe",
                ".hidden.exe",
                "build.txt",
                "build name.exe",
            ] {
                assert!(validate_slot_name(invalid).is_err(), "{invalid}");
            }
        }

        #[test]
        fn machine_output_fields_are_parsed_and_terminal_text_is_sanitized() {
            let line = "CODEX_RECORDER_CHECK candidates=1 exact_policy=true producer_version=0.1.0+continuous.5 capture_profile=codex-uia-v1 control_state_schema=ziranma-recorder-active-v1";
            assert_eq!(field_value(line, "candidates").unwrap(), "1");
            assert_eq!(
                field_value(line, "producer_version").unwrap(),
                "0.1.0+continuous.5"
            );
            assert_eq!(
                field_value_optional(line, "control_state_schema"),
                Some(ACTIVE_SCHEMA)
            );
            assert!(field_value(line, "missing").is_err());
            assert_eq!(sanitize_field("a\"\r\nb"), "a'  b");

            let process = machine_process_line(42, "current");
            assert!(process.contains("pid=42"));
            assert!(process.contains("path=\"redacted\""));
            assert!(process.contains("path_disclosed=false"));
            assert!(process.contains("contains_behavioral_metadata=true"));
            assert!(!process.contains("Users"));
        }

        #[test]
        fn active_state_is_strict_redacted_metadata() {
            let valid = format!(
                "schema={ACTIVE_SCHEMA}\npid=42\nsession=1234-42\nkind=daily\n\
                 producer_version=0.1.0+continuous.5\ncapture_profile=codex-uia-v1\n\
                 started_unix_ms=1234\nphase=running\ntarget=connected\nsaved_segments=2\n\
                 saved_events=17\nlast_flush_unix_ms=1300\n"
            );
            assert_eq!(
                parse_active_state(&valid).unwrap(),
                ActiveState {
                    pid: 42,
                    session: "1234-42".to_owned(),
                    kind: "daily".to_owned(),
                    producer_version: "0.1.0+continuous.5".to_owned(),
                    capture_profile: "codex-uia-v1".to_owned(),
                    started_unix_ms: 1234,
                    phase: "running".to_owned(),
                    target: "connected".to_owned(),
                    saved_segments: 2,
                    saved_events: 17,
                    last_flush_unix_ms: Some(1300),
                }
            );
            assert!(parse_active_state(&format!("{valid}text=猫猫\n")).is_err());
            assert!(parse_active_state(&valid.replace("phase=running", "phase=unknown")).is_err());
            assert_eq!(format_duration(12_345), "12秒");
            assert_eq!(format_duration(3_661_000), "1小时1分钟");
        }

        #[test]
        fn human_status_does_not_invent_an_unobserved_exit_cause() {
            assert_eq!(human_phase(true, "running"), "运行中");
            assert_eq!(
                human_phase(false, "running"),
                "进程已不在；没有正常退出回执（外部结束或不可观测中止）"
            );
            assert_eq!(human_phase(false, "failed"), "记录器已报告内部错误并停止");
            assert_eq!(
                human_session_status(true, "running", "waiting"),
                "运行中；正在等待 Codex 输入框"
            );
            assert_eq!(
                human_session_status(false, "stopped", "waiting"),
                "已正常停止"
            );
            assert_eq!(
                human_session_status(false, "running", "connected"),
                "进程已不在；没有正常退出回执（外部结束或不可观测中止）"
            );
            assert_eq!(human_session_age(true, 12_345), "已运行12秒");
            assert_eq!(human_session_age(false, 12_345), "开始于12秒前");
        }

        #[test]
        fn metadata_lines_require_unique_lowercase_keys() {
            let valid = format!(
                "schema={BUILD_SCHEMA}\nproducer_version=v1\ncapture_profile=p1\nbyte_len=1\n"
            );
            assert_eq!(
                parse_key_value_lines(&valid).unwrap().get("schema"),
                Some(&BUILD_SCHEMA.to_owned())
            );
            assert!(parse_key_value_lines("Bad=x\n").is_err());
            assert!(parse_key_value_lines("a=1\na=2\n").is_err());
            assert!(parse_key_value_lines("missing-separator\n").is_err());
        }
    }
}
