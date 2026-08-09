//! Explicit inspection, registration, and removal of the Windows TSF alpha.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[cfg(windows)]
use windows::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W,
    Process32FirstW, Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};

const MAX_DLL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPORT_NAMES: usize = 4_096;
const MAX_EXPORT_NAME_BYTES: usize = 256;
const MAX_RECEIPT_BYTES: u64 = 4_096;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_DLL: u16 = 0x2000;
const PE32_PLUS_MAGIC: u16 = 0x020b;
const REQUIRED_COM_EXPORTS: [&str; 2] = ["DllCanUnloadNow", "DllGetClassObject"];
const REGISTRATION_EXPORTS: [&str; 2] = ["DllRegisterServer", "DllUnregisterServer"];
const CONFIRMATION_FLAG: &str = "--confirm-machine-wide-development-alpha";
const ENABLE_CONFIRMATION_FLAG: &str = "--confirm-enable-current-user-development-alpha";
const DISABLE_CONFIRMATION_FLAG: &str = "--confirm-disable-current-user-development-alpha";
const INSTALL_SCHEMA: &str = "ziranma-tsf-alpha-install-v1";
const INSTALL_ROOT: &str = ".local/tsf-alpha";
const RECEIPT_FILE: &str = "install-v1.txt";
const INSTALLED_DLL_FILE: &str = "ziranma_core.dll";
const PROFILE_DESCRIPTION: &str = "Ziranma Decoder Alpha";
const TSF_PROPAGATION_TIMEOUT: Duration = Duration::from_secs(5);
const TSF_PROPAGATION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const TSF_ENABLE_STABILITY_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Inspect { dll: PathBuf },
    HostCacheState { dll: PathBuf },
    RegisterMachine { dll: PathBuf },
    UnregisterMachine,
    EnableCurrentUser,
    DisableCurrentUser,
    CurrentUserState,
    VerifyCurrentUserEnabled { allow_active: bool },
}

#[derive(Debug, Eq, PartialEq)]
struct PeInspection {
    machine: u16,
    optional_magic: u16,
    is_dll: bool,
    exports: BTreeSet<String>,
    certificate_table_present: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ProfileStatus {
    text_service_registered: bool,
    registered: bool,
    enabled: bool,
    active: bool,
    keyboard_category: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ComRegistrationStatus {
    current_user_64: bool,
    local_machine_64: bool,
    current_user_32: bool,
    local_machine_32: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LoadedHostStatus {
    scan_available: bool,
    matching_version: u32,
    other_versions: u32,
}

#[derive(Debug, Eq, PartialEq)]
struct InstallReceipt {
    dll_sha256: String,
    relative_dll: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum RegistrationAction {
    RegisterCom,
    RegisterComRollbackIncomplete,
    RegisterTextService,
    RegisterProfile,
    RegisterCategory,
    VerifyRegistered,
    WriteReceipt,
    UnregisterCategory,
    UnregisterProfile,
    UnregisterTextService,
    UnregisterCom,
    UnregisterComRollbackIncomplete,
    VerifyUnregistered,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ProfileTransitionAction {
    Enable,
    VerifyEnabled,
    Disable,
    VerifyDisabled,
}

impl ProfileTransitionAction {
    fn failure_message(self) -> &'static str {
        match self {
            Self::Enable => "cannot enable the current-user TSF alpha profile",
            Self::VerifyEnabled => "the current-user TSF alpha profile did not verify as enabled",
            Self::Disable => "cannot disable the current-user TSF alpha profile",
            Self::VerifyDisabled => {
                "the current-user TSF alpha profile did not verify as disabled and inactive"
            }
        }
    }
}

impl RegistrationAction {
    fn failure_message(self) -> &'static str {
        match self {
            Self::RegisterCom => "cannot register the machine COM server",
            Self::RegisterComRollbackIncomplete => {
                "cannot register the machine COM server; its internal rollback is incomplete"
            }
            Self::RegisterTextService => "cannot register the TSF text-service identity",
            Self::RegisterProfile => "cannot register the disabled TSF language profile",
            Self::RegisterCategory => "cannot register the TSF keyboard category",
            Self::VerifyRegistered => "the completed TSF registration did not verify",
            Self::WriteReceipt => "cannot write the local TSF installation receipt",
            Self::UnregisterCategory => "cannot remove the TSF keyboard category",
            Self::UnregisterProfile => "cannot remove the TSF language profile",
            Self::UnregisterTextService => "cannot remove the TSF text-service identity",
            Self::UnregisterCom => "cannot remove the machine COM server",
            Self::UnregisterComRollbackIncomplete => {
                "cannot remove the machine COM server; its internal rollback is incomplete"
            }
            Self::VerifyUnregistered => "the completed TSF removal did not verify",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct RegistrationTransactionError {
    failed: RegistrationAction,
    rollback_failed: Vec<RegistrationAction>,
}

impl std::fmt::Display for RegistrationTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.failed.failure_message())?;
        if !self.rollback_failed.is_empty() {
            formatter.write_str("; rollback incomplete: ")?;
            for (index, action) in self.rollback_failed.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                formatter.write_str(action.failure_message())?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for RegistrationTransactionError {}

#[derive(Debug, Eq, PartialEq)]
struct ProfileTransitionError {
    failed: ProfileTransitionAction,
    recovery_failed: Vec<ProfileTransitionAction>,
}

impl std::fmt::Display for ProfileTransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.failed.failure_message())?;
        if self.recovery_failed.is_empty() {
            formatter.write_str("; the safe disabled state was restored")?;
        } else {
            formatter.write_str("; recovery to the disabled state is incomplete: ")?;
            for (index, action) in self.recovery_failed.iter().enumerate() {
                if index > 0 {
                    formatter.write_str(", ")?;
                }
                formatter.write_str(action.failure_message())?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ProfileTransitionError {}

trait RegistrationBackend {
    fn register_com(&mut self, dll: &Path) -> Result<(), RegistrationAction>;
    fn register_text_service(&mut self) -> Result<(), RegistrationAction>;
    fn register_profile(&mut self, dll: &Path) -> Result<(), RegistrationAction>;
    fn register_category(&mut self) -> Result<(), RegistrationAction>;
    fn verify_registered(&mut self, dll: &Path) -> Result<(), RegistrationAction>;
    fn unregister_category(&mut self) -> Result<(), RegistrationAction>;
    fn unregister_profile(&mut self) -> Result<(), RegistrationAction>;
    fn unregister_text_service(&mut self) -> Result<(), RegistrationAction>;
    fn unregister_com(&mut self, dll: &Path) -> Result<(), RegistrationAction>;
    fn verify_unregistered(&mut self) -> Result<(), RegistrationAction>;
}

trait ProfileToggleBackend {
    fn enable_profile(&mut self) -> Result<(), ProfileTransitionAction>;
    fn verify_profile_enabled(&mut self) -> Result<(), ProfileTransitionAction>;
    fn disable_profile(&mut self) -> Result<(), ProfileTransitionAction>;
    fn verify_profile_disabled(&mut self) -> Result<(), ProfileTransitionAction>;
}

#[derive(Clone, Copy)]
struct PeSection {
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_offset: u32,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("TSF 开发操作失败：{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Inspect { dll } => inspect(&dll)?,
        Options::HostCacheState { dll } => host_cache_state(&dll)?,
        Options::RegisterMachine { dll } => register_machine(&dll)?,
        Options::UnregisterMachine => unregister_machine()?,
        Options::EnableCurrentUser => enable_current_user()?,
        Options::DisableCurrentUser => disable_current_user()?,
        Options::CurrentUserState => current_user_state()?,
        Options::VerifyCurrentUserEnabled { allow_active } => {
            verify_current_user_enabled(allow_active)?
        }
    }
    Ok(())
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<Options, Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(Options::Help);
    };
    if command == "--help" || command == "-h" {
        if arguments.next().is_some() {
            return Err("--help must be used by itself".into());
        }
        return Ok(Options::Help);
    }
    match command.as_str() {
        "inspect" => Ok(Options::Inspect {
            dll: parse_dll_arguments(arguments, false)?,
        }),
        "host-cache-state" => Ok(Options::HostCacheState {
            dll: parse_dll_arguments(arguments, false)?,
        }),
        "register-machine" => Ok(Options::RegisterMachine {
            dll: parse_dll_arguments(arguments, true)?,
        }),
        "unregister-machine" => {
            let confirmation = arguments
                .next()
                .ok_or("unregister-machine requires --confirm-machine-wide-development-alpha")?;
            if confirmation != CONFIRMATION_FLAG || arguments.next().is_some() {
                return Err(
                    "unregister-machine accepts only --confirm-machine-wide-development-alpha"
                        .into(),
                );
            }
            Ok(Options::UnregisterMachine)
        }
        "enable-current-user" => {
            parse_confirmation_only(arguments, "enable-current-user", ENABLE_CONFIRMATION_FLAG)?;
            Ok(Options::EnableCurrentUser)
        }
        "disable-current-user" => {
            parse_confirmation_only(arguments, "disable-current-user", DISABLE_CONFIRMATION_FLAG)?;
            Ok(Options::DisableCurrentUser)
        }
        "current-user-state" => {
            if arguments.next().is_some() {
                return Err("current-user-state does not accept arguments".into());
            }
            Ok(Options::CurrentUserState)
        }
        "verify-current-user-enabled" => {
            let allow_active = match arguments.next().as_deref() {
                None => false,
                Some("--allow-active") if arguments.next().is_none() => true,
                Some(_) => {
                    return Err(
                        "verify-current-user-enabled accepts only optional --allow-active".into(),
                    );
                }
            };
            Ok(Options::VerifyCurrentUserEnabled { allow_active })
        }
        _ => Err("unknown tsf-devctl command; value was suppressed".into()),
    }
}

fn parse_confirmation_only(
    arguments: impl IntoIterator<Item = String>,
    command: &str,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = arguments.into_iter();
    let confirmation = arguments
        .next()
        .ok_or_else(|| format!("{command} requires {expected}"))?;
    if confirmation != expected || arguments.next().is_some() {
        return Err(format!("{command} accepts only {expected}").into());
    }
    Ok(())
}

fn parse_dll_arguments(
    arguments: impl IntoIterator<Item = String>,
    require_confirmation: bool,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut dll = None;
    let mut confirmed = false;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dll" => {
                if dll.is_some() {
                    return Err("--dll can be given only once".into());
                }
                dll = Some(PathBuf::from(
                    arguments.next().ok_or("--dll requires a path")?,
                ));
            }
            CONFIRMATION_FLAG if require_confirmation => {
                if confirmed {
                    return Err(
                        "--confirm-machine-wide-development-alpha can be given only once".into(),
                    );
                }
                confirmed = true;
            }
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err("unknown command argument; value was suppressed".into()),
        }
    }
    if require_confirmation && !confirmed {
        return Err("register-machine requires --confirm-machine-wide-development-alpha".into());
    }
    dll.ok_or_else(|| "command requires exactly one --dll path".into())
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --bin tsf-devctl -- inspect --dll \
         target/release/ziranma_core.dll"
    );
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- host-cache-state --dll \
         target/release/ziranma_core.dll"
    );
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- register-machine --dll \
         target/release/ziranma_core.dll --confirm-machine-wide-development-alpha"
    );
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- unregister-machine \
         --confirm-machine-wide-development-alpha"
    );
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- enable-current-user \
         --confirm-enable-current-user-development-alpha"
    );
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- disable-current-user \
         --confirm-disable-current-user-development-alpha"
    );
    eprintln!("       cargo run --release --bin tsf-devctl -- current-user-state");
    eprintln!(
        "       cargo run --release --bin tsf-devctl -- verify-current-user-enabled \
         [--allow-active]"
    );
    eprintln!(
        "Machine registration is 64-bit, requires elevation, and is disabled by default. \
         Current-user enable/disable never makes the alpha the default input method or requests \
         process/session-wide activation."
    );
}

fn inspect(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_explicit_dll(path)?;
    let image = inspect_pe(&bytes)?;
    validate_alpha_dll(&image)?;
    let registration = inspect_com_registration()?;
    let profile = inspect_system_profile()?;
    let loaded_hosts = inspect_loaded_hosts(&hex_sha256(&bytes));
    print!(
        "{}",
        render_report(path, &image, registration, profile, loaded_hosts)
    );
    Ok(())
}

fn host_cache_state(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_explicit_dll(path)?;
    validate_alpha_dll(&inspect_pe(&bytes)?)?;
    print!(
        "{}",
        render_host_cache_state(inspect_loaded_hosts(&hex_sha256(&bytes)))
    );
    Ok(())
}

fn register_machine(source: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_explicit_dll(source)?;
    validate_alpha_dll(&inspect_pe(&bytes)?)?;
    let install_root = checked_install_root(true)?;
    let receipt_path = install_root.join(RECEIPT_FILE);
    if receipt_path.try_exists()? {
        return Err("a TSF alpha installation receipt already exists".into());
    }
    wait_for_unregistered_state()?;

    let digest = hex_sha256(&bytes);
    let installed_dll = prepare_immutable_dll(&install_root, &digest, &bytes)?;
    let mut backend = create_registration_backend()?;
    register_transaction(&mut backend, &installed_dll)?;

    let receipt = InstallReceipt {
        relative_dll: relative_dll_path(&digest),
        dll_sha256: digest,
    };
    if write_install_receipt(&install_root, &receipt).is_err() {
        let rollback_failed = rollback_registered(&mut backend, &installed_dll);
        return Err(RegistrationTransactionError {
            failed: RegistrationAction::WriteReceipt,
            rollback_failed,
        }
        .into());
    }

    println!("TSF Alpha 已注册（本机，64 位）");
    println!("状态：未启用，未激活");
    println!("微软拼音与默认输入法：未改动");
    Ok(())
}

fn unregister_machine() -> Result<(), Box<dyn std::error::Error>> {
    let (install_root, installed_dll) = verified_installed_dll()?;
    let com = inspect_com_registration()?;
    let profile = inspect_system_profile()?;
    if com == ComRegistrationStatus::default()
        && !profile.text_service_registered
        && !profile.registered
        && !profile.keyboard_category
    {
        fs::remove_file(install_root.join(RECEIPT_FILE))
            .map_err(|_| "the stale local TSF installation receipt could not be deleted")?;
        println!("TSF Alpha 系统注册已不存在；本地安装记录已清理");
        return Ok(());
    }
    require_exact_registered_state(&installed_dll)?;

    let mut backend = create_registration_backend()?;
    unregister_transaction(&mut backend, &installed_dll)?;
    let receipt_path = install_root.join(RECEIPT_FILE);
    fs::remove_file(&receipt_path)
        .map_err(|_| "TSF registration was removed, but its local receipt could not be deleted")?;

    println!("TSF Alpha 已注销（本机）");
    println!("微软拼音与默认输入法：未改动");
    Ok(())
}

