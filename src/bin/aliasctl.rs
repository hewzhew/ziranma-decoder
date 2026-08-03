use std::env;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
use ziranma_core::WindowsUserDataProtector;
use ziranma_core::{
    DataProtector, EXPLICIT_ALIAS_PACKAGE_FILE, EXPLICIT_ALIAS_PACKAGES_DIRECTORY,
    EXPLICIT_ALIAS_SLOT_FILE, ExplicitAliasSlotState, ExplicitAliasSnapshot,
    MAX_EXPLICIT_ALIAS_PACKAGE_BYTES, explicit_alias_package_id, load_explicit_alias_package,
    load_explicit_alias_slot_state, protect_explicit_alias_snapshot,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Command {
    Status,
    List { candidate: bool, show_text: bool },
    Set { code: String, text: String },
    Remove { code: String },
    PinPrivateStdin,
    UnpinPrivateStdin,
    Promote,
    Rollback,
    Unstage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Options {
    root: PathBuf,
    command: Command,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("别名管理失败：{error}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn run() -> Result<(), Box<dyn Error>> {
    Err("显式别名的当前用户加密目前只支持 Windows".into())
}

#[cfg(windows)]
fn run() -> Result<(), Box<dyn Error>> {
    let options = parse_options(env::args().skip(1))?;
    let protector = WindowsUserDataProtector;
    match options.command {
        Command::Status => status(&options.root, &protector),
        Command::List {
            candidate,
            show_text,
        } => list(&options.root, &protector, candidate, show_text),
        Command::Set { code, text } => change(
            &options.root,
            &protector,
            |snapshot| {
                snapshot.set(&code, &text)?;
                Ok(ChangeResult::Changed)
            },
            ChangePublication::Stage,
        ),
        Command::Remove { code } => change(
            &options.root,
            &protector,
            |snapshot| {
                Ok(if snapshot.remove(&code)?.is_some() {
                    ChangeResult::Changed
                } else {
                    ChangeResult::Unchanged
                })
            },
            ChangePublication::Stage,
        ),
        Command::PinPrivateStdin => {
            let (code, text) = read_private_action(std::io::stdin(), true)?;
            let text = text.expect("pin private input always contains text");
            change(
                &options.root,
                &protector,
                |snapshot| {
                    snapshot.set(&code, &text)?;
                    Ok(ChangeResult::Changed)
                },
                ChangePublication::Apply,
            )
        }
        Command::UnpinPrivateStdin => {
            let (code, _) = read_private_action(std::io::stdin(), false)?;
            change(
                &options.root,
                &protector,
                |snapshot| {
                    Ok(if snapshot.remove(&code)?.is_some() {
                        ChangeResult::Changed
                    } else {
                        ChangeResult::Unchanged
                    })
                },
                ChangePublication::Apply,
            )
        }
        Command::Promote => promote(&options.root, &protector),
        Command::Rollback => rollback(&options.root, &protector),
        Command::Unstage => unstage(&options.root),
    }
}

fn parse_options(arguments: impl IntoIterator<Item = String>) -> Result<Options, Box<dyn Error>> {
    let mut arguments = arguments.into_iter();
    let command = arguments.next().ok_or_else(usage)?;
    let mut root = None;
    let mut code = None;
    let mut text = None;
    let mut candidate = false;
    let mut show_text = false;
    let mut private_stdin = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--root" => set_value(&mut root, arguments.next(), "--root")?,
            "--code" => set_value(&mut code, arguments.next(), "--code")?,
            "--text" => set_value(&mut text, arguments.next(), "--text")?,
            "--candidate" if !candidate => candidate = true,
            "--confirm-show-private-text" if !show_text => show_text = true,
            "--private-stdin" if !private_stdin => private_stdin = true,
            _ => return Err(format!("无法识别或重复的参数：{argument}\n{}", usage()).into()),
        }
    }
    let root = root.ok_or("缺少 --root")?;
    let command = match command.as_str() {
        "status"
            if code.is_none() && text.is_none() && !candidate && !show_text && !private_stdin =>
        {
            Command::Status
        }
        "list" if code.is_none() && text.is_none() && !private_stdin => Command::List {
            candidate,
            show_text,
        },
        "set" if !candidate && !show_text && !private_stdin => Command::Set {
            code: code.ok_or("set 缺少 --code")?,
            text: text.ok_or("set 缺少 --text")?,
        },
        "remove" if text.is_none() && !candidate && !show_text && !private_stdin => {
            Command::Remove {
                code: code.ok_or("remove 缺少 --code")?,
            }
        }
        "pin" if code.is_none() && text.is_none() && !candidate && !show_text && private_stdin => {
            Command::PinPrivateStdin
        }
        "unpin"
            if code.is_none() && text.is_none() && !candidate && !show_text && private_stdin =>
        {
            Command::UnpinPrivateStdin
        }
        "promote"
            if code.is_none() && text.is_none() && !candidate && !show_text && !private_stdin =>
        {
            Command::Promote
        }
        "rollback"
            if code.is_none() && text.is_none() && !candidate && !show_text && !private_stdin =>
        {
            Command::Rollback
        }
        "unstage"
            if code.is_none() && text.is_none() && !candidate && !show_text && !private_stdin =>
        {
            Command::Unstage
        }
        _ => return Err(usage().into()),
    };
    Ok(Options {
        root: PathBuf::from(root),
        command,
    })
}

fn set_value(
    destination: &mut Option<String>,
    value: Option<String>,
    option: &str,
) -> Result<(), Box<dyn Error>> {
    if destination.is_some() {
        return Err(format!("参数重复：{option}").into());
    }
    let value = value.ok_or_else(|| format!("{option} 缺少值"))?;
    if value.is_empty() {
        return Err(format!("{option} 的值不能为空").into());
    }
    *destination = Some(value);
    Ok(())
}

fn usage() -> String {
    "用法：\n  aliasctl status --root <目录>\n  aliasctl list --root <目录> --confirm-show-private-text [--candidate]\n  aliasctl set --root <目录> --code <小写字母码> --text <文字>\n  aliasctl remove --root <目录> --code <小写字母码>\n  aliasctl pin --root <目录> --private-stdin\n  aliasctl unpin --root <目录> --private-stdin\n  aliasctl promote --root <目录>\n  aliasctl rollback --root <目录>\n  aliasctl unstage --root <目录>"
        .to_owned()
}

const MAX_PRIVATE_ACTION_BYTES: usize = 1_024;

fn read_private_action(
    input: impl Read,
    expects_text: bool,
) -> Result<(String, Option<String>), Box<dyn Error>> {
    let mut bytes = Vec::new();
    input
        .take(u64::try_from(MAX_PRIVATE_ACTION_BYTES + 1).expect("small fixed input bound"))
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > MAX_PRIVATE_ACTION_BYTES {
        return Err("私密输入为空或超过上限".into());
    }
    let input = std::str::from_utf8(&bytes).map_err(|_| "私密输入不是有效 UTF-8")?;
    if input.contains(['\r', '\0']) || !input.ends_with('\n') {
        return Err("私密输入格式无效".into());
    }
    let fields = input[..input.len() - 1].split('\n').collect::<Vec<_>>();
    let valid_count = if expects_text { 2 } else { 1 };
    if fields.len() != valid_count || fields.iter().any(|field| field.is_empty()) {
        return Err("私密输入格式无效".into());
    }
    Ok((
        fields[0].to_owned(),
        expects_text.then(|| fields[1].to_owned()),
    ))
}

fn status(root: &Path, protector: &dyn DataProtector) -> Result<(), Box<dyn Error>> {
    let Some(state) = load_explicit_alias_slot_state(root)? else {
        println!("显式别名：尚未配置");
        println!("  加密：Windows 当前用户");
        println!("  网络：未连接");
        return Ok(());
    };
    println!(
        "显式别名：{}",
        if state.current().is_some() {
            "已启用"
        } else {
            "尚未配置"
        }
    );
    print_slot(root, protector, "当前", state.current())?;
    print_slot(root, protector, "待切换", state.candidate())?;
    print_slot(root, protector, "可回退", state.previous())?;
    println!("  加密：Windows 当前用户");
    println!("  网络：未连接");
    Ok(())
}

fn print_slot(
    root: &Path,
    protector: &dyn DataProtector,
    label: &str,
    package_id: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    match package_id {
        Some(package_id) => {
            let loaded = load_explicit_alias_package(root, package_id, protector)?;
            println!("  {label}：{} 条，校验通过", loaded.snapshot().len());
        }
        None => println!("  {label}：无"),
    }
    Ok(())
}

fn list(
    root: &Path,
    protector: &dyn DataProtector,
    candidate: bool,
    show_text: bool,
) -> Result<(), Box<dyn Error>> {
    if !show_text {
        return Err("list 会在当前终端显示私人别名；请显式加入 --confirm-show-private-text".into());
    }
    let state = load_explicit_alias_slot_state(root)?.ok_or("别名尚未配置")?;
    let package_id = if candidate {
        state.candidate().ok_or("没有待切换别名")?
    } else {
        state.current().ok_or("没有当前别名")?
    };
    let loaded = load_explicit_alias_package(root, package_id, protector)?;
    let label = if candidate { "待切换" } else { "当前" };
    println!("{label}显式别名 · {} 条", loaded.snapshot().len());
    for (code, text) in loaded.snapshot().iter() {
        println!("{code} → {text}");
    }
    println!("原文只显示在当前终端；未联网，未学习。");
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChangeResult {
    Changed,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChangePublication {
    Stage,
    Apply,
}

fn change(
    root: &Path,
    protector: &dyn DataProtector,
    mutate: impl FnOnce(&mut ExplicitAliasSnapshot) -> Result<ChangeResult, Box<dyn Error>>,
    publication: ChangePublication,
) -> Result<(), Box<dyn Error>> {
    let mut state = load_explicit_alias_slot_state(root)?.unwrap_or_default();
    let base_id = state.candidate().or_else(|| state.current());
    let mut snapshot = match base_id {
        Some(package_id) => load_explicit_alias_package(root, package_id, protector)?
            .snapshot()
            .as_ref()
            .clone(),
        None => ExplicitAliasSnapshot::default(),
    };
    let before = snapshot.clone();
    if mutate(&mut snapshot)? == ChangeResult::Unchanged || snapshot == before {
        println!("显式别名没有变化");
        return Ok(());
    }

    prepare_root(root)?;
    let protected = protect_explicit_alias_snapshot(&snapshot, protector)?;
    let package_id = install_package(root, &protected)?;
    let first = state.current().is_none();
    if first {
        state.adopt(&package_id)?;
    } else {
        state.stage(&package_id)?;
        if publication == ChangePublication::Apply {
            state.promote()?;
        }
    }
    write_slot_state(root, &state)?;
    if first {
        println!("显式别名已启用 · {} 条", snapshot.len());
        println!("新的输入组合会自动读取这一版本。");
    } else if publication == ChangePublication::Apply {
        println!("首选已固定 · {} 条", snapshot.len());
        println!("新的输入组合会自动读取这一版本；需要时可以回退。");
    } else {
        println!("显式别名已暂存 · {} 条", snapshot.len());
        println!("确认后运行 alias-ime.cmd promote；也可运行 unstage 放弃本次暂存。");
    }
    Ok(())
}

fn promote(root: &Path, protector: &dyn DataProtector) -> Result<(), Box<dyn Error>> {
    let mut state = load_explicit_alias_slot_state(root)?.ok_or("别名尚未配置")?;
    let package_id = state.candidate().ok_or("没有待切换别名")?.to_owned();
    let loaded = load_explicit_alias_package(root, &package_id, protector)?;
    let entries = loaded.snapshot().len();
    state.promote()?;
    write_slot_state(root, &state)?;
    println!("显式别名已切换 · {entries} 条");
    println!("新的输入组合会自动读取这一版本；无需重装输入法。");
    Ok(())
}

fn rollback(root: &Path, protector: &dyn DataProtector) -> Result<(), Box<dyn Error>> {
    let mut state = load_explicit_alias_slot_state(root)?.ok_or("别名尚未配置")?;
    let package_id = state.previous().ok_or("没有可回退别名")?.to_owned();
    let loaded = load_explicit_alias_package(root, &package_id, protector)?;
    let entries = loaded.snapshot().len();
    state.rollback()?;
    write_slot_state(root, &state)?;
    println!("显式别名已回退 · {entries} 条");
    println!("新的输入组合会自动读取这一版本；无需重装输入法。");
    Ok(())
}

fn unstage(root: &Path) -> Result<(), Box<dyn Error>> {
    let mut state = load_explicit_alias_slot_state(root)?.ok_or("别名尚未配置")?;
    state.unstage()?;
    write_slot_state(root, &state)?;
    println!("已放弃待切换别名；当前版本未改变。");
    Ok(())
}

fn prepare_root(root: &Path) -> Result<(), Box<dyn Error>> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        Ok(_) => return Err("别名数据目录必须是普通目录".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root)?;
            let metadata = fs::symlink_metadata(root)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("新建的别名数据路径不是普通目录".into());
            }
        }
        Err(_) => return Err("无法检查别名数据目录".into()),
    }
    let packages = root.join(EXPLICIT_ALIAS_PACKAGES_DIRECTORY);
    match fs::symlink_metadata(&packages) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err("别名包存储必须是普通目录".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&packages)?;
            Ok(())
        }
        Err(_) => Err("无法检查别名包存储".into()),
    }
}

