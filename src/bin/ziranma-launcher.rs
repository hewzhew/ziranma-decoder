#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("自然码桌面启动器目前只支持 Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = windows_launcher::run() {
        windows_launcher::show_error(&error.to_string());
    }
}

#[cfg(windows)]
mod windows_launcher {
    use std::error::Error;
    use std::ffi::OsStr;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    use windows::core::PCWSTR;
    use ziranma_core::{
        LaunchableUserTool, repository_root_for_desktop_launcher_executable,
        resolve_current_user_tool,
    };

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum LauncherAction {
        Wish,
        Alias,
        Practice,
        Update,
    }

    pub fn run() -> Result<(), Box<dyn Error>> {
        let action = parse_action(std::env::args_os().skip(1))?;
        let executable = std::env::current_exe()?;
        let repository = repository_root_for_desktop_launcher_executable(&executable)
            .ok_or("启动器必须从项目构建、用户工具包或固定桌面启动位置运行")?;
        match action {
            LauncherAction::Wish => launch_user_tool(&repository, LaunchableUserTool::WishPad),
            LauncherAction::Alias => launch_user_tool(&repository, LaunchableUserTool::AliasPad),
            LauncherAction::Practice => {
                launch_user_tool(&repository, LaunchableUserTool::TypingPractice)
            }
            LauncherAction::Update => launch_update(&repository),
        }
    }

    fn parse_action(
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Result<LauncherAction, &'static str> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        match arguments.as_slice() {
            [action] if action == OsStr::new("wish") => Ok(LauncherAction::Wish),
            [action] if action == OsStr::new("alias") => Ok(LauncherAction::Alias),
            [action] if action == OsStr::new("practice") => Ok(LauncherAction::Practice),
            [action] if action == OsStr::new("update") => Ok(LauncherAction::Update),
            _ => Err("启动器只接受 wish、alias、practice 或 update 中的一个固定动作"),
        }
    }

    fn launch_user_tool(repository: &Path, tool: LaunchableUserTool) -> Result<(), Box<dyn Error>> {
        let executable = resolve_current_user_tool(repository, tool)?;
        Command::new(executable)
            .current_dir(repository)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|_| "无法启动当前用户工具".into())
    }

    fn launch_update(repository: &Path) -> Result<(), Box<dyn Error>> {
        let update = repository.join("update-ime.cmd");
        if !update.is_file() {
            return Err("项目中的 update-ime.cmd 不可用".into());
        }
        let command_prompt = system_command_prompt()?;
        let command_line = update_command_line(&update);
        Command::new(command_prompt)
            .args(["/d", "/s", "/c"])
            .arg(command_line)
            .current_dir(repository)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
            .map_err(|_| "无法打开自然码换代窗口".into())
    }

    fn update_command_line(update: &Path) -> String {
        format!("call \"{}\" update", update.display())
    }

    fn system_command_prompt() -> Result<PathBuf, Box<dyn Error>> {
        let windows_root = std::env::var_os("SystemRoot").ok_or("无法确定 Windows 系统目录")?;
        let command_prompt = PathBuf::from(windows_root).join("System32").join("cmd.exe");
        if !command_prompt.is_file() {
            return Err("Windows 命令处理器不可用".into());
        }
        Ok(command_prompt)
    }

    pub fn show_error(message: &str) {
        let message = wide(message);
        let title = wide("自然码启动器");
        // SAFETY: both UTF-16 buffers remain valid for the duration of this
        // synchronous message box call.
        unsafe {
            let _ = MessageBoxW(
                None,
                PCWSTR(message.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parser_accepts_only_one_fixed_action() {
            assert_eq!(parse_action(["wish".into()]), Ok(LauncherAction::Wish));
            assert_eq!(parse_action(["alias".into()]), Ok(LauncherAction::Alias));
            assert_eq!(
                parse_action(["practice".into()]),
                Ok(LauncherAction::Practice)
            );
            assert_eq!(parse_action(["update".into()]), Ok(LauncherAction::Update));
            assert!(parse_action(Vec::new()).is_err());
            assert!(parse_action(["wish".into(), "extra".into()]).is_err());
            assert!(parse_action([r"C:\Windows\System32\cmd.exe".into()]).is_err());
        }

        #[test]
        fn desktop_update_passes_the_explicit_mutating_action() {
            assert_eq!(
                update_command_line(Path::new(r"project\update-ime.cmd")),
                r#"call "project\update-ime.cmd" update"#
            );
        }
    }
}