fn enable_current_user() -> Result<(), Box<dyn std::error::Error>> {
    let (_, installed_dll) = verified_installed_dll()?;
    let profile = require_exact_registered_layout(&installed_dll)?;
    if profile.active {
        return Err(
            "the TSF alpha profile is unexpectedly active; refusing to change it implicitly".into(),
        );
    }
    if profile.enabled {
        println!("TSF Alpha 已为当前用户启用");
        println!("状态：尚未激活；默认输入法未改动");
        return Ok(());
    }

    let mut backend = create_registration_backend()?;
    enable_profile_transaction(&mut backend)?;
    drop(backend);
    println!("TSF Alpha 已为当前用户启用");
    println!("状态：未设为默认；未请求进程或桌面范围激活");
    println!("微软拼音与默认输入法：未改动");
    Ok(())
}

fn disable_current_user() -> Result<(), Box<dyn std::error::Error>> {
    let (_, installed_dll) = verified_installed_dll()?;
    let profile = require_exact_registered_layout(&installed_dll)?;
    if !profile.enabled && !profile.active {
        println!("TSF Alpha 已处于当前用户禁用状态");
        println!("微软拼音与默认输入法：未改动");
        return Ok(());
    }

    let mut backend = create_registration_backend()?;
    disable_profile_transaction(&mut backend)?;
    println!("TSF Alpha 已为当前用户禁用");
    println!("状态：未激活；默认输入法未改动");
    Ok(())
}

fn current_user_state() -> Result<(), Box<dyn std::error::Error>> {
    let (_, installed_dll) = verified_installed_dll()?;
    let profile = require_exact_registered_layout(&installed_dll)?;
    println!(
        "TSF_CURRENT_USER_STATE schema=ziranma-tsf-current-user-state-v1 enabled={} active={} writes=false",
        profile.enabled, profile.active
    );
    Ok(())
}

fn verify_current_user_enabled(allow_active: bool) -> Result<(), Box<dyn std::error::Error>> {
    let (_, installed_dll) = verified_installed_dll()?;
    let profile = require_exact_registered_layout(&installed_dll)?;
    let matches_expected_state = if allow_active {
        profile_matches_persisted_state(profile, true)
    } else {
        profile_matches_toggle_state(profile, true)
    };
    if !matches_expected_state {
        return Err("the current-user TSF alpha enablement is not persistent".into());
    }

    // The COM inspection above has already released its apartment. Observe
    // only the persisted current-user value during the stability window so a
    // profile that Windows removes while tearing down its cache cannot pass.
    let started = Instant::now();
    loop {
        if current_user_profile_enable_state()? != Some(true) {
            return Err("the current-user TSF alpha enablement is not persistent".into());
        }
        if started.elapsed() >= TSF_ENABLE_STABILITY_WINDOW {
            break;
        }
        thread::sleep(TSF_PROPAGATION_POLL_INTERVAL);
    }
    println!("TSF Alpha 当前用户启用状态已验证");
    if profile.active {
        println!("状态：换代前已在使用，现有宿主仍可保持活动；默认输入法未改动");
    } else {
        println!("状态：尚未激活；默认输入法未改动");
    }
    Ok(())
}

fn verified_installed_dll() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let install_root = checked_install_root(false)?;
    let receipt = read_install_receipt(&install_root)?;
    let installed_dll = resolve_receipt_dll(&install_root, &receipt)?;
    let bytes = read_explicit_dll(&installed_dll)?;
    validate_alpha_dll(&inspect_pe(&bytes)?)?;
    if hex_sha256(&bytes) != receipt.dll_sha256 {
        return Err("the installed DLL no longer matches the installation receipt".into());
    }
    Ok((install_root, installed_dll))
}

fn register_transaction<B: RegistrationBackend + ?Sized>(
    backend: &mut B,
    dll: &Path,
) -> Result<(), RegistrationTransactionError> {
    backend
        .register_com(dll)
        .map_err(|failed| RegistrationTransactionError {
            failed,
            rollback_failed: Vec::new(),
        })?;
    if let Err(failed) = backend.register_text_service() {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([backend.unregister_com(dll)]),
        });
    }
    if let Err(failed) = backend.register_profile(dll) {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([
                backend.unregister_text_service(),
                backend.unregister_com(dll),
            ]),
        });
    }
    if let Err(failed) = backend.register_category() {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([
                backend.unregister_profile(),
                backend.unregister_text_service(),
                backend.unregister_com(dll),
            ]),
        });
    }
    if let Err(failed) = backend.verify_registered(dll) {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: rollback_registered(backend, dll),
        });
    }
    Ok(())
}

fn rollback_registered<B: RegistrationBackend + ?Sized>(
    backend: &mut B,
    dll: &Path,
) -> Vec<RegistrationAction> {
    collect_failures([
        backend.unregister_category(),
        backend.unregister_profile(),
        backend.unregister_text_service(),
        backend.unregister_com(dll),
    ])
}

fn unregister_transaction<B: RegistrationBackend + ?Sized>(
    backend: &mut B,
    dll: &Path,
) -> Result<(), RegistrationTransactionError> {
    backend
        .unregister_category()
        .map_err(|failed| RegistrationTransactionError {
            failed,
            rollback_failed: Vec::new(),
        })?;
    if let Err(failed) = backend.unregister_profile() {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([backend.register_category()]),
        });
    }
    if let Err(failed) = backend.unregister_text_service() {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([
                backend.register_profile(dll),
                backend.register_category(),
            ]),
        });
    }
    if let Err(failed) = backend.unregister_com(dll) {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([
                backend.register_text_service(),
                backend.register_profile(dll),
                backend.register_category(),
            ]),
        });
    }
    if let Err(failed) = backend.verify_unregistered() {
        return Err(RegistrationTransactionError {
            failed,
            rollback_failed: collect_failures([
                backend.register_com(dll),
                backend.register_text_service(),
                backend.register_profile(dll),
                backend.register_category(),
            ]),
        });
    }
    Ok(())
}

fn collect_failures<const N: usize>(
    results: [Result<(), RegistrationAction>; N],
) -> Vec<RegistrationAction> {
    results.into_iter().filter_map(Result::err).collect()
}

fn enable_profile_transaction<B: ProfileToggleBackend + ?Sized>(
    backend: &mut B,
) -> Result<(), ProfileTransitionError> {
    if let Err(failed) = backend.enable_profile() {
        return Err(ProfileTransitionError {
            failed,
            recovery_failed: recover_disabled_profile(backend),
        });
    }
    if let Err(failed) = backend.verify_profile_enabled() {
        return Err(ProfileTransitionError {
            failed,
            recovery_failed: recover_disabled_profile(backend),
        });
    }
    Ok(())
}

fn disable_profile_transaction<B: ProfileToggleBackend + ?Sized>(
    backend: &mut B,
) -> Result<(), ProfileTransitionError> {
    if let Err(failed) = backend.disable_profile() {
        return Err(ProfileTransitionError {
            failed,
            recovery_failed: recover_disabled_profile(backend),
        });
    }
    if let Err(failed) = backend.verify_profile_disabled() {
        return Err(ProfileTransitionError {
            failed,
            recovery_failed: recover_disabled_profile(backend),
        });
    }
    Ok(())
}

fn recover_disabled_profile<B: ProfileToggleBackend + ?Sized>(
    backend: &mut B,
) -> Vec<ProfileTransitionAction> {
    let disable_failure = backend.disable_profile().err();
    match backend.verify_profile_disabled() {
        Ok(()) => Vec::new(),
        Err(verify_failure) => disable_failure
            .into_iter()
            .chain([verify_failure])
            .collect(),
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn relative_dll_path(digest: &str) -> PathBuf {
    PathBuf::from("builds")
        .join(digest)
        .join(INSTALLED_DLL_FILE)
}

fn checked_install_root(create: bool) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let workspace = fs::canonicalize(std::env::current_dir()?)?;
    let local = std::path::absolute(".local")?;
    if local.try_exists()? {
        require_local_directory(&local, &workspace)?;
    } else if !create {
        return Err("the local TSF installation directory was not found".into());
    } else {
        fs::create_dir(&local)?;
        require_local_directory(&local, &workspace)?;
    }
    let local_canonical = fs::canonicalize(&local)?;
    let install_root = std::path::absolute(INSTALL_ROOT)?;
    if install_root.try_exists()? {
        require_local_directory(&install_root, &local_canonical)?;
    }
    Ok(install_root)
}

fn require_local_directory(
    path: &Path,
    canonical_parent: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("the local TSF installation path is not a regular directory".into());
    }
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with(canonical_parent) {
        return Err("the local TSF installation path escapes the workspace".into());
    }
    Ok(())
}

fn prepare_immutable_dll(
    install_root: &Path,
    digest: &str,
    bytes: &[u8],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let build_dir = install_root.join("builds").join(digest);
    fs::create_dir_all(&build_dir)?;
    let root_canonical = fs::canonicalize(install_root)?;
    require_local_directory(&install_root.join("builds"), &root_canonical)?;
    let builds_canonical = fs::canonicalize(install_root.join("builds"))?;
    require_local_directory(&build_dir, &builds_canonical)?;
    let destination = build_dir.join(INSTALLED_DLL_FILE);
    if destination.try_exists()? {
        let existing = read_explicit_dll(&destination)?;
        if existing != bytes {
            return Err("the immutable TSF build path contains different bytes".into());
        }
        return Ok(destination);
    }

    let temporary = build_dir.join(unique_temporary_name(INSTALLED_DLL_FILE));
    let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    let installed = read_explicit_dll(&destination)?;
    if hex_sha256(&installed) != digest {
        return Err("the immutable TSF DLL failed its post-copy digest check".into());
    }
    Ok(destination)
}

fn write_install_receipt(
    install_root: &Path,
    receipt: &InstallReceipt,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(install_root)?;
    let path = install_root.join(RECEIPT_FILE);
    let temporary = install_root.join(unique_temporary_name(RECEIPT_FILE));
    let text = render_install_receipt(receipt);
    let write_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn render_install_receipt(receipt: &InstallReceipt) -> String {
    format!(
        "schema={INSTALL_SCHEMA}\nscope=local-machine\narchitecture=x86-64\n\
         dll_sha256={}\nrelative_dll={}\nprofile_enabled_by_default=false\n",
        receipt.dll_sha256,
        receipt.relative_dll.to_string_lossy().replace('\\', "/")
    )
}

fn read_install_receipt(install_root: &Path) -> Result<InstallReceipt, Box<dyn std::error::Error>> {
    let path = install_root.join(RECEIPT_FILE);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "the TSF alpha installation receipt was not found")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err("the TSF alpha installation receipt is not a bounded regular file".into());
    }
    let bytes = fs::read(&path)?;
    if bytes.is_empty() || u64::try_from(bytes.len())? > MAX_RECEIPT_BYTES {
        return Err("the TSF alpha installation receipt changed to an invalid size".into());
    }
    parse_install_receipt(std::str::from_utf8(&bytes)?)
}

fn parse_install_receipt(text: &str) -> Result<InstallReceipt, Box<dyn std::error::Error>> {
    let lines = text.lines().collect::<Vec<_>>();
    if lines.len() != 6
        || lines[0] != format!("schema={INSTALL_SCHEMA}")
        || lines[1] != "scope=local-machine"
        || lines[2] != "architecture=x86-64"
        || lines[5] != "profile_enabled_by_default=false"
    {
        return Err("the TSF alpha installation receipt has an unsupported format".into());
    }
    let digest = lines[3]
        .strip_prefix("dll_sha256=")
        .ok_or("the TSF alpha installation receipt is missing its digest")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("the TSF alpha installation receipt has an invalid digest".into());
    }
    let relative = lines[4]
        .strip_prefix("relative_dll=")
        .ok_or("the TSF alpha installation receipt is missing its DLL location")?;
    let expected = format!("builds/{digest}/{INSTALLED_DLL_FILE}");
    if relative != expected {
        return Err("the TSF alpha installation receipt has an invalid DLL location".into());
    }
    Ok(InstallReceipt {
        dll_sha256: digest.to_owned(),
        relative_dll: relative_dll_path(digest),
    })
}

fn resolve_receipt_dll(
    install_root: &Path,
    receipt: &InstallReceipt,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if receipt.relative_dll != relative_dll_path(&receipt.dll_sha256) {
        return Err("the TSF alpha installation receipt is internally inconsistent".into());
    }
    let root_canonical = fs::canonicalize(install_root)?;
    let builds = install_root.join("builds");
    require_local_directory(&builds, &root_canonical)?;
    let builds_canonical = fs::canonicalize(&builds)?;
    require_local_directory(
        &install_root.join("builds").join(&receipt.dll_sha256),
        &builds_canonical,
    )?;
    Ok(install_root.join(&receipt.relative_dll))
}

fn unique_temporary_name(base: &str) -> String {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!(".{base}.tmp-{}-{nonce}", std::process::id())
}

fn read_explicit_dll(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err("DLL path cannot be a symbolic link".into());
    }
    if !metadata.is_file() {
        return Err("DLL path must name a regular file".into());
    }
    if metadata.len() == 0 || metadata.len() > MAX_DLL_BYTES {
        return Err(format!("DLL size must be between 1 and {MAX_DLL_BYTES} bytes").into());
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_DLL_BYTES {
        return Err("DLL changed to an invalid size while it was being read".into());
    }
    Ok(bytes)
}