fn install_package(root: &Path, protected: &[u8]) -> Result<String, Box<dyn Error>> {
    if protected.is_empty() || protected.len() > MAX_EXPLICIT_ALIAS_PACKAGE_BYTES {
        return Err("加密别名包大小无效".into());
    }
    let package_id = explicit_alias_package_id(protected);
    let directory = root
        .join(EXPLICIT_ALIAS_PACKAGES_DIRECTORY)
        .join(&package_id);
    match fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&directory)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("现有别名包路径不是普通目录".into());
            }
            let existing = read_regular_bytes(
                &directory.join(EXPLICIT_ALIAS_PACKAGE_FILE),
                MAX_EXPLICIT_ALIAS_PACKAGE_BYTES,
            )?;
            if existing != protected {
                return Err("现有别名包目录与内容标识冲突".into());
            }
            return Ok(package_id);
        }
        Err(error) => return Err(error.into()),
    }
    let path = directory.join(EXPLICIT_ALIAS_PACKAGE_FILE);
    let result = write_new_synced(&path, protected);
    if result.is_err() {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&directory);
    }
    result?;
    Ok(package_id)
}

fn write_slot_state(root: &Path, state: &ExplicitAliasSlotState) -> Result<(), Box<dyn Error>> {
    prepare_root(root)?;
    let body = state.render();
    ExplicitAliasSlotState::parse(&body)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let temporary = root.join(format!(".slots-{}-{stamp}.tmp", std::process::id()));
    write_new_synced(&temporary, body.as_bytes())?;
    let result = move_replace(&temporary, &root.join(EXPLICIT_ALIAS_SLOT_FILE));
    if result.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_regular_bytes(path: &Path, maximum: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > u64::try_from(maximum).unwrap_or(u64::MAX)
    {
        return Err("别名包文件无效".into());
    }
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() > maximum {
        return Err("别名包文件大小无效".into());
    }
    Ok(bytes)
}

fn write_new_synced(path: &Path, contents: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(windows)]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // SAFETY: both NUL-terminated path buffers remain alive for the call.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Result<Vec<u16>, Box<dyn Error>> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err("路径含有 NUL".into());
    }
    wide.push(0);
    Ok(wide)
}

