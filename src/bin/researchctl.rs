use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

use ziranma_core::{
    RESEARCH_FEEDBACK_DIRECTORY, list_wish_packages, research_feedback_enabled,
    set_research_feedback_enabled,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Enable,
    Disable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    command: Command,
    root: Option<PathBuf>,
    confirmed: bool,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("持续研究设置失败：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    let root = match options.root {
        Some(root) => root,
        None => env::current_exe()
            .ok()
            .and_then(|path| research_root_for_executable(&path))
            .ok_or("无法从当前 release 工具位置确定持续研究目录")?,
    };
    match options.command {
        Command::Status => print_status(&root),
        Command::Enable => {
            if !options.confirmed {
                return Err("开启会持续保存普通输入域里的原码、候选与提交原文；请加入 \
                     --confirm-continuous-private-feedback"
                    .into());
            }
            let changed = set_research_feedback_enabled(&root, true)?;
            println!(
                "持续研究已{}\n生效：正在运行的输入法宿主会在后续输入中自动发现\n范围：猫猫输入法的普通输入域\n保存：当前用户 DPAPI 加密\n密码、PIN 与受限输入域：不记录\n网络：不连接",
                if changed { "开启" } else { "经开启" }
            );
            Ok(())
        }
        Command::Disable => {
            let changed = set_research_feedback_enabled(&root, false)?;
            println!(
                "持续研究已{}\n新增保存：已停止，运行中的宿主不会再发布批次\n已有加密批次：保留",
                if changed {
                    "关闭"
                } else {
                    "处于关闭状态"
                }
            );
            Ok(())
        }
    }
}

fn print_status(root: &Path) -> Result<(), Box<dyn Error>> {
    let enabled = research_feedback_enabled(root)?;
    let packages = list_wish_packages(root)?.len();
    println!(
        "持续研究：{}\n已保存加密批次：{packages}\n原文显示：没有\n网络：没有",
        if enabled { "已开启" } else { "已关闭" }
    );
    Ok(())
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref() {
        Some("status") => Command::Status,
        Some("enable") => Command::Enable,
        Some("disable") => Command::Disable,
        _ => return Err(usage().into()),
    };
    let mut root = None;
    let mut confirmed = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(arguments.next().ok_or("--root 后缺少目录")?));
            }
            "--confirm-continuous-private-feedback" if !confirmed => confirmed = true,
            _ => return Err(usage().into()),
        }
    }
    if command != Command::Enable && confirmed {
        return Err(usage().into());
    }
    Ok(Options {
        command,
        root,
        confirmed,
    })
}

fn research_root_for_executable(executable: &Path) -> Option<PathBuf> {
    let release = executable.parent()?;
    let target = release.parent()?;
    let repository = target.parent()?;
    if release.file_name()?.to_str()? != "release"
        || target.file_name()?.to_str()? != "target"
        || executable.file_stem()?.to_str()? != "researchctl"
    {
        return None;
    }
    Some(
        repository
            .join(".local")
            .join("tsf-alpha")
            .join("user-data")
            .join(RESEARCH_FEEDBACK_DIRECTORY),
    )
}

fn usage() -> String {
    "用法：\n  researchctl status [--root <目录>]\n  researchctl enable \
     --confirm-continuous-private-feedback [--root <目录>]\n  researchctl disable [--root <目录>]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_the_private_capture_confirmation_only_for_enable() {
        assert_eq!(
            parse_options(["status".to_owned()]).unwrap(),
            Options {
                command: Command::Status,
                root: None,
                confirmed: false,
            }
        );
        assert!(!parse_options(["enable".to_owned()]).unwrap().confirmed);
        assert!(
            parse_options([
                "enable".to_owned(),
                "--confirm-continuous-private-feedback".to_owned(),
            ])
            .unwrap()
            .confirmed
        );
        assert!(
            parse_options([
                "disable".to_owned(),
                "--confirm-continuous-private-feedback".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn release_tool_derives_the_stable_installed_user_data_root() {
        assert_eq!(
            research_root_for_executable(Path::new(
                r"D:\IME\ziranma-decoder\target\release\researchctl.exe"
            )),
            Some(PathBuf::from(
                r"D:\IME\ziranma-decoder\.local\tsf-alpha\user-data\research-inbox"
            ))
        );
        assert!(research_root_for_executable(Path::new(r"D:\tools\researchctl.exe")).is_none());
    }
}