fn inspect_pe(bytes: &[u8]) -> Result<PeInspection, Box<dyn std::error::Error>> {
    if bytes.get(0..2) != Some(b"MZ") {
        return Err("DLL does not start with an MZ header".into());
    }
    let pe_offset = usize::try_from(read_u32(bytes, 0x3c)?).map_err(|_| "PE offset overflow")?;
    if bytes.get(pe_offset..pe_offset.saturating_add(4)) != Some(b"PE\0\0") {
        return Err("DLL does not contain a valid PE signature".into());
    }

    let coff = checked_add(pe_offset, 4)?;
    let machine = read_u16(bytes, coff)?;
    let section_count = usize::from(read_u16(bytes, checked_add(coff, 2)?)?);
    let optional_size = usize::from(read_u16(bytes, checked_add(coff, 16)?)?);
    let characteristics = read_u16(bytes, checked_add(coff, 18)?)?;
    let optional = checked_add(coff, 20)?;
    require_range(bytes, optional, optional_size)?;
    let optional_magic = read_u16(bytes, optional)?;
    let (directory_count_offset, directories_offset) = match optional_magic {
        PE32_PLUS_MAGIC => (108, 112),
        0x010b => (92, 96),
        _ => return Err("DLL uses an unsupported PE optional-header format".into()),
    };
    if optional_size < directories_offset {
        return Err("PE optional header is too short for its data directories".into());
    }
    let directory_count = usize::try_from(read_u32(
        bytes,
        checked_add(optional, directory_count_offset)?,
    )?)
    .map_err(|_| "PE data-directory count overflow")?;

    let section_table = checked_add(optional, optional_size)?;
    let section_bytes = section_count
        .checked_mul(40)
        .ok_or("PE section-table size overflow")?;
    require_range(bytes, section_table, section_bytes)?;
    let mut sections = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let offset = checked_add(
            section_table,
            index.checked_mul(40).ok_or("PE section offset overflow")?,
        )?;
        sections.push(PeSection {
            virtual_size: read_u32(bytes, checked_add(offset, 8)?)?,
            virtual_address: read_u32(bytes, checked_add(offset, 12)?)?,
            raw_size: read_u32(bytes, checked_add(offset, 16)?)?,
            raw_offset: read_u32(bytes, checked_add(offset, 20)?)?,
        });
    }

    let export_directory = read_data_directory(
        bytes,
        optional,
        optional_size,
        directories_offset,
        directory_count,
        0,
    )?;
    let certificate_directory = read_data_directory(
        bytes,
        optional,
        optional_size,
        directories_offset,
        directory_count,
        4,
    )?;
    if (certificate_directory.0 == 0) != (certificate_directory.1 == 0) {
        return Err("PE certificate directory has only an offset or a size".into());
    }
    if certificate_directory.1 > 0 {
        let certificate_offset = usize::try_from(certificate_directory.0)
            .map_err(|_| "certificate-table offset overflow")?;
        let certificate_size = usize::try_from(certificate_directory.1)
            .map_err(|_| "certificate-table size overflow")?;
        require_range(bytes, certificate_offset, certificate_size)?;
    }

    let exports = read_export_names(bytes, &sections, export_directory)?;
    Ok(PeInspection {
        machine,
        optional_magic,
        is_dll: characteristics & IMAGE_FILE_DLL != 0,
        exports,
        certificate_table_present: certificate_directory.1 > 0,
    })
}

fn read_data_directory(
    bytes: &[u8],
    optional: usize,
    optional_size: usize,
    directories_offset: usize,
    directory_count: usize,
    index: usize,
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    if index >= directory_count {
        return Ok((0, 0));
    }
    let relative = directories_offset
        .checked_add(
            index
                .checked_mul(8)
                .ok_or("data-directory index overflow")?,
        )
        .ok_or("data-directory offset overflow")?;
    if relative
        .checked_add(8)
        .ok_or("data-directory end overflow")?
        > optional_size
    {
        return Err("PE data-directory count exceeds the optional header".into());
    }
    let offset = checked_add(optional, relative)?;
    Ok((
        read_u32(bytes, offset)?,
        read_u32(bytes, checked_add(offset, 4)?)?,
    ))
}

fn read_export_names(
    bytes: &[u8],
    sections: &[PeSection],
    directory: (u32, u32),
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let (directory_rva, directory_size) = directory;
    if directory_rva == 0 && directory_size == 0 {
        return Ok(BTreeSet::new());
    }
    if directory_rva == 0 || directory_size < 40 {
        return Err("PE export directory is incomplete".into());
    }
    let directory_offset = rva_to_offset(directory_rva, sections, bytes.len())?;
    require_range(bytes, directory_offset, 40)?;
    let name_count = usize::try_from(read_u32(bytes, checked_add(directory_offset, 24)?)?)
        .map_err(|_| "export-name count overflow")?;
    if name_count > MAX_EXPORT_NAMES {
        return Err(format!("DLL has more than {MAX_EXPORT_NAMES} named exports").into());
    }
    if name_count == 0 {
        return Ok(BTreeSet::new());
    }
    let names_rva = read_u32(bytes, checked_add(directory_offset, 32)?)?;
    let names_offset = rva_to_offset(names_rva, sections, bytes.len())?;
    require_range(
        bytes,
        names_offset,
        name_count
            .checked_mul(4)
            .ok_or("export-name table overflow")?,
    )?;

    let mut exports = BTreeSet::new();
    for index in 0..name_count {
        let entry = checked_add(
            names_offset,
            index.checked_mul(4).ok_or("export-name entry overflow")?,
        )?;
        let name_rva = read_u32(bytes, entry)?;
        let name_offset = rva_to_offset(name_rva, sections, bytes.len())?;
        let name = read_ascii_name(bytes, name_offset)?;
        if !exports.insert(name) {
            return Err("DLL export table contains a duplicate name".into());
        }
    }
    Ok(exports)
}

fn rva_to_offset(
    rva: u32,
    sections: &[PeSection],
    file_len: usize,
) -> Result<usize, Box<dyn std::error::Error>> {
    for section in sections {
        let span = section.virtual_size.max(section.raw_size);
        let Some(delta) = rva.checked_sub(section.virtual_address) else {
            continue;
        };
        if delta >= span {
            continue;
        }
        if delta >= section.raw_size {
            return Err("PE RVA points into virtual data without file bytes".into());
        }
        let offset = section
            .raw_offset
            .checked_add(delta)
            .ok_or("PE RVA file offset overflow")?;
        let offset = usize::try_from(offset).map_err(|_| "PE RVA does not fit this process")?;
        if offset >= file_len {
            return Err("PE RVA points outside the DLL".into());
        }
        return Ok(offset);
    }
    Err("PE RVA is not covered by a file-backed section".into())
}

fn read_ascii_name(bytes: &[u8], offset: usize) -> Result<String, Box<dyn std::error::Error>> {
    let end = offset
        .saturating_add(MAX_EXPORT_NAME_BYTES + 1)
        .min(bytes.len());
    let available = bytes
        .get(offset..end)
        .ok_or("export name starts outside the DLL")?;
    let nul = available
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("export name is missing a bounded terminator")?;
    if nul == 0 || !available[..nul].iter().all(u8::is_ascii_graphic) {
        return Err("export name is empty or contains non-ASCII bytes".into());
    }
    Ok(std::str::from_utf8(&available[..nul])?.to_owned())
}

fn validate_alpha_dll(image: &PeInspection) -> Result<(), Box<dyn std::error::Error>> {
    if image.machine != IMAGE_FILE_MACHINE_AMD64 {
        return Err("TSF alpha DLL must target x86-64".into());
    }
    if image.optional_magic != PE32_PLUS_MAGIC {
        return Err("TSF alpha DLL must use the PE32+ format".into());
    }
    if !image.is_dll {
        return Err("PE image is not marked as a DLL".into());
    }
    for required in REQUIRED_COM_EXPORTS {
        if !image.exports.contains(required) {
            return Err(format!("TSF alpha DLL is missing {required}").into());
        }
    }
    for forbidden in REGISTRATION_EXPORTS {
        if image.exports.contains(forbidden) {
            return Err(format!("build-only TSF alpha unexpectedly exports {forbidden}").into());
        }
    }
    Ok(())
}

fn immutable_alpha_digest(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if !file_name.eq_ignore_ascii_case(INSTALLED_DLL_FILE) {
        return None;
    }
    let digest = path.parent()?.file_name()?.to_str()?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if !path
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("builds")
    {
        return None;
    }
    if !path
        .parent()?
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case("tsf-alpha")
    {
        return None;
    }
    Some(digest.to_ascii_lowercase())
}

#[cfg(windows)]
struct ToolhelpSnapshot(HANDLE);

#[cfg(windows)]
impl Drop for ToolhelpSnapshot {
    fn drop(&mut self) {
        // SAFETY: this handle came from CreateToolhelp32Snapshot and remains
        // owned by this guard.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[cfg(windows)]
fn loaded_alpha_digest(process_id: u32) -> Option<String> {
    // SAFETY: the snapshot is read-only and scoped to one enumerated process.
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, process_id) }
            .ok()?;
    let snapshot = ToolhelpSnapshot(snapshot);
    let mut module = MODULEENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<MODULEENTRY32W>()).ok()?,
        ..MODULEENTRY32W::default()
    };
    // SAFETY: module has the documented size and stays writable throughout
    // enumeration.
    if unsafe { Module32FirstW(snapshot.0, &mut module) }.is_err() {
        return None;
    }
    loop {
        let path_length = module
            .szExePath
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(module.szExePath.len());
        let path = PathBuf::from(String::from_utf16_lossy(&module.szExePath[..path_length]));
        if let Some(digest) = immutable_alpha_digest(&path) {
            return Some(digest);
        }
        // SAFETY: reuses the initialized writable structure for the next
        // module. Any error ends this process's bounded enumeration.
        if unsafe { Module32NextW(snapshot.0, &mut module) }.is_err() {
            return None;
        }
    }
}

#[cfg(windows)]
fn inspect_loaded_hosts(expected_digest: &str) -> LoadedHostStatus {
    // SAFETY: this creates a read-only point-in-time process snapshot.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return LoadedHostStatus::default();
    };
    let snapshot = ToolhelpSnapshot(snapshot);
    let mut process = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..PROCESSENTRY32W::default()
    };
    // SAFETY: process has the documented size and remains writable during
    // enumeration.
    if unsafe { Process32FirstW(snapshot.0, &mut process) }.is_err() {
        return LoadedHostStatus::default();
    }

    let mut status = LoadedHostStatus {
        scan_available: true,
        ..LoadedHostStatus::default()
    };
    loop {
        if let Some(digest) = loaded_alpha_digest(process.th32ProcessID) {
            if digest == expected_digest {
                status.matching_version = status.matching_version.saturating_add(1);
            } else {
                status.other_versions = status.other_versions.saturating_add(1);
            }
        }
        // SAFETY: reuses the initialized writable structure for the next
        // process. ERROR_NO_MORE_FILES and process churn both end the scan.
        if unsafe { Process32NextW(snapshot.0, &mut process) }.is_err() {
            break;
        }
    }
    status
}

#[cfg(not(windows))]
fn inspect_loaded_hosts(_expected_digest: &str) -> LoadedHostStatus {
    LoadedHostStatus::default()
}

fn render_report(
    path: &Path,
    image: &PeInspection,
    registration: ComRegistrationStatus,
    profile: ProfileStatus,
    loaded_hosts: LoadedHostStatus,
) -> String {
    let mut output = String::new();
    writeln!(output, "TSF 开发检查").unwrap();
    writeln!(output, "DLL：{}", path.display()).unwrap();
    writeln!(output, "格式：x86-64 · PE32+ · DLL").unwrap();
    writeln!(output, "COM 入口：DllGetClassObject、DllCanUnloadNow").unwrap();
    writeln!(output, "注册入口：无").unwrap();
    writeln!(
        output,
        "证书目录：{}",
        if image.certificate_table_present {
            "存在（未验证签名有效性）"
        } else {
            "无"
        }
    )
    .unwrap();
    writeln!(
        output,
        "COM 注册：{}",
        render_com_registration(registration)
    )
    .unwrap();
    writeln!(
        output,
        "文本服务身份：{}",
        if profile.text_service_registered {
            "已发现"
        } else {
            "未发现"
        }
    )
    .unwrap();
    if profile.registered {
        writeln!(
            output,
            "系统语言配置：已发现（{}；{}；{}）",
            if profile.enabled {
                "已启用"
            } else {
                "未启用"
            },
            if profile.active {
                "当前活动"
            } else {
                "当前未活动"
            },
            if profile.keyboard_category {
                "键盘类别"
            } else {
                "非键盘类别"
            }
        )
        .unwrap();
    } else {
        writeln!(output, "系统语言配置：未发现").unwrap();
    }
    writeln!(
        output,
        "键盘类别：{}",
        if profile.keyboard_category {
            "已发现"
        } else {
            "未发现"
        }
    )
    .unwrap();
    writeln!(output, "宿主缓存：{}", render_loaded_hosts(loaded_hosts)).unwrap();
    writeln!(output, "本次操作：只读").unwrap();
    output
}

fn render_loaded_hosts(status: LoadedHostStatus) -> String {
    if !status.scan_available {
        return "无法检查".to_owned();
    }
    if status.matching_version == 0 && status.other_versions == 0 {
        return "未发现正在加载 Alpha 的应用".to_owned();
    }
    format!(
        "此版本 {}，其他版本 {}（仅计可见进程）",
        status.matching_version, status.other_versions
    )
}

fn render_host_cache_state(status: LoadedHostStatus) -> String {
    format!(
        "TSF_HOST_CACHE_STATE schema=ziranma-tsf-host-cache-state-v1 \
         scan_available={} matching_version={} other_versions={} writes=false\n",
        status.scan_available, status.matching_version, status.other_versions
    )
}

fn render_com_registration(status: ComRegistrationStatus) -> String {
    let mut locations = Vec::new();
    if status.current_user_64 {
        locations.push("当前用户 64 位");
    }
    if status.local_machine_64 {
        locations.push("本机 64 位");
    }
    if status.current_user_32 {
        locations.push("当前用户 32 位");
    }
    if status.local_machine_32 {
        locations.push("本机 32 位");
    }
    if locations.is_empty() {
        "未发现".to_owned()
    } else {
        format!("已发现（{}）", locations.join("、"))
    }
}

