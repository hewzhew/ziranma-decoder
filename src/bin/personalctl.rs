//! Inspect or recoverably archive the local personal-ranking evidence store.

use std::env;
use std::error::Error;
use std::fs;
#[cfg(windows)]
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use ziranma_core::{
    PERSONAL_RANKING_SUPPRESSION_DIRECTORY, PersonalRankingSuppressionAction,
    PersonalRankingSuppressionActionKind, WindowsUserDataProtector, load_personal_ranking,
    load_personal_ranking_suppressions, save_personal_ranking_suppression_action,
};

const MAX_PRIVATE_CODE_BYTES: usize = 64;
const MAX_PRIVATE_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Status,
    Clear { confirmed: bool },
    Forget,
    Restore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    root: PathBuf,
    command: Command,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("个人排序管理失败：{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run() -> Result<(), Box<dyn Error>> {
    Err("个人排序的当前用户加密目前只支持 Windows".into())
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    match options.command {
        Command::Status => status(&options.root),
        Command::Clear { confirmed } => clear(&options.root, confirmed),
        Command::Forget => change_suppression(
            &options.root,
            PersonalRankingSuppressionActionKind::Suppress,
        ),
        Command::Restore => {
            change_suppression(&options.root, PersonalRankingSuppressionActionKind::Restore)
        }
    }
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut arguments = arguments.into_iter();
    let command = arguments.next().ok_or_else(usage)?;
    let mut root = None;
    let mut confirmed = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" if root.is_none() => {
                root = Some(PathBuf::from(arguments.next().ok_or("--root 缺少值")?));
            }
            "--confirm-clear-personal-ranking" if !confirmed => confirmed = true,
            _ => return Err(format!("无法识别或重复的参数：{argument}\n{}", usage()).into()),
        }
    }
    let root = root.ok_or("缺少 --root")?;
    if root.as_os_str().is_empty() {
        return Err("--root 的值不能为空".into());
    }
    let command = match command.as_str() {
        "status" if !confirmed => Command::Status,
        "clear" => Command::Clear { confirmed },
        "forget" if !confirmed => Command::Forget,
        "restore" if !confirmed => Command::Restore,
        _ => return Err(usage().into()),
    };
    Ok(Options { root, command })
}

fn usage() -> String {
    "用法：\n  personalctl status --root <个人排序目录>\n  personalctl forget --root <个人排序目录>\n  personalctl restore --root <个人排序目录>\n  personalctl clear --root <个人排序目录> --confirm-clear-personal-ranking\n\nforget 与 restore 启动后再读取编码和候选文字，不接受私人命令行参数。".to_owned()
}

#[cfg(windows)]
fn status(root: &Path) -> Result<(), Box<dyn Error>> {
    let loaded = load_personal_ranking(root, &WindowsUserDataProtector)?;
    let suppressions = load_personal_ranking_suppressions(
        &suppression_root_for_ranking(root)?,
        &WindowsUserDataProtector,
    )?;
    println!("个人排序");
    println!("  加密批次：{}", loaded.batch_count());
    println!("  明确选择：{}", loaded.selection_count());
    println!("  排序条目：{}", loaded.snapshot().entry_count());
    println!("  忘记与恢复动作：{}", suppressions.action_count());
    println!("  当前忘记条目：{}", suppressions.snapshot().entry_count());
    println!("  内容：Windows 当前用户加密");
    Ok(())
}

#[cfg(windows)]
fn change_suppression(
    ranking_root: &Path,
    kind: PersonalRankingSuppressionActionKind,
) -> Result<(), Box<dyn Error>> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    change_suppression_with_io(ranking_root, kind, &mut input, &mut output, true)
}

#[cfg(windows)]
fn change_suppression_with_io(
    ranking_root: &Path,
    kind: PersonalRankingSuppressionActionKind,
    input: &mut impl Read,
    output: &mut impl Write,
    show_prompts: bool,
) -> Result<(), Box<dyn Error>> {
    let suppression_root = suppression_root_for_ranking(ranking_root)?;
    let (code, text) = read_private_identity(input, output, show_prompts)?;
    let loaded = load_personal_ranking_suppressions(&suppression_root, &WindowsUserDataProtector)?;
    let sequence = u64::try_from(loaded.action_count()).map_err(|_| "忘记动作计数过大")?;
    let action =
        PersonalRankingSuppressionAction::now(std::process::id(), sequence, kind, &code, &text)?;
    save_personal_ranking_suppression_action(
        &suppression_root,
        &action,
        &WindowsUserDataProtector,
    )?;
    match kind {
        PersonalRankingSuppressionActionKind::Suppress => {
            writeln!(output, "已忘记这条个人排序。")?;
        }
        PersonalRankingSuppressionActionKind::Restore => {
            writeln!(output, "已恢复这条个人排序。")?;
        }
    }
    writeln!(output, "输入法下次激活时会读取这项变更。")?;
    Ok(())
}

#[cfg(windows)]
fn suppression_root_for_ranking(ranking_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let parent = ranking_root.parent().ok_or("个人排序目录缺少父目录")?;
    Ok(parent.join(PERSONAL_RANKING_SUPPRESSION_DIRECTORY))
}

fn read_private_identity(
    input: &mut impl Read,
    output: &mut impl Write,
    show_prompts: bool,
) -> Result<(String, String), Box<dyn Error>> {
    if show_prompts {
        write!(output, "双拼编码：")?;
        output.flush()?;
    }
    let code = read_bounded_line(input, MAX_PRIVATE_CODE_BYTES)?;
    if show_prompts {
        write!(output, "候选文字：")?;
        output.flush()?;
    }
    let text = read_bounded_line(input, MAX_PRIVATE_TEXT_BYTES)?;
    Ok((code, text))
}