#[cfg(not(windows))]
fn move_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn Error>> {
    fs::rename(source, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use ziranma_core::ContinuousCaptureError;

    struct TestProtector;

    impl DataProtector for TestProtector {
        fn protection_name(&self) -> &'static str {
            "test"
        }

        fn protect(&self, plaintext: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            Ok(plaintext.iter().map(|byte| byte ^ 0xa5).collect())
        }

        fn unprotect(&self, protected: &[u8]) -> Result<Vec<u8>, ContinuousCaptureError> {
            self.protect(protected)
        }
    }

    #[test]
    fn command_parser_requires_explicit_root_and_private_display_confirmation() {
        assert_eq!(
            parse_options(
                [
                    "set", "--root", "aliases", "--code", "wua", "--text", "呜哇",
                ]
                .map(str::to_owned),
            )
            .unwrap(),
            Options {
                root: PathBuf::from("aliases"),
                command: Command::Set {
                    code: "wua".to_owned(),
                    text: "呜哇".to_owned(),
                },
            }
        );
        assert!(
            parse_options(["list", "--root", "aliases"].map(str::to_owned)).is_ok(),
            "display confirmation is enforced by execution so the parser can render a focused error"
        );
        assert!(parse_options(["status"].map(str::to_owned)).is_err());
        assert!(
            parse_options(
                [
                    "remove", "--root", "aliases", "--code", "wua", "--text", "x"
                ]
                .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn first_change_creates_the_explicit_nested_root_and_valid_current_slot() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-aliasctl-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = parent.join("user-data").join("aliases");
        change(
            &root,
            &TestProtector,
            |snapshot| {
                snapshot.set("aa", "合成")?;
                Ok(ChangeResult::Changed)
            },
            ChangePublication::Stage,
        )
        .unwrap();
        let state = load_explicit_alias_slot_state(&root).unwrap().unwrap();
        let current = state.current().unwrap();
        assert_eq!(
            load_explicit_alias_package(&root, current, &TestProtector)
                .unwrap()
                .snapshot()
                .get("aa"),
            Some("合成")
        );
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn private_stdin_is_bounded_exact_and_never_needs_private_arguments() {
        assert_eq!(
            read_private_action(&b"qnq\n\xE4\xBA\xB2\xE4\xBA\xB2\n"[..], true).unwrap(),
            ("qnq".to_owned(), Some("亲亲".to_owned()))
        );
        assert_eq!(
            read_private_action(&b"qnq\n"[..], false).unwrap(),
            ("qnq".to_owned(), None)
        );
        assert!(read_private_action(&b"qnq\r\n"[..], false).is_err());
        assert!(read_private_action(&b"qnq\nextra\n"[..], false).is_err());
        assert!(read_private_action(&vec![b'a'; MAX_PRIVATE_ACTION_BYTES + 1][..], false).is_err());
    }

    #[test]
    fn applied_change_replaces_current_and_keeps_one_step_rollback() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = std::env::temp_dir().join(format!(
            "ziranma-aliasctl-apply-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = parent.join("aliases");
        for text in ["甲", "乙"] {
            change(
                &root,
                &TestProtector,
                |snapshot| {
                    snapshot.set("aa", text)?;
                    Ok(ChangeResult::Changed)
                },
                ChangePublication::Apply,
            )
            .unwrap();
        }
        let state = load_explicit_alias_slot_state(&root).unwrap().unwrap();
        assert!(state.candidate().is_none());
        assert!(state.previous().is_some());
        assert_eq!(
            load_explicit_alias_package(&root, state.current().unwrap(), &TestProtector)
                .unwrap()
                .snapshot()
                .get("aa"),
            Some("乙")
        );

        rollback(&root, &TestProtector).unwrap();
        let state = load_explicit_alias_slot_state(&root).unwrap().unwrap();
        assert_eq!(
            load_explicit_alias_package(&root, state.current().unwrap(), &TestProtector)
                .unwrap()
                .snapshot()
                .get("aa"),
            Some("甲")
        );
        fs::remove_dir_all(parent).unwrap();
    }
}