#[cfg(windows)]
fn inspect_com_registration() -> Result<ComRegistrationStatus, Box<dyn std::error::Error>> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    };
    use windows::core::PCWSTR;

    let mut subkey = alpha_inproc_server_registry_path()
        .encode_utf16()
        .collect::<Vec<_>>();
    subkey.push(0);
    let subkey = PCWSTR(subkey.as_ptr());
    Ok(ComRegistrationStatus {
        current_user_64: registration_key_exists(HKEY_CURRENT_USER, subkey, KEY_WOW64_64KEY)?,
        local_machine_64: registration_key_exists(HKEY_LOCAL_MACHINE, subkey, KEY_WOW64_64KEY)?,
        current_user_32: registration_key_exists(HKEY_CURRENT_USER, subkey, KEY_WOW64_32KEY)?,
        local_machine_32: registration_key_exists(HKEY_LOCAL_MACHINE, subkey, KEY_WOW64_32KEY)?,
    })
}

#[cfg(windows)]
fn alpha_inproc_server_registry_path() -> String {
    use ziranma_core::TSF_ALPHA_CLSID;

    format!(
        "Software\\Classes\\CLSID\\{}\\InprocServer32",
        registry_guid(TSF_ALPHA_CLSID)
    )
}

#[cfg(windows)]
fn alpha_language_profile_registry_path() -> String {
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

    format!(
        "Software\\Microsoft\\CTF\\TIP\\{}\\LanguageProfile\\0x{TSF_ALPHA_LANGID:08X}\\{}",
        registry_guid(TSF_ALPHA_CLSID),
        registry_guid(TSF_ALPHA_PROFILE_GUID)
    )
}

#[cfg(windows)]
fn registry_guid(guid: windows::core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    )
}

#[cfg(windows)]
fn machine_profile_key_exists() -> Result<bool, Box<dyn std::error::Error>> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};
    use windows::core::PCWSTR;

    let subkey = wide_nul(&alpha_language_profile_registry_path());
    registration_key_exists(HKEY_LOCAL_MACHINE, PCWSTR(subkey.as_ptr()), KEY_WOW64_64KEY)
}

#[cfg(windows)]
fn current_user_profile_enable_state() -> Result<Option<bool>, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_64KEY, RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    let profile_path = wide_nul(&alpha_language_profile_registry_path());
    let mut profile = HKEY::default();
    // SAFETY: the fixed path is NUL-terminated and the output is writable.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(profile_path.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut profile,
        )
    };
    if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    if result != ERROR_SUCCESS {
        return Err("cannot inspect the fixed current-user language profile key".into());
    }
    let profile = OpenedRegistryKey(profile);
    let (subkeys, values) = registry_key_counts(profile.0)?;
    if subkeys != 0 || values != 1 {
        return Err("the fixed current-user language profile key has an unexpected shape".into());
    }
    match read_registry_dword(profile.0, "Enable")? {
        Some(0) => Ok(Some(false)),
        Some(1) => Ok(Some(true)),
        _ => Err("the fixed current-user profile enable value is invalid".into()),
    }
}

#[cfg(windows)]
fn registration_key_exists(
    root: windows::Win32::System::Registry::HKEY,
    subkey: windows::core::PCWSTR,
    view: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<bool, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{HKEY, KEY_READ, RegCloseKey, RegOpenKeyExW};

    struct OpenedKey(HKEY);
    impl Drop for OpenedKey {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by a successful RegOpenKeyExW
            // call and is owned by this guard.
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    let mut opened = HKEY::default();
    // SAFETY: the subkey is NUL-terminated and remains live for this
    // synchronous read-only call; the output handle points to writable storage.
    let result = unsafe { RegOpenKeyExW(root, subkey, None, KEY_READ | view, &mut opened) };
    if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
        return Ok(false);
    }
    if result != ERROR_SUCCESS {
        return Err("cannot inspect the fixed COM registration location".into());
    }
    let _opened = OpenedKey(opened);
    Ok(true)
}

#[cfg(not(windows))]
fn inspect_com_registration() -> Result<ComRegistrationStatus, Box<dyn std::error::Error>> {
    Err("COM registration inspection requires Windows".into())
}

#[cfg(windows)]
fn alpha_profile_status(
    profile: &windows::Win32::UI::TextServices::TF_INPUTPROCESSORPROFILE,
) -> Option<ProfileStatus> {
    use windows::Win32::UI::TextServices::{
        TF_IPP_FLAG_ACTIVE, TF_IPP_FLAG_ENABLED, TF_PROFILETYPE_INPUTPROCESSOR,
    };
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

    (profile.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
        && profile.langid == TSF_ALPHA_LANGID
        && profile.clsid == TSF_ALPHA_CLSID
        && profile.guidProfile == TSF_ALPHA_PROFILE_GUID)
        .then_some(ProfileStatus {
            text_service_registered: false,
            registered: true,
            enabled: profile.dwFlags & TF_IPP_FLAG_ENABLED != 0,
            active: profile.dwFlags & TF_IPP_FLAG_ACTIVE != 0,
            keyboard_category: false,
        })
}

#[cfg(windows)]
fn alpha_language_profile_matches(
    profile: &windows::Win32::UI::TextServices::TF_LANGUAGEPROFILE,
) -> bool {
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

    profile.langid == TSF_ALPHA_LANGID
        && profile.clsid == TSF_ALPHA_CLSID
        && profile.guidProfile == TSF_ALPHA_PROFILE_GUID
}

#[cfg(windows)]
fn inspect_system_profile() -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::TextServices::{
        CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
        ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
    };
    use windows::core::IUnknown;

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: balances the successful CoInitializeEx below on this
            // standalone command's main thread.
            unsafe { CoUninitialize() };
        }
    }

    // SAFETY: this standalone command owns its main thread until the matching
    // guard is dropped and performs no nested COM initialization.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let _apartment = Apartment;
    // SAFETY: requests the legacy TSF service enumeration interface. It is
    // needed to expose service-only residue separately from language profiles.
    let text_services: ITfInputProcessorProfiles = unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: requests the documented TSF profile-manager interface from the
    // system in-process server; no mutating method is called below.
    let manager: ITfInputProcessorProfileMgr = unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: requests the documented TSF category-manager interface from the
    // system in-process server; only bounded enumeration is used below.
    let categories: ITfCategoryMgr = unsafe {
        CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    inspect_profile_with_managers(&text_services, &manager, &categories)
}

#[cfg(windows)]
fn inspect_profile_with_managers(
    text_services: &windows::Win32::UI::TextServices::ITfInputProcessorProfiles,
    manager: &windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr,
    categories: &windows::Win32::UI::TextServices::ITfCategoryMgr,
) -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    use windows::Win32::UI::TextServices::{
        GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE, TF_LANGUAGEPROFILE,
    };
    use windows::core::GUID;
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID};

    // SAFETY: enumerates registered text-service CLSIDs without opening their
    // servers or reading any profile text.
    let service_items = unsafe { text_services.EnumInputProcessorInfo() }?;
    let mut text_service_registered = false;
    loop {
        let mut batch = [GUID::zeroed(); 16];
        let mut fetched = 0_u32;
        // SAFETY: the batch and fetched count remain writable for the call.
        unsafe { service_items.Next(&mut batch, Some(&mut fetched)) }.ok()?;
        let fetched = usize::try_from(fetched).map_err(|_| "TSF service count overflow")?;
        if fetched > batch.len() {
            return Err("TSF returned more services than the supplied buffer".into());
        }
        if batch[..fetched].contains(&TSF_ALPHA_CLSID) {
            if text_service_registered {
                return Err("TSF returned the alpha text service more than once".into());
            }
            text_service_registered = true;
        }
        if fetched == 0 {
            break;
        }
    }

    // The legacy enumeration is the authoritative installed-profile view. In
    // particular, it continues to expose a freshly registered profile before
    // that profile has been enabled for the current user.
    let language_profiles = unsafe { text_services.EnumLanguageProfiles(TSF_ALPHA_LANGID) }?;
    let mut enumerated_registration = false;
    loop {
        let mut batch = [TF_LANGUAGEPROFILE::default(); 16];
        let mut fetched = 0_u32;
        // SAFETY: the batch and fetched count remain writable for the call.
        unsafe { language_profiles.Next(&mut batch, &mut fetched) }?;
        let fetched = usize::try_from(fetched).map_err(|_| "TSF profile count overflow")?;
        if fetched > batch.len() {
            return Err("TSF returned more language profiles than the supplied buffer".into());
        }
        for profile in &batch[..fetched] {
            if alpha_language_profile_matches(profile) {
                if enumerated_registration {
                    return Err("TSF returned the alpha language profile more than once".into());
                }
                enumerated_registration = true;
            }
        }
        if fetched == 0 {
            break;
        }
    }

    // Some Windows builds omit disabled profiles from both COM profile
    // enumerators after the registration process exits. The fixed machine
    // profile key remains the authoritative disabled installation record.
    let registered = enumerated_registration || machine_profile_key_exists()?;

    let persisted_enabled = if registered {
        current_user_profile_enable_state()?.unwrap_or(false)
    } else {
        false
    };

    // The modern enumeration contributes active-state evidence. Windows may
    // omit a disabled profile here, so it must not be used as installation
    // evidence.
    let profiles = unsafe { manager.EnumProfiles(TSF_ALPHA_LANGID) }?;
    let mut found = None;
    loop {
        let mut batch = [TF_INPUTPROCESSORPROFILE::default(); 16];
        let mut fetched = 0_u32;
        // SAFETY: the batch and fetched count remain writable for the call.
        unsafe { profiles.Next(&mut batch, &mut fetched) }?;
        let fetched = usize::try_from(fetched).map_err(|_| "TSF profile count overflow")?;
        if fetched > batch.len() {
            return Err("TSF returned more profiles than the supplied buffer".into());
        }
        for profile in &batch[..fetched] {
            if let Some(status) = alpha_profile_status(profile) {
                if found.is_some() {
                    return Err("TSF returned the alpha language profile more than once".into());
                }
                found = Some(status);
            }
        }
        if fetched == 0 {
            break;
        }
    }

    if found.is_some() && !registered {
        return Err("TSF exposed an enabled alpha profile without an installed profile".into());
    }

    // SAFETY: enumeration is read-only and restricted to the fixed alpha
    // CLSID. The returned enumerator owns its COM reference.
    let category_items = unsafe { categories.EnumCategoriesInItem(&TSF_ALPHA_CLSID) }?;
    let mut keyboard_category = false;
    loop {
        let mut batch = [GUID::zeroed(); 16];
        let mut fetched = 0_u32;
        // SAFETY: the batch and fetched count remain writable for the call.
        unsafe { category_items.Next(&mut batch, Some(&mut fetched)) }.ok()?;
        let fetched = usize::try_from(fetched).map_err(|_| "TSF category count overflow")?;
        if fetched > batch.len() {
            return Err("TSF returned more categories than the supplied buffer".into());
        }
        keyboard_category |= batch[..fetched].contains(&GUID_TFCAT_TIP_KEYBOARD);
        if fetched == 0 {
            break;
        }
    }
    Ok(ProfileStatus {
        text_service_registered,
        registered,
        enabled: persisted_enabled,
        active: found.is_some_and(|status| status.active),
        keyboard_category,
    })
}

#[cfg(not(windows))]
fn inspect_system_profile() -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    Err("TSF profile inspection requires Windows".into())
}

fn require_unregistered_state() -> Result<(), Box<dyn std::error::Error>> {
    let com = inspect_com_registration()?;
    let profile = inspect_system_profile()?;
    if com != ComRegistrationStatus::default()
        || profile.text_service_registered
        || profile.registered
        || profile.keyboard_category
    {
        return Err(
            "the fixed TSF alpha identity is already present; refusing to overwrite it".into(),
        );
    }
    Ok(())
}

fn wait_for_unregistered_state() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    loop {
        match require_unregistered_state() {
            Ok(()) => return Ok(()),
            Err(error) if started.elapsed() >= TSF_PROPAGATION_TIMEOUT => return Err(error),
            Err(_) => thread::sleep(TSF_PROPAGATION_POLL_INTERVAL),
        }
    }
}

fn require_exact_registered_state(dll: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let profile = require_exact_registered_layout(dll)?;
    if profile.enabled || profile.active {
        return Err(
            "the installed TSF alpha must be disabled and inactive before machine removal".into(),
        );
    }
    Ok(())
}

fn require_exact_registered_layout(
    dll: &Path,
) -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    let com = inspect_com_registration()?;
    let expected_com = ComRegistrationStatus {
        local_machine_64: true,
        ..Default::default()
    };
    let profile = inspect_system_profile()?;
    if com != expected_com
        || !profile.registered
        || !profile.text_service_registered
        || !profile.keyboard_category
        || !verify_machine_com_registration(dll)?
        || !verify_machine_profile_registration(dll)?
    {
        return Err(
            "the installed TSF alpha layout does not exactly match its local installation receipt"
                .into(),
        );
    }
    Ok(profile)
}

fn profile_matches_toggle_state(profile: ProfileStatus, enabled: bool) -> bool {
    profile_matches_persisted_state(profile, enabled) && !profile.active
}

fn profile_matches_persisted_state(profile: ProfileStatus, enabled: bool) -> bool {
    profile.text_service_registered
        && profile.registered
        && profile.enabled == enabled
        && profile.keyboard_category
}

#[cfg(windows)]
struct WindowsRegistrationBackend {
    text_services: windows::Win32::UI::TextServices::ITfInputProcessorProfiles,
    profiles: windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr,
    categories: windows::Win32::UI::TextServices::ITfCategoryMgr,
    // Fields drop in declaration order. Keep the apartment last so both COM
    // interfaces release before CoUninitialize runs.
    _apartment: RegistrationApartment,
}

#[cfg(windows)]
struct RegistrationApartment;

#[cfg(windows)]
impl Drop for RegistrationApartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx in
        // create_registration_backend on this command's main thread.
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

#[cfg(windows)]
fn create_registration_backend() -> Result<WindowsRegistrationBackend, Box<dyn std::error::Error>> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::TextServices::{
        CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, ITfCategoryMgr,
        ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
    };
    use windows::core::IUnknown;

    // SAFETY: this command owns its main thread until the returned backend is
    // dropped and performs no nested COM initialization.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;
    let apartment = RegistrationApartment;
    // SAFETY: requests the documented legacy TSF service-registration
    // interface. Windows requires this explicit identity before accepting the
    // modern profile call on the tested development host.
    let text_services: ITfInputProcessorProfiles = unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: requests documented TSF system COM servers and stores the owned
    // interface references in the returned backend.
    let profiles: ITfInputProcessorProfileMgr = unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: same ownership and system-server boundary as above.
    let categories: ITfCategoryMgr = unsafe {
        CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    Ok(WindowsRegistrationBackend {
        text_services,
        profiles,
        categories,
        _apartment: apartment,
    })
}