fn read_bounded_line(input: &mut impl Read, maximum: usize) -> Result<String, Box<dyn Error>> {
    let mut bytes = Vec::with_capacity(maximum.min(64));
    loop {
        let mut byte = [0_u8; 1];
        match input.read(&mut byte)? {
            0 if bytes.is_empty() => return Err("私人输入提前结束".into()),
            0 => break,
            _ if byte[0] == b'\n' => break,
            _ if bytes.len() == maximum => return Err("私人输入超过长度上限".into()),
            _ => bytes.push(byte[0]),
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|_| "私人输入不是有效 UTF-8".into())
}

fn clear(root: &Path, confirmed: bool) -> Result<(), Box<dyn Error>> {
    if !confirmed {
        return Err("clear 需要 --confirm-clear-personal-ranking".into());
    }
    let Some(archive) = archive_personal_ranking(root)? else {
        println!("当前没有个人排序记录。");
        return Ok(());
    };
    let archive_name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("个人排序归档");
    println!("个人排序已移入可恢复归档：{archive_name}");
    println!(
        "仍在使用旧版输入法的应用可能稍后重新生成记录。彻底清空前请先停用输入法或关闭相关应用。"
    );
    Ok(())
}

fn archive_personal_ranking(root: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("个人排序路径不是普通目录".into());
    }
    let parent = root.parent().ok_or("个人排序目录缺少父目录")?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err("个人排序父路径不是普通目录".into());
    }
    let leaf = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("个人排序目录名无效")?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    for attempt in 0..16_u32 {
        let archive = parent.join(format!("{leaf}.archive-{stamp}-{attempt}"));
        if archive.exists() {
            continue;
        }
        fs::rename(root, &archive)?;
        return Ok(Some(archive));
    }
    Err("无法分配个人排序归档名".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = env::temp_dir().join(format!(
                "ziranma-personalctl-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_status_and_confirmed_clear() {
        assert_eq!(
            parse_options(["status", "--root", "ranking"].map(str::to_owned)).unwrap(),
            Options {
                root: PathBuf::from("ranking"),
                command: Command::Status,
            }
        );
        assert_eq!(
            parse_options(
                [
                    "clear",
                    "--root",
                    "ranking",
                    "--confirm-clear-personal-ranking",
                ]
                .map(str::to_owned),
            )
            .unwrap(),
            Options {
                root: PathBuf::from("ranking"),
                command: Command::Clear { confirmed: true },
            }
        );
        assert_eq!(
            parse_options(["forget", "--root", "ranking"].map(str::to_owned)).unwrap(),
            Options {
                root: PathBuf::from("ranking"),
                command: Command::Forget,
            }
        );
        assert_eq!(
            parse_options(["restore", "--root", "ranking"].map(str::to_owned)).unwrap(),
            Options {
                root: PathBuf::from("ranking"),
                command: Command::Restore,
            }
        );
    }

    #[test]
    fn private_identity_is_bounded_and_absent_from_command_options() {
        let mut input = std::io::Cursor::new("qnqn\r\n亲亲\r\n".as_bytes());
        let mut output = Vec::new();
        assert_eq!(
            read_private_identity(&mut input, &mut output, false).unwrap(),
            ("qnqn".to_owned(), "亲亲".to_owned())
        );
        assert!(output.is_empty());

        let oversized = format!("{}\n甲\n", "a".repeat(MAX_PRIVATE_CODE_BYTES + 1));
        assert!(
            read_private_identity(
                &mut std::io::Cursor::new(oversized.as_bytes()),
                &mut Vec::new(),
                false,
            )
            .is_err()
        );
        let options = parse_options(["forget", "--root", "ranking"].map(str::to_owned)).unwrap();
        let debug = format!("{options:?}");
        assert!(!debug.contains("qnqn"));
        assert!(!debug.contains("亲亲"));
    }

    #[cfg(windows)]
    #[test]
    fn forget_and_restore_append_encrypted_actions_without_private_arguments() {
        let directory = TestDirectory::new();
        let ranking_root = directory.0.join("personal-ranking");
        let mut forget_input = std::io::Cursor::new("qnqn\n亲亲\n".as_bytes());
        let mut output = Vec::new();
        change_suppression_with_io(
            &ranking_root,
            PersonalRankingSuppressionActionKind::Suppress,
            &mut forget_input,
            &mut output,
            false,
        )
        .unwrap();
        let suppression_root = suppression_root_for_ranking(&ranking_root).unwrap();
        let forgotten =
            load_personal_ranking_suppressions(&suppression_root, &WindowsUserDataProtector)
                .unwrap();
        assert_eq!(forgotten.action_count(), 1);
        assert!(forgotten.snapshot().is_suppressed("qnqn", "亲亲"));

        let mut restore_input = std::io::Cursor::new("qnqn\n亲亲\n".as_bytes());
        change_suppression_with_io(
            &ranking_root,
            PersonalRankingSuppressionActionKind::Restore,
            &mut restore_input,
            &mut output,
            false,
        )
        .unwrap();
        let restored =
            load_personal_ranking_suppressions(&suppression_root, &WindowsUserDataProtector)
                .unwrap();
        assert_eq!(restored.action_count(), 2);
        assert!(!restored.snapshot().is_suppressed("qnqn", "亲亲"));
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("qnqn"));
        assert!(!output.contains("亲亲"));
    }

    #[test]
    fn clear_moves_the_store_to_a_recoverable_sibling() {
        let directory = TestDirectory::new();
        let root = directory.0.join("personal-ranking");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("sentinel"), b"kept").unwrap();

        let archive = archive_personal_ranking(&root).unwrap().unwrap();

        assert!(!root.exists());
        assert_eq!(fs::read(archive.join("sentinel")).unwrap(), b"kept");
    }
}