#[cfg(not(windows))]
struct WindowsRegistrationBackend;

#[cfg(not(windows))]
fn create_registration_backend() -> Result<WindowsRegistrationBackend, Box<dyn std::error::Error>> {
    Err("TSF registration requires Windows".into())
}

#[cfg(not(windows))]
impl RegistrationBackend for WindowsRegistrationBackend {
    fn register_com(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::RegisterCom)
    }
    fn register_text_service(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::RegisterTextService)
    }
    fn register_profile(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::RegisterProfile)
    }
    fn register_category(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::RegisterCategory)
    }
    fn verify_registered(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::VerifyRegistered)
    }
    fn unregister_category(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::UnregisterCategory)
    }
    fn unregister_profile(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::UnregisterProfile)
    }
    fn unregister_text_service(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::UnregisterTextService)
    }
    fn unregister_com(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::UnregisterCom)
    }
    fn verify_unregistered(&mut self) -> Result<(), RegistrationAction> {
        Err(RegistrationAction::VerifyUnregistered)
    }
}

#[cfg(windows)]
impl RegistrationBackend for WindowsRegistrationBackend {
    fn register_com(&mut self, dll: &Path) -> Result<(), RegistrationAction> {
        register_machine_com(dll)
    }

    fn register_text_service(&mut self) -> Result<(), RegistrationAction> {
        use ziranma_core::TSF_ALPHA_CLSID;

        // SAFETY: registers only the fixed alpha CLSID as a TSF text-service
        // identity. The separate language profile remains disabled below.
        unsafe { self.text_services.Register(&TSF_ALPHA_CLSID) }.map_err(|error| {
            eprintln!(
                "TSF_SERVICE_REGISTRATION_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            RegistrationAction::RegisterTextService
        })
    }

    fn register_profile(&mut self, dll: &Path) -> Result<(), RegistrationAction> {
        use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

        let description = PROFILE_DESCRIPTION.encode_utf16().collect::<Vec<_>>();
        let icon_path = dll
            .to_str()
            .ok_or(RegistrationAction::RegisterProfile)?
            .encode_utf16()
            .collect::<Vec<_>>();
        // SAFETY: all identities are fixed constants and the description and
        // icon slices remain live for the synchronous call. Using the same
        // legacy profile family as current-user enablement keeps freshly
        // registered disabled profiles discoverable across Windows builds.
        unsafe {
            self.text_services.AddLanguageProfile(
                &TSF_ALPHA_CLSID,
                TSF_ALPHA_LANGID,
                &TSF_ALPHA_PROFILE_GUID,
                &description,
                &icon_path,
                0,
            )
        }
        .map_err(|error| {
            eprintln!(
                "TSF_LEGACY_PROFILE_REGISTRATION_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            RegistrationAction::RegisterProfile
        })?;

        use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
        // SAFETY: the modern registration makes the same fixed profile visible
        // to the Windows 11 user-language list. It runs after the compatible
        // language-profile registration and keeps the profile disabled.
        unsafe {
            self.profiles.RegisterProfile(
                &TSF_ALPHA_CLSID,
                TSF_ALPHA_LANGID,
                &TSF_ALPHA_PROFILE_GUID,
                &description,
                &icon_path,
                0,
                HKL::default(),
                0,
                false,
                0,
            )
        }
        .map_err(|error| {
            eprintln!(
                "TSF_PROFILE_REGISTRATION_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            RegistrationAction::RegisterProfile
        })
    }

    fn register_category(&mut self) -> Result<(), RegistrationAction> {
        use windows::Win32::UI::TextServices::GUID_TFCAT_TIP_KEYBOARD;
        use ziranma_core::TSF_ALPHA_CLSID;

        // SAFETY: registers only the fixed alpha CLSID as a member of the
        // documented keyboard TIP category.
        unsafe {
            self.categories.RegisterCategory(
                &TSF_ALPHA_CLSID,
                &GUID_TFCAT_TIP_KEYBOARD,
                &TSF_ALPHA_CLSID,
            )
        }
        .map_err(|error| {
            eprintln!(
                "TSF_CATEGORY_REGISTRATION_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            RegistrationAction::RegisterCategory
        })
    }

    fn verify_registered(&mut self, dll: &Path) -> Result<(), RegistrationAction> {
        let com = inspect_com_registration().map_err(|_| RegistrationAction::VerifyRegistered)?;
        let profile =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map_err(|_| RegistrationAction::VerifyRegistered)?;
        let expected_com = ComRegistrationStatus {
            local_machine_64: true,
            ..Default::default()
        };
        let exact_com = verify_machine_com_registration(dll)
            .map_err(|_| RegistrationAction::VerifyRegistered)?;
        let exact_profile = verify_machine_profile_registration(dll)
            .map_err(|_| RegistrationAction::VerifyRegistered)?;
        if com == expected_com
            && exact_com
            && exact_profile
            && profile.text_service_registered
            && profile.registered
            && !profile.enabled
            && !profile.active
            && profile.keyboard_category
        {
            Ok(())
        } else {
            eprintln!(
                "TSF_REGISTRATION_VERIFY com={} exact_com={} exact_profile={} service={} profile={} enabled={} active={} category={}",
                com == expected_com,
                exact_com,
                exact_profile,
                profile.text_service_registered,
                profile.registered,
                profile.enabled,
                profile.active,
                profile.keyboard_category,
            );
            Err(RegistrationAction::VerifyRegistered)
        }
    }

    fn unregister_category(&mut self) -> Result<(), RegistrationAction> {
        use windows::Win32::UI::TextServices::GUID_TFCAT_TIP_KEYBOARD;
        use ziranma_core::TSF_ALPHA_CLSID;

        // SAFETY: removes only the exact category membership created above.
        unsafe {
            self.categories.UnregisterCategory(
                &TSF_ALPHA_CLSID,
                &GUID_TFCAT_TIP_KEYBOARD,
                &TSF_ALPHA_CLSID,
            )
        }
        .map_err(|error| {
            eprintln!(
                "TSF_CATEGORY_REMOVAL_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            RegistrationAction::UnregisterCategory
        })
    }

    fn unregister_profile(&mut self) -> Result<(), RegistrationAction> {
        use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

        // SAFETY: removes only the exact fixed zh-CN alpha profile. The modern
        // fallback also cleans installations produced by older alpha builds.
        let legacy = unsafe {
            self.text_services.RemoveLanguageProfile(
                &TSF_ALPHA_CLSID,
                TSF_ALPHA_LANGID,
                &TSF_ALPHA_PROFILE_GUID,
            )
        };
        if legacy.is_ok() {
            return Ok(());
        }
        unsafe {
            self.profiles.UnregisterProfile(
                &TSF_ALPHA_CLSID,
                TSF_ALPHA_LANGID,
                &TSF_ALPHA_PROFILE_GUID,
                0,
            )
        }
        .map_err(|_| RegistrationAction::UnregisterProfile)
    }

    fn unregister_text_service(&mut self) -> Result<(), RegistrationAction> {
        use ziranma_core::TSF_ALPHA_CLSID;

        // SAFETY: removes only the fixed alpha text-service identity after its
        // language profile and category have been removed.
        unsafe { self.text_services.Unregister(&TSF_ALPHA_CLSID) }
            .map_err(|_| RegistrationAction::UnregisterTextService)
    }

    fn unregister_com(&mut self, dll: &Path) -> Result<(), RegistrationAction> {
        unregister_machine_com(dll)
    }

    fn verify_unregistered(&mut self) -> Result<(), RegistrationAction> {
        let com = inspect_com_registration().map_err(|_| RegistrationAction::VerifyUnregistered)?;
        let profile =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map_err(|_| RegistrationAction::VerifyUnregistered)?;
        if com == ComRegistrationStatus::default()
            && !profile.text_service_registered
            && !profile.registered
            && !profile.keyboard_category
        {
            Ok(())
        } else {
            Err(RegistrationAction::VerifyUnregistered)
        }
    }
}

#[cfg(not(windows))]
impl ProfileToggleBackend for WindowsRegistrationBackend {
    fn enable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
        Err(ProfileTransitionAction::Enable)
    }

    fn verify_profile_enabled(&mut self) -> Result<(), ProfileTransitionAction> {
        Err(ProfileTransitionAction::VerifyEnabled)
    }

    fn disable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
        Err(ProfileTransitionAction::Disable)
    }

    fn verify_profile_disabled(&mut self) -> Result<(), ProfileTransitionAction> {
        Err(ProfileTransitionAction::VerifyDisabled)
    }
}

#[cfg(windows)]
impl ProfileToggleBackend for WindowsRegistrationBackend {
    fn enable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
        let attempt = set_current_user_profile_enabled(&self.text_services, &self.profiles, true);
        if attempt.is_ok() {
            return Ok(());
        }
        let observed =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map(|profile| profile_matches_persisted_state(profile, true))
                .unwrap_or(false);
        reconcile_profile_toggle_result(attempt, observed).map_err(|error| {
            eprintln!(
                "TSF_PROFILE_ENABLE_STATE_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            ProfileTransitionAction::Enable
        })
    }

    fn verify_profile_enabled(&mut self) -> Result<(), ProfileTransitionAction> {
        let profile =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map_err(|_| ProfileTransitionAction::VerifyEnabled)?;
        // ActivateProfile can transiently activate the profile on this helper
        // thread. The command deliberately omits the process/session flags,
        // and the replacement script performs the authoritative inactive
        // check in a separate process after this helper exits.
        if profile_matches_persisted_state(profile, true) {
            Ok(())
        } else {
            Err(ProfileTransitionAction::VerifyEnabled)
        }
    }

    fn disable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
        let attempt = set_current_user_profile_enabled(&self.text_services, &self.profiles, false);
        if attempt.is_ok() {
            return Ok(());
        }
        let observed =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map(|profile| profile_matches_toggle_state(profile, false))
                .unwrap_or(false);
        reconcile_profile_toggle_result(attempt, observed).map_err(|error| {
            eprintln!(
                "TSF_PROFILE_ENABLE_STATE_FAILED hresult=0x{:08X}",
                error.code().0 as u32
            );
            ProfileTransitionAction::Disable
        })
    }

    fn verify_profile_disabled(&mut self) -> Result<(), ProfileTransitionAction> {
        let profile =
            inspect_profile_with_managers(&self.text_services, &self.profiles, &self.categories)
                .map_err(|_| ProfileTransitionAction::VerifyDisabled)?;
        if profile_matches_toggle_state(profile, false) {
            Ok(())
        } else {
            Err(ProfileTransitionAction::VerifyDisabled)
        }
    }
}

#[cfg(windows)]
fn set_current_user_profile_enabled(
    profiles: &windows::Win32::UI::TextServices::ITfInputProcessorProfiles,
    manager: &windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr,
    enabled: bool,
) -> windows::core::Result<()> {
    use windows::Win32::Foundation::E_INVALIDARG;
    use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
    use windows::Win32::UI::TextServices::{
        TF_IPPMF_DISABLEPROFILE, TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, TF_IPPMF_ENABLEPROFILE,
        TF_PROFILETYPE_INPUTPROCESSOR,
    };
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

    let flag = if enabled {
        TF_IPPMF_ENABLEPROFILE
    } else {
        TF_IPPMF_DISABLEPROFILE
    };

    apply_current_user_profile_state(
        || {
            // SAFETY: the legacy compatibility call changes only the current
            // user's enabled bit for the fixed alpha profile.
            unsafe {
                profiles.EnableLanguageProfile(
                    &TSF_ALPHA_CLSID,
                    TSF_ALPHA_LANGID,
                    &TSF_ALPHA_PROFILE_GUID,
                    enabled,
                )
            }
            .inspect_err(|error| {
                eprintln!(
                    "TSF_LEGACY_PROFILE_ENABLE_STATE_FAILED hresult=0x{:08X}",
                    error.code().0 as u32
                );
            })
        },
        || {
            // SAFETY: the modern profile manager receives the same fixed
            // identity. TF_IPPMF_ENABLEPROFILE/TF_IPPMF_DISABLEPROFILE is the
            // documented persistent current-user operation. No default,
            // process-wide, or session-wide flag is supplied; any activation
            // is confined to this short-lived helper thread.
            retry_profile_manager_state(
                || unsafe {
                    manager.ActivateProfile(
                        TF_PROFILETYPE_INPUTPROCESSOR,
                        TSF_ALPHA_LANGID,
                        &TSF_ALPHA_CLSID,
                        &TSF_ALPHA_PROFILE_GUID,
                        HKL::default(),
                        flag | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
                    )
                },
                || unsafe {
                    manager.ActivateProfile(
                        TF_PROFILETYPE_INPUTPROCESSOR,
                        TSF_ALPHA_LANGID,
                        &TSF_ALPHA_CLSID,
                        &TSF_ALPHA_PROFILE_GUID,
                        HKL::default(),
                        flag,
                    )
                },
                |error| error.code() == E_INVALIDARG,
            )
        },
    )
}

fn apply_current_user_profile_state<E>(
    legacy: impl FnOnce() -> Result<(), E>,
    modern: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    // EnableLanguageProfile can report success without producing durable
    // Windows 11 state. Keep it as a compatibility notification, but always
    // execute the Vista+ profile-manager operation and treat that result as
    // authoritative.
    let _ = legacy();
    modern()
}

fn retry_profile_manager_state<E>(
    primary: impl FnOnce() -> Result<(), E>,
    compatibility: impl FnOnce() -> Result<(), E>,
    should_retry: impl FnOnce(&E) -> bool,
) -> Result<(), E> {
    match primary() {
        Err(error) if should_retry(&error) => compatibility(),
        result => result,
    }
}

fn reconcile_profile_toggle_result<E>(
    attempt: Result<(), E>,
    observed_requested_state: bool,
) -> Result<(), E> {
    match attempt {
        Err(_) if observed_requested_state => Ok(()),
        result => result,
    }
}

#[cfg(windows)]
struct OpenedRegistryKey(windows::Win32::System::Registry::HKEY);

#[cfg(windows)]
impl Drop for OpenedRegistryKey {
    fn drop(&mut self) {
        // SAFETY: this guard owns a handle returned by a successful registry
        // open or create call.
        unsafe {
            let _ = windows::Win32::System::Registry::RegCloseKey(self.0);
        }
    }
}

#[cfg(windows)]
fn alpha_clsid_registry_path() -> String {
    alpha_inproc_server_registry_path()
        .strip_suffix("\\InprocServer32")
        .unwrap()
        .to_owned()
}

#[cfg(windows)]
fn register_machine_com(dll: &Path) -> Result<(), RegistrationAction> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_CREATED_NEW_KEY,
        REG_OPTION_NON_VOLATILE, RegCreateKeyExW, RegDeleteKeyExW,
    };
    use windows::core::PCWSTR;

    let clsid_path = wide_nul(&alpha_clsid_registry_path());
    let mut clsid = HKEY::default();
    let mut disposition = Default::default();
    // SAFETY: the fixed path is NUL-terminated and live for the synchronous
    // call; output handle and disposition point to writable storage.
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(clsid_path.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut clsid,
            Some(&mut disposition),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(RegistrationAction::RegisterCom);
    }
    let clsid = OpenedRegistryKey(clsid);
    if disposition != REG_CREATED_NEW_KEY {
        return Err(RegistrationAction::RegisterCom);
    }

    let child_name = wide_nul("InprocServer32");
    let mut child = HKEY::default();
    let mut child_disposition = Default::default();
    // SAFETY: the child name and outputs obey the same boundary as the parent
    // create call above.
    let child_result = unsafe {
        RegCreateKeyExW(
            clsid.0,
            PCWSTR(child_name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut child,
            Some(&mut child_disposition),
        )
    };
    if child_result != ERROR_SUCCESS || child_disposition != REG_CREATED_NEW_KEY {
        drop(clsid);
        // SAFETY: removes only the exact newly-created empty alpha CLSID key.
        let rollback = unsafe {
            RegDeleteKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(clsid_path.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            )
        };
        return Err(if rollback == ERROR_SUCCESS {
            RegistrationAction::RegisterCom
        } else {
            RegistrationAction::RegisterComRollbackIncomplete
        });
    }
    let child = OpenedRegistryKey(child);
    let write_result = set_registry_string(clsid.0, None, PROFILE_DESCRIPTION)
        .map_err(|_| RegistrationAction::RegisterCom)
        .and_then(|()| {
            dll.to_str()
                .ok_or(RegistrationAction::RegisterCom)
                .and_then(|dll_text| {
                    set_registry_string(child.0, None, dll_text)
                        .and_then(|()| {
                            set_registry_string(child.0, Some("ThreadingModel"), "Apartment")
                        })
                        .map_err(|_| RegistrationAction::RegisterCom)
                })
        });
    drop(child);
    drop(clsid);
    if write_result.is_err() {
        let inproc_path = wide_nul(&alpha_inproc_server_registry_path());
        // SAFETY: both paths identify only keys created by this failed call.
        let (child_rollback, parent_rollback) = unsafe {
            let child_rollback = RegDeleteKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(inproc_path.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            );
            let parent_rollback = RegDeleteKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(clsid_path.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            );
            (child_rollback, parent_rollback)
        };
        return Err(
            if child_rollback == ERROR_SUCCESS && parent_rollback == ERROR_SUCCESS {
                RegistrationAction::RegisterCom
            } else {
                RegistrationAction::RegisterComRollbackIncomplete
            },
        );
    }
    Ok(())
}

#[cfg(windows)]
fn unregister_machine_com(dll: &Path) -> Result<(), RegistrationAction> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, RegDeleteKeyExW};
    use windows::core::PCWSTR;

    if !verify_machine_com_registration(dll).map_err(|_| RegistrationAction::UnregisterCom)? {
        return Err(RegistrationAction::UnregisterCom);
    }
    let inproc_path = wide_nul(&alpha_inproc_server_registry_path());
    let clsid_path = wide_nul(&alpha_clsid_registry_path());
    // SAFETY: exact ownership, values, and empty subkey counts were verified
    // immediately above. Only the fixed 64-bit machine keys are removed.
    let child_result = unsafe {
        RegDeleteKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(inproc_path.as_ptr()),
            KEY_WOW64_64KEY.0,
            None,
        )
    };
    if child_result != ERROR_SUCCESS {
        return Err(RegistrationAction::UnregisterCom);
    }
    // SAFETY: the child was removed and the verified parent has no other
    // values or subkeys.
    let parent_result = unsafe {
        RegDeleteKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(clsid_path.as_ptr()),
            KEY_WOW64_64KEY.0,
            None,
        )
    };
    if parent_result != ERROR_SUCCESS {
        return Err(if restore_machine_com_child(dll) {
            RegistrationAction::UnregisterCom
        } else {
            RegistrationAction::UnregisterComRollbackIncomplete
        });
    }
    Ok(())
}

#[cfg(windows)]
fn restore_machine_com_child(dll: &Path) -> bool {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_CREATED_NEW_KEY,
        REG_OPTION_NON_VOLATILE, RegCreateKeyExW, RegDeleteKeyExW, RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    let clsid_path = wide_nul(&alpha_clsid_registry_path());
    let mut clsid = HKEY::default();
    // SAFETY: opens only the fixed parent that failed to delete.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(clsid_path.as_ptr()),
            None,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            &mut clsid,
        )
    };
    if result != ERROR_SUCCESS {
        return false;
    }
    let clsid = OpenedRegistryKey(clsid);
    if registry_key_counts(clsid.0).ok() != Some((0, 1))
        || read_registry_string(clsid.0, None)
            .ok()
            .flatten()
            .as_deref()
            != Some(PROFILE_DESCRIPTION)
    {
        return false;
    }
    let child_name = wide_nul("InprocServer32");
    let mut child = HKEY::default();
    let mut disposition = Default::default();
    // SAFETY: recreates only the exact child removed by the interrupted
    // unregister operation.
    let result = unsafe {
        RegCreateKeyExW(
            clsid.0,
            PCWSTR(child_name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut child,
            Some(&mut disposition),
        )
    };
    if result != ERROR_SUCCESS || disposition != REG_CREATED_NEW_KEY {
        return false;
    }
    let child = OpenedRegistryKey(child);
    let written = dll.to_str().is_some_and(|dll_text| {
        set_registry_string(child.0, None, dll_text).is_ok()
            && set_registry_string(child.0, Some("ThreadingModel"), "Apartment").is_ok()
    });
    drop(child);
    drop(clsid);
    if !written {
        let inproc_path = wide_nul(&alpha_inproc_server_registry_path());
        // SAFETY: cleanup is limited to the child created by this helper.
        unsafe {
            let _ = RegDeleteKeyExW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(inproc_path.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            );
        }
        return false;
    }
    verify_machine_com_registration(dll).unwrap_or(false)
}

#[cfg(windows)]
fn verify_machine_com_registration(dll: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    let clsid_path = wide_nul(&alpha_clsid_registry_path());
    let mut clsid = HKEY::default();
    // SAFETY: the fixed path is NUL-terminated and outputs are writable.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(clsid_path.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut clsid,
        )
    };
    if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
        return Ok(false);
    }
    if result != ERROR_SUCCESS {
        return Err("cannot verify the fixed machine COM class key".into());
    }
    let clsid = OpenedRegistryKey(clsid);
    let (clsid_subkeys, clsid_values) = registry_key_counts(clsid.0)?;
    if clsid_subkeys != 1
        || clsid_values != 1
        || read_registry_string(clsid.0, None)?.as_deref() != Some(PROFILE_DESCRIPTION)
    {
        return Ok(false);
    }

    let child_name = wide_nul("InprocServer32");
    let mut child = HKEY::default();
    // SAFETY: opens only the fixed child beneath the owned class key.
    let result = unsafe {
        RegOpenKeyExW(
            clsid.0,
            PCWSTR(child_name.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut child,
        )
    };
    if result != ERROR_SUCCESS {
        return Ok(false);
    }
    let child = OpenedRegistryKey(child);
    let (child_subkeys, child_values) = registry_key_counts(child.0)?;
    let dll_text = dll
        .to_str()
        .ok_or("the immutable TSF DLL path is not valid Unicode")?;
    Ok(child_subkeys == 0
        && child_values == 2
        && read_registry_string(child.0, None)?.as_deref() == Some(dll_text)
        && read_registry_string(child.0, Some("ThreadingModel"))?.as_deref() == Some("Apartment"))
}

#[cfg(windows)]
fn verify_machine_profile_registration(dll: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RegOpenKeyExW,
    };
    use windows::core::PCWSTR;

    let profile_path = wide_nul(&alpha_language_profile_registry_path());
    let mut profile = HKEY::default();
    // SAFETY: the fixed path is NUL-terminated and the output is writable.
    let result = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(profile_path.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut profile,
        )
    };
    if result == ERROR_FILE_NOT_FOUND || result == ERROR_PATH_NOT_FOUND {
        return Ok(false);
    }
    if result != ERROR_SUCCESS {
        return Err("cannot verify the fixed machine language profile key".into());
    }
    let profile = OpenedRegistryKey(profile);
    let (subkeys, values) = registry_key_counts(profile.0)?;
    let dll_text = dll
        .to_str()
        .ok_or("the immutable TSF DLL path is not valid Unicode")?;
    let description = read_registry_string(profile.0, Some("Description"))?;
    let description_matches = description.as_deref() == Some(PROFILE_DESCRIPTION);
    let bounded_description = description.as_deref().is_some_and(|description| {
        !description.is_empty() && description.encode_utf16().count() <= 128
    });
    let icon_matches =
        read_registry_string(profile.0, Some("IconFile"))?.as_deref() == Some(dll_text);
    let icon_index = read_registry_dword(profile.0, "IconIndex")?;
    let default_enable = read_registry_dword(profile.0, "Enable")?;
    let common = subkeys == 0 && icon_matches && icon_index == Some(0);
    let modern_shape = values == 4 && description_matches && default_enable == Some(0);
    let legacy_shape = values == 3 && bounded_description && default_enable.is_none();
    let exact = common && (modern_shape || legacy_shape);
    if !exact {
        eprintln!(
            "TSF_PROFILE_REGISTRY_VERIFY subkeys={subkeys} values={values} description={} icon={} icon_index={icon_index:?} default_enable={default_enable:?}",
            description_matches, icon_matches,
        );
    }
    Ok(exact)
}

#[cfg(not(windows))]
fn verify_machine_com_registration(_dll: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    Err("machine COM registration verification requires Windows".into())
}

#[cfg(not(windows))]
fn verify_machine_profile_registration(_dll: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    Err("machine language profile verification requires Windows".into())
}

#[cfg(windows)]
fn registry_key_counts(
    key: windows::Win32::System::Registry::HKEY,
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::RegQueryInfoKeyW;

    let mut subkeys = 0_u32;
    let mut values = 0_u32;
    // SAFETY: requests only bounded counts and supplies writable outputs.
    let result = unsafe {
        RegQueryInfoKeyW(
            key,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    if result != ERROR_SUCCESS {
        return Err("cannot verify the fixed machine COM key shape".into());
    }
    Ok((subkeys, values))
}

#[cfg(windows)]
fn set_registry_string(
    key: windows::Win32::System::Registry::HKEY,
    name: Option<&str>,
    value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{REG_SZ, RegSetValueExW};
    use windows::core::PCWSTR;

    let name = name.map(wide_nul);
    let name = name
        .as_ref()
        .map_or_else(PCWSTR::null, |value| PCWSTR(value.as_ptr()));
    let bytes = registry_string_bytes(value);
    // SAFETY: the optional name is NUL-terminated, the byte slice contains a
    // NUL-terminated UTF-16 string, and both remain live for the call.
    let result = unsafe { RegSetValueExW(key, name, None, REG_SZ, Some(&bytes)) };
    if result != ERROR_SUCCESS {
        return Err("cannot write the fixed machine COM value".into());
    }
    Ok(())
}

#[cfg(windows)]
fn read_registry_string(
    key: windows::Win32::System::Registry::HKEY,
    name: Option<&str>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;

    let name = name.map(wide_nul);
    let name = name
        .as_ref()
        .map_or_else(PCWSTR::null, |value| PCWSTR(value.as_ptr()));
    let mut byte_count = 0_u32;
    // SAFETY: first call requests only the bounded byte count.
    let result = unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut byte_count),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if result != ERROR_SUCCESS
        || !(2..=32_768).contains(&byte_count)
        || !byte_count.is_multiple_of(2)
    {
        return Err("cannot read the fixed machine registry string value".into());
    }
    let mut bytes = vec![0_u8; usize::try_from(byte_count)?];
    // SAFETY: the allocated buffer has exactly the reported writable size.
    let result = unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            name,
            RRF_RT_REG_SZ,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut byte_count),
        )
    };
    if result != ERROR_SUCCESS
        || usize::try_from(byte_count)? > bytes.len()
        || byte_count < 2
        || !byte_count.is_multiple_of(2)
    {
        return Err("cannot read the fixed machine registry string value".into());
    }
    bytes.truncate(usize::try_from(byte_count)?);
    let mut utf16 = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    if utf16.last() != Some(&0) {
        return Err("the fixed machine COM string is not terminated".into());
    }
    while utf16.last() == Some(&0) {
        utf16.pop();
    }
    Ok(Some(String::from_utf16(&utf16)?))
}

#[cfg(windows)]
fn read_registry_dword(
    key: windows::Win32::System::Registry::HKEY,
    name: &str,
) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW};
    use windows::core::PCWSTR;

    let name = wide_nul(name);
    let mut value = 0_u32;
    let mut byte_count = u32::try_from(std::mem::size_of::<u32>())?;
    // SAFETY: the fixed value name is NUL-terminated and the value and size
    // outputs are bounded writable storage for one REG_DWORD.
    let result = unsafe {
        RegGetValueW(
            key,
            PCWSTR::null(),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut byte_count),
        )
    };
    if result == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if result != ERROR_SUCCESS || byte_count != u32::try_from(std::mem::size_of::<u32>())? {
        return Err("cannot read the fixed machine registry DWORD value".into());
    }
    Ok(Some(value))
}

#[cfg(windows)]
fn registry_string_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn wide_nul(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn checked_add(left: usize, right: usize) -> Result<usize, Box<dyn std::error::Error>> {
    left.checked_add(right)
        .ok_or_else(|| "PE offset overflow".into())
}

fn require_range(
    bytes: &[u8],
    offset: usize,
    length: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let end = offset.checked_add(length).ok_or("PE range overflow")?;
    if end > bytes.len() {
        return Err("PE range extends outside the DLL".into());
    }
    Ok(())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Box<dyn std::error::Error>> {
    require_range(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Box<dyn std::error::Error>> {
    require_range(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_powershell_51_parses_the_replacement_script() {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("replace-tsf-alpha.ps1");
        let bytes = fs::read(&script).unwrap();
        assert!(
            bytes.is_ascii() || bytes.starts_with(&[0xef, 0xbb, 0xbf]),
            "Windows PowerShell 5.1 requires ASCII or a UTF-8 BOM for this script"
        );

        let powershell = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        let checker = "$parseTokens=$null; $parseErrors=$null; ".to_owned()
            + "[void][System.Management.Automation.Language.Parser]::ParseFile("
            + "$env:ZIRANMA_REPLACE_PARSE_TARGET, [ref]$parseTokens, [ref]$parseErrors); "
            + "if ($parseErrors.Count -ne 0) { exit 1 }";
        let output = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &checker])
            .env("ZIRANMA_REPLACE_PARSE_TARGET", &script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Windows PowerShell 5.1 rejected the replacement script: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn replacement_script_uses_one_bounded_stability_gate_per_success_path() {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("replace-tsf-alpha.ps1");
        let contents = fs::read_to_string(script).unwrap();

        assert_eq!(
            contents
                .matches("Invoke-DevCtl -Arguments $currentUserVerificationArguments -Quiet")
                .count(),
            2,
            "the already-current and replacement paths should each verify once"
        );
        assert!(
            !contents.contains("Invoke-DevCtl -Arguments @('inspect'"),
            "host-cache-state already validates the source DLL while enable verification validates the installed layout"
        );
        assert!(contents.contains("Timing: {0} ms total ({1})"));
    }

    #[test]
    fn update_wrapper_keeps_status_read_only_and_replacement_explicit() {
        let wrapper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("update-ime.cmd");
        let contents = fs::read_to_string(wrapper).unwrap();

        assert!(contents.contains("Usage: update-ime.cmd [status]"));
        assert!(contents.contains("-StatusOnly"));
        assert!(contents.contains("-EnableCurrentUserAfterReplace"));
        assert!(contents.contains("if /i \"%update_mode%\"==\"status\" goto status"));

        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("replace-tsf-alpha.ps1");
        let script_contents = fs::read_to_string(script).unwrap();
        assert!(script_contents.contains("This action: read only"));
        assert!(
            script_contents.contains("StatusOnly cannot be combined with replacement switches.")
        );
    }

    #[test]
    fn parser_requires_one_explicit_dll() {
        assert_eq!(parse_options(Vec::<String>::new()).unwrap(), Options::Help);
        assert_eq!(
            parse_options(["inspect", "--dll", "alpha.dll"].map(str::to_owned)).unwrap(),
            Options::Inspect {
                dll: PathBuf::from("alpha.dll")
            }
        );
        assert_eq!(
            parse_options(["host-cache-state", "--dll", "alpha.dll"].map(str::to_owned)).unwrap(),
            Options::HostCacheState {
                dll: PathBuf::from("alpha.dll")
            }
        );
        assert_eq!(
            parse_options(
                [
                    "register-machine",
                    "--dll",
                    "alpha.dll",
                    "--confirm-machine-wide-development-alpha",
                ]
                .map(str::to_owned)
            )
            .unwrap(),
            Options::RegisterMachine {
                dll: PathBuf::from("alpha.dll")
            }
        );
        assert_eq!(
            parse_options(
                [
                    "unregister-machine",
                    "--confirm-machine-wide-development-alpha",
                ]
                .map(str::to_owned)
            )
            .unwrap(),
            Options::UnregisterMachine
        );
        assert_eq!(
            parse_options(
                [
                    "enable-current-user",
                    "--confirm-enable-current-user-development-alpha",
                ]
                .map(str::to_owned)
            )
            .unwrap(),
            Options::EnableCurrentUser
        );
        assert_eq!(
            parse_options(
                [
                    "disable-current-user",
                    "--confirm-disable-current-user-development-alpha",
                ]
                .map(str::to_owned)
            )
            .unwrap(),
            Options::DisableCurrentUser
        );
        assert_eq!(
            parse_options(["current-user-state"].map(str::to_owned)).unwrap(),
            Options::CurrentUserState
        );
        assert_eq!(
            parse_options(["verify-current-user-enabled"].map(str::to_owned)).unwrap(),
            Options::VerifyCurrentUserEnabled {
                allow_active: false
            }
        );
        assert_eq!(
            parse_options(["verify-current-user-enabled", "--allow-active"].map(str::to_owned))
                .unwrap(),
            Options::VerifyCurrentUserEnabled { allow_active: true }
        );
        assert!(
            parse_options(["register-machine", "--dll", "alpha.dll"].map(str::to_owned)).is_err()
        );
        assert!(parse_options(["unregister-machine"].map(str::to_owned)).is_err());
        assert!(parse_options(["enable-current-user"].map(str::to_owned)).is_err());
        assert!(parse_options(["disable-current-user"].map(str::to_owned)).is_err());
        assert!(parse_options(["current-user-state", "extra"].map(str::to_owned)).is_err());
        assert!(
            parse_options(["verify-current-user-enabled", "extra"].map(str::to_owned)).is_err()
        );
        assert!(
            parse_options(
                [
                    "enable-current-user",
                    "--confirm-disable-current-user-development-alpha",
                ]
                .map(str::to_owned)
            )
            .is_err()
        );
        assert!(parse_options(["inspect"].map(str::to_owned)).is_err());
        assert!(parse_options(["host-cache-state"].map(str::to_owned)).is_err());
        assert!(
            parse_options(["inspect", "--dll", "a.dll", "--dll", "b.dll"].map(str::to_owned))
                .is_err()
        );
        let error = parse_options(["inspect", "secret-value"].map(str::to_owned))
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret-value"));
    }

    #[test]
    fn synthetic_pe64_exposes_only_the_build_boundary() {
        let bytes = synthetic_pe(&REQUIRED_COM_EXPORTS, false);
        let inspection = inspect_pe(&bytes).unwrap();
        assert_eq!(inspection.machine, IMAGE_FILE_MACHINE_AMD64);
        assert_eq!(inspection.optional_magic, PE32_PLUS_MAGIC);
        assert!(inspection.is_dll);
        assert_eq!(
            inspection.exports,
            REQUIRED_COM_EXPORTS
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
        assert!(!inspection.certificate_table_present);
        validate_alpha_dll(&inspection).unwrap();
    }

    #[test]
    fn validation_rejects_registration_exports_and_missing_com_exports() {
        let with_registration = inspect_pe(&synthetic_pe(
            &["DllCanUnloadNow", "DllGetClassObject", "DllRegisterServer"],
            false,
        ))
        .unwrap();
        assert!(validate_alpha_dll(&with_registration).is_err());

        let missing_factory = inspect_pe(&synthetic_pe(&["DllCanUnloadNow"], false)).unwrap();
        assert!(validate_alpha_dll(&missing_factory).is_err());
    }

    #[test]
    fn certificate_directory_is_reported_without_claiming_validation() {
        let inspection = inspect_pe(&synthetic_pe(&REQUIRED_COM_EXPORTS, true)).unwrap();
        assert!(inspection.certificate_table_present);
        let report = render_report(
            Path::new("target/release/ziranma_core.dll"),
            &inspection,
            ComRegistrationStatus::default(),
            ProfileStatus::default(),
            LoadedHostStatus {
                scan_available: true,
                matching_version: 1,
                other_versions: 2,
            },
        );
        assert!(report.contains("证书目录：存在（未验证签名有效性）"));
        assert!(report.contains("COM 注册：未发现"));
        assert!(report.contains("系统语言配置：未发现"));
        assert!(report.contains("键盘类别：未发现"));
        assert!(report.contains("宿主缓存：此版本 1，其他版本 2（仅计可见进程）"));
        assert!(report.contains("本次操作：只读"));
        assert!(!report.contains("下一步"));
    }

    #[test]
    fn loaded_host_report_is_redacted_and_version_scoped() {
        let digest = "ab".repeat(32);
        let path = PathBuf::from(format!(
            r"D:\repo\.local\tsf-alpha\builds\{digest}\ziranma_core.dll"
        ));
        assert_eq!(immutable_alpha_digest(&path), Some(digest));
        assert_eq!(
            immutable_alpha_digest(Path::new(r"D:\private\builds\abab\ziranma_core.dll")),
            None
        );
        assert_eq!(
            render_loaded_hosts(LoadedHostStatus {
                scan_available: true,
                matching_version: 0,
                other_versions: 0,
            }),
            "未发现正在加载 Alpha 的应用"
        );
        assert_eq!(render_loaded_hosts(LoadedHostStatus::default()), "无法检查");
        assert_eq!(
            render_host_cache_state(LoadedHostStatus {
                scan_available: true,
                matching_version: 3,
                other_versions: 4,
            }),
            "TSF_HOST_CACHE_STATE schema=ziranma-tsf-host-cache-state-v1 \
             scan_available=true matching_version=3 other_versions=4 writes=false\n"
        );
    }

    #[test]
    fn registration_report_is_bounded_and_never_echoes_registry_paths() {
        assert_eq!(
            render_com_registration(ComRegistrationStatus {
                current_user_64: true,
                local_machine_64: true,
                current_user_32: false,
                local_machine_32: true,
            }),
            "已发现（当前用户 64 位、本机 64 位、本机 32 位）"
        );
        assert_eq!(
            render_com_registration(ComRegistrationStatus::default()),
            "未发现"
        );
        assert!(
            !render_com_registration(ComRegistrationStatus {
                current_user_64: true,
                ..Default::default()
            })
            .contains("Software\\Classes")
        );
    }

    #[cfg(windows)]
    #[test]
    fn fixed_clsid_registry_path_matches_the_documented_identity() {
        assert_eq!(
            alpha_inproc_server_registry_path(),
            "Software\\Classes\\CLSID\\{4CC8427B-D0F5-439E-B6AF-D45EACD7E577}\\InprocServer32"
        );
        assert_eq!(
            alpha_language_profile_registry_path(),
            "Software\\Microsoft\\CTF\\TIP\\{4CC8427B-D0F5-439E-B6AF-D45EACD7E577}\\LanguageProfile\\0x00000804\\{8099D3F8-9F40-4DA5-9B01-C12DE0CD6370}"
        );
    }

    #[test]
    fn malformed_headers_and_unbounded_export_names_are_rejected() {
        assert!(inspect_pe(b"not a PE image").is_err());
        let mut bytes = synthetic_pe(&REQUIRED_COM_EXPORTS, false);
        bytes[0x250..0x250 + MAX_EXPORT_NAME_BYTES + 1].fill(b'x');
        assert!(inspect_pe(&bytes).is_err());

        let mut incomplete_certificate = synthetic_pe(&REQUIRED_COM_EXPORTS, false);
        let certificate_size = 0x84 + 20 + 116 + 4 * 8;
        put_u32(&mut incomplete_certificate, certificate_size, 8);
        assert!(inspect_pe(&incomplete_certificate).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn system_profile_match_requires_every_fixed_identity_field() {
        use windows::Win32::UI::TextServices::{
            GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE, TF_IPP_FLAG_ACTIVE,
            TF_IPP_FLAG_ENABLED, TF_LANGUAGEPROFILE, TF_PROFILETYPE_INPUTPROCESSOR,
        };
        use windows::core::GUID;
        use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

        let exact = TF_INPUTPROCESSORPROFILE {
            dwProfileType: TF_PROFILETYPE_INPUTPROCESSOR,
            langid: TSF_ALPHA_LANGID,
            clsid: TSF_ALPHA_CLSID,
            guidProfile: TSF_ALPHA_PROFILE_GUID,
            catid: GUID_TFCAT_TIP_KEYBOARD,
            dwFlags: TF_IPP_FLAG_ACTIVE | TF_IPP_FLAG_ENABLED,
            ..Default::default()
        };
        assert_eq!(
            alpha_profile_status(&exact),
            Some(ProfileStatus {
                text_service_registered: false,
                registered: true,
                enabled: true,
                active: true,
                keyboard_category: false,
            })
        );

        let mut wrong_profile = exact;
        wrong_profile.guidProfile = GUID::from_u128(1);
        assert_eq!(alpha_profile_status(&wrong_profile), None);

        let installed = TF_LANGUAGEPROFILE {
            clsid: TSF_ALPHA_CLSID,
            langid: TSF_ALPHA_LANGID,
            guidProfile: TSF_ALPHA_PROFILE_GUID,
            catid: GUID_TFCAT_TIP_KEYBOARD,
            fActive: false.into(),
        };
        assert!(alpha_language_profile_matches(&installed));
        let mut wrong_installed = installed;
        wrong_installed.clsid = GUID::from_u128(1);
        assert!(!alpha_language_profile_matches(&wrong_installed));
    }

    #[test]
    fn toggle_state_requires_complete_registration_and_never_accepts_active() {
        let disabled = ProfileStatus {
            text_service_registered: true,
            registered: true,
            enabled: false,
            active: false,
            keyboard_category: true,
        };
        assert!(profile_matches_toggle_state(disabled, false));
        assert!(!profile_matches_toggle_state(disabled, true));

        let enabled = ProfileStatus {
            enabled: true,
            ..disabled
        };
        assert!(profile_matches_toggle_state(enabled, true));

        let active = ProfileStatus {
            active: true,
            ..enabled
        };
        assert!(profile_matches_persisted_state(active, true));
        assert!(!profile_matches_toggle_state(active, true));

        let incomplete = ProfileStatus {
            keyboard_category: false,
            ..disabled
        };
        assert!(!profile_matches_toggle_state(incomplete, false));
    }

    #[test]
    fn modern_profile_state_call_is_always_executed_and_authoritative() {
        let calls = std::cell::RefCell::new(Vec::new());
        let result = apply_current_user_profile_state(
            || {
                calls.borrow_mut().push("legacy");
                Ok::<(), &'static str>(())
            },
            || {
                calls.borrow_mut().push("modern");
                Err("modern failed")
            },
        );
        assert_eq!(result, Err("modern failed"));
        assert_eq!(*calls.borrow(), ["legacy", "modern"]);

        calls.borrow_mut().clear();
        let result = apply_current_user_profile_state(
            || {
                calls.borrow_mut().push("legacy");
                Err("legacy failed")
            },
            || {
                calls.borrow_mut().push("modern");
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(*calls.borrow(), ["legacy", "modern"]);
    }

    #[test]
    fn profile_manager_retries_only_the_optional_invalid_argument_case() {
        let calls = std::cell::RefCell::new(Vec::new());
        let result = retry_profile_manager_state(
            || {
                calls.borrow_mut().push("primary");
                Err("invalid argument")
            },
            || {
                calls.borrow_mut().push("compatibility");
                Ok(())
            },
            |error| *error == "invalid argument",
        );
        assert_eq!(result, Ok(()));
        assert_eq!(*calls.borrow(), ["primary", "compatibility"]);

        calls.borrow_mut().clear();
        let result = retry_profile_manager_state(
            || {
                calls.borrow_mut().push("primary");
                Err("other failure")
            },
            || {
                calls.borrow_mut().push("compatibility");
                Ok(())
            },
            |error| *error == "invalid argument",
        );
        assert_eq!(result, Err("other failure"));
        assert_eq!(*calls.borrow(), ["primary"]);

        calls.borrow_mut().clear();
        let result = retry_profile_manager_state(
            || {
                calls.borrow_mut().push("primary");
                Ok::<(), &'static str>(())
            },
            || {
                calls.borrow_mut().push("compatibility");
                Ok(())
            },
            |_| true,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(*calls.borrow(), ["primary"]);
    }

    #[test]
    fn failed_toggle_is_accepted_only_after_observing_the_requested_state() {
        assert_eq!(
            reconcile_profile_toggle_result::<&'static str>(Err("modern failed"), true),
            Ok(())
        );
        assert_eq!(
            reconcile_profile_toggle_result(Err("modern failed"), false),
            Err("modern failed")
        );
        assert_eq!(
            reconcile_profile_toggle_result::<&'static str>(Ok(()), false),
            Ok(())
        );
    }

    #[test]
    fn installation_receipt_is_strict_and_binds_the_digest_path() {
        let digest = "ab".repeat(32);
        let receipt = InstallReceipt {
            dll_sha256: digest.clone(),
            relative_dll: relative_dll_path(&digest),
        };
        let rendered = render_install_receipt(&receipt);
        assert_eq!(parse_install_receipt(&rendered).unwrap(), receipt);
        assert!(
            parse_install_receipt(&rendered.replace(
                "profile_enabled_by_default=false",
                "profile_enabled_by_default=true"
            ))
            .is_err()
        );
        assert!(
            parse_install_receipt(&rendered.replace(&digest, &format!("{digest}/escape"))).is_err()
        );
        assert!(parse_install_receipt(&format!("{rendered}extra=value\n")).is_err());
    }

    #[test]
    fn registration_transaction_rolls_back_in_reverse_order() {
        let dll = Path::new("immutable.dll");

        let mut success = FakeBackend::default();
        register_transaction(&mut success, dll).unwrap();
        assert_eq!(
            success.calls,
            [
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
                RegistrationAction::VerifyRegistered,
            ]
        );

        let mut service_failure = FakeBackend::failing([RegistrationAction::RegisterTextService]);
        register_transaction(&mut service_failure, dll).unwrap_err();
        assert_eq!(
            service_failure.calls,
            [
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::UnregisterCom,
            ]
        );

        let mut profile_failure = FakeBackend::failing([RegistrationAction::RegisterProfile]);
        register_transaction(&mut profile_failure, dll).unwrap_err();
        assert_eq!(
            profile_failure.calls,
            [
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::UnregisterCom,
            ]
        );

        let mut category_failure = FakeBackend::failing([RegistrationAction::RegisterCategory]);
        register_transaction(&mut category_failure, dll).unwrap_err();
        assert_eq!(
            category_failure.calls,
            [
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::UnregisterCom,
            ]
        );

        let mut verify_failure = FakeBackend::failing([RegistrationAction::VerifyRegistered]);
        register_transaction(&mut verify_failure, dll).unwrap_err();
        assert_eq!(
            verify_failure.calls,
            [
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
                RegistrationAction::VerifyRegistered,
                RegistrationAction::UnregisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::UnregisterCom,
            ]
        );
    }

    #[test]
    fn transaction_error_reports_incomplete_rollback_without_external_values() {
        let dll = Path::new("secret-path.dll");
        let mut backend = FakeBackend::failing([
            RegistrationAction::RegisterProfile,
            RegistrationAction::UnregisterCom,
        ]);
        let error = register_transaction(&mut backend, dll).unwrap_err();
        assert_eq!(error.failed, RegistrationAction::RegisterProfile);
        assert_eq!(error.rollback_failed, [RegistrationAction::UnregisterCom]);
        let message = error.to_string();
        assert!(message.contains("rollback incomplete"));
        assert!(!message.contains("secret-path"));
    }

    #[test]
    fn removal_transaction_restores_registration_after_late_failure() {
        let dll = Path::new("immutable.dll");

        let mut profile_failure = FakeBackend::failing([RegistrationAction::UnregisterProfile]);
        unregister_transaction(&mut profile_failure, dll).unwrap_err();
        assert_eq!(
            profile_failure.calls,
            [
                RegistrationAction::UnregisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::RegisterCategory,
            ]
        );

        let mut service_failure = FakeBackend::failing([RegistrationAction::UnregisterTextService]);
        unregister_transaction(&mut service_failure, dll).unwrap_err();
        assert_eq!(
            service_failure.calls,
            [
                RegistrationAction::UnregisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
            ]
        );

        let mut com_failure = FakeBackend::failing([RegistrationAction::UnregisterCom]);
        unregister_transaction(&mut com_failure, dll).unwrap_err();
        assert_eq!(
            com_failure.calls,
            [
                RegistrationAction::UnregisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::UnregisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
            ]
        );

        let mut verify_failure = FakeBackend::failing([RegistrationAction::VerifyUnregistered]);
        unregister_transaction(&mut verify_failure, dll).unwrap_err();
        assert_eq!(
            verify_failure.calls,
            [
                RegistrationAction::UnregisterCategory,
                RegistrationAction::UnregisterProfile,
                RegistrationAction::UnregisterTextService,
                RegistrationAction::UnregisterCom,
                RegistrationAction::VerifyUnregistered,
                RegistrationAction::RegisterCom,
                RegistrationAction::RegisterTextService,
                RegistrationAction::RegisterProfile,
                RegistrationAction::RegisterCategory,
            ]
        );
    }

    #[test]
    fn current_user_enable_verifies_requested_state_and_recovers_to_disabled() {
        let mut success = FakeProfileToggleBackend::default();
        enable_profile_transaction(&mut success).unwrap();
        assert_eq!(
            success.calls,
            [
                ProfileTransitionAction::Enable,
                ProfileTransitionAction::VerifyEnabled,
            ]
        );

        let mut enable_failure =
            FakeProfileToggleBackend::failing([ProfileTransitionAction::Enable]);
        let error = enable_profile_transaction(&mut enable_failure).unwrap_err();
        assert_eq!(error.failed, ProfileTransitionAction::Enable);
        assert!(error.recovery_failed.is_empty());
        assert_eq!(
            enable_failure.calls,
            [
                ProfileTransitionAction::Enable,
                ProfileTransitionAction::Disable,
                ProfileTransitionAction::VerifyDisabled,
            ]
        );
        assert!(
            error
                .to_string()
                .contains("safe disabled state was restored")
        );

        let mut verify_failure =
            FakeProfileToggleBackend::failing([ProfileTransitionAction::VerifyEnabled]);
        let error = enable_profile_transaction(&mut verify_failure).unwrap_err();
        assert_eq!(error.failed, ProfileTransitionAction::VerifyEnabled);
        assert!(error.recovery_failed.is_empty());
        assert_eq!(
            verify_failure.calls,
            [
                ProfileTransitionAction::Enable,
                ProfileTransitionAction::VerifyEnabled,
                ProfileTransitionAction::Disable,
                ProfileTransitionAction::VerifyDisabled,
            ]
        );
    }

    #[test]
    fn current_user_disable_retries_toward_the_safe_state() {
        let mut success = FakeProfileToggleBackend::default();
        disable_profile_transaction(&mut success).unwrap();
        assert_eq!(
            success.calls,
            [
                ProfileTransitionAction::Disable,
                ProfileTransitionAction::VerifyDisabled,
            ]
        );

        let mut first_disable_failure =
            FakeProfileToggleBackend::failing_once(ProfileTransitionAction::Disable);
        let error = disable_profile_transaction(&mut first_disable_failure).unwrap_err();
        assert_eq!(error.failed, ProfileTransitionAction::Disable);
        assert!(error.recovery_failed.is_empty());
        assert_eq!(
            first_disable_failure.calls,
            [
                ProfileTransitionAction::Disable,
                ProfileTransitionAction::Disable,
                ProfileTransitionAction::VerifyDisabled,
            ]
        );

        let mut recovery_failure = FakeProfileToggleBackend::failing([
            ProfileTransitionAction::VerifyEnabled,
            ProfileTransitionAction::VerifyDisabled,
        ]);
        let error = enable_profile_transaction(&mut recovery_failure).unwrap_err();
        assert_eq!(
            error.recovery_failed,
            [ProfileTransitionAction::VerifyDisabled]
        );
        assert!(
            error
                .to_string()
                .contains("recovery to the disabled state is incomplete")
        );
    }

    #[derive(Default)]
    struct FakeProfileToggleBackend {
        calls: Vec<ProfileTransitionAction>,
        failures: BTreeSet<ProfileTransitionAction>,
        fail_once: Option<ProfileTransitionAction>,
    }

    impl FakeProfileToggleBackend {
        fn failing<const N: usize>(failures: [ProfileTransitionAction; N]) -> Self {
            Self {
                calls: Vec::new(),
                failures: failures.into_iter().collect(),
                fail_once: None,
            }
        }

        fn failing_once(action: ProfileTransitionAction) -> Self {
            Self {
                calls: Vec::new(),
                failures: BTreeSet::new(),
                fail_once: Some(action),
            }
        }

        fn call(&mut self, action: ProfileTransitionAction) -> Result<(), ProfileTransitionAction> {
            self.calls.push(action);
            if self.fail_once == Some(action) {
                self.fail_once = None;
                Err(action)
            } else if self.failures.contains(&action) {
                Err(action)
            } else {
                Ok(())
            }
        }
    }

    impl ProfileToggleBackend for FakeProfileToggleBackend {
        fn enable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
            self.call(ProfileTransitionAction::Enable)
        }

        fn verify_profile_enabled(&mut self) -> Result<(), ProfileTransitionAction> {
            self.call(ProfileTransitionAction::VerifyEnabled)
        }

        fn disable_profile(&mut self) -> Result<(), ProfileTransitionAction> {
            self.call(ProfileTransitionAction::Disable)
        }

        fn verify_profile_disabled(&mut self) -> Result<(), ProfileTransitionAction> {
            self.call(ProfileTransitionAction::VerifyDisabled)
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        calls: Vec<RegistrationAction>,
        failures: BTreeSet<RegistrationAction>,
    }

    impl FakeBackend {
        fn failing<const N: usize>(failures: [RegistrationAction; N]) -> Self {
            Self {
                calls: Vec::new(),
                failures: failures.into_iter().collect(),
            }
        }

        fn call(&mut self, action: RegistrationAction) -> Result<(), RegistrationAction> {
            self.calls.push(action);
            if self.failures.contains(&action) {
                Err(action)
            } else {
                Ok(())
            }
        }
    }

    impl RegistrationBackend for FakeBackend {
        fn register_com(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::RegisterCom)
        }

        fn register_text_service(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::RegisterTextService)
        }

        fn register_profile(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::RegisterProfile)
        }

        fn register_category(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::RegisterCategory)
        }

        fn verify_registered(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::VerifyRegistered)
        }

        fn unregister_category(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::UnregisterCategory)
        }

        fn unregister_profile(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::UnregisterProfile)
        }

        fn unregister_text_service(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::UnregisterTextService)
        }

        fn unregister_com(&mut self, _dll: &Path) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::UnregisterCom)
        }

        fn verify_unregistered(&mut self) -> Result<(), RegistrationAction> {
            self.call(RegistrationAction::VerifyUnregistered)
        }
    }

    fn synthetic_pe(exports: &[&str], with_certificate: bool) -> Vec<u8> {
        let mut bytes = vec![0_u8; 0x800];
        bytes[0..2].copy_from_slice(b"MZ");
        put_u32(&mut bytes, 0x3c, 0x80);
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        put_u16(&mut bytes, coff, IMAGE_FILE_MACHINE_AMD64);
        put_u16(&mut bytes, coff + 2, 1);
        put_u16(&mut bytes, coff + 16, 0x00f0);
        put_u16(&mut bytes, coff + 18, IMAGE_FILE_DLL);

        let optional = coff + 20;
        put_u16(&mut bytes, optional, PE32_PLUS_MAGIC);
        put_u32(&mut bytes, optional + 108, 16);
        put_u32(&mut bytes, optional + 112, 0x1000);
        put_u32(&mut bytes, optional + 116, 0x200);
        if with_certificate {
            put_u32(&mut bytes, optional + 112 + 4 * 8, 0x700);
            put_u32(&mut bytes, optional + 116 + 4 * 8, 8);
        }

        let section = optional + 0x00f0;
        bytes[section..section + 8].copy_from_slice(b".rdata\0\0");
        put_u32(&mut bytes, section + 8, 0x500);
        put_u32(&mut bytes, section + 12, 0x1000);
        put_u32(&mut bytes, section + 16, 0x500);
        put_u32(&mut bytes, section + 20, 0x200);

        let export_directory = 0x200;
        put_u32(
            &mut bytes,
            export_directory + 24,
            u32::try_from(exports.len()).unwrap(),
        );
        put_u32(&mut bytes, export_directory + 32, 0x1040);
        let names_table = 0x240;
        let mut name_offset = 0x250;
        for (index, name) in exports.iter().enumerate() {
            let rva = 0x1000_u32 + u32::try_from(name_offset - 0x200).unwrap();
            put_u32(&mut bytes, names_table + index * 4, rva);
            bytes[name_offset..name_offset + name.len()].copy_from_slice(name.as_bytes());
            bytes[name_offset + name.len()] = 0;
            name_offset += name.len() + 1;
        }
        bytes
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
