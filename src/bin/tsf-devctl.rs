//! Read-only inspection of the build-only Windows TSF alpha.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_DLL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EXPORT_NAMES: usize = 4_096;
const MAX_EXPORT_NAME_BYTES: usize = 256;
const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_DLL: u16 = 0x2000;
const PE32_PLUS_MAGIC: u16 = 0x020b;
const REQUIRED_COM_EXPORTS: [&str; 2] = ["DllCanUnloadNow", "DllGetClassObject"];
const REGISTRATION_EXPORTS: [&str; 2] = ["DllRegisterServer", "DllUnregisterServer"];

#[derive(Debug, Eq, PartialEq)]
enum Options {
    Help,
    Inspect { dll: PathBuf },
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

#[derive(Clone, Copy)]
struct PeSection {
    virtual_size: u32,
    virtual_address: u32,
    raw_size: u32,
    raw_offset: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_options(std::env::args().skip(1))? {
        Options::Help => print_usage(),
        Options::Inspect { dll } => inspect(&dll)?,
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
    if command != "inspect" {
        return Err("unknown tsf-devctl command; value was suppressed".into());
    }

    let mut dll = None;
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
            "--help" | "-h" => return Err("--help must be used by itself".into()),
            _ => return Err("unknown inspect argument; value was suppressed".into()),
        }
    }

    Ok(Options::Inspect {
        dll: dll.ok_or("inspect requires exactly one --dll path")?,
    })
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --bin tsf-devctl -- inspect --dll \
         target/release/ziranma_core.dll"
    );
    eprintln!(
        "Reads one explicitly named DLL, the fixed COM registration locations, and the current \
         TSF profile list. It does not register, unregister, activate, write files, or use the \
         network."
    );
}

fn inspect(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = read_explicit_dll(path)?;
    let image = inspect_pe(&bytes)?;
    validate_alpha_dll(&image)?;
    let registration = inspect_com_registration()?;
    let profile = inspect_system_profile()?;
    print!("{}", render_report(path, &image, registration, profile));
    Ok(())
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

fn render_report(
    path: &Path,
    image: &PeInspection,
    registration: ComRegistrationStatus,
    profile: ProfileStatus,
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
    writeln!(output, "本次操作：只读").unwrap();
    output
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

    let guid = TSF_ALPHA_CLSID;
    format!(
        "Software\\Classes\\CLSID\\{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-\
         {:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}\\InprocServer32",
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
        guid.data4[7]
    )
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
        GUID_TFCAT_TIP_KEYBOARD, TF_IPP_FLAG_ACTIVE, TF_IPP_FLAG_ENABLED,
        TF_PROFILETYPE_INPUTPROCESSOR,
    };
    use ziranma_core::{TSF_ALPHA_CLSID, TSF_ALPHA_LANGID, TSF_ALPHA_PROFILE_GUID};

    (profile.dwProfileType == TF_PROFILETYPE_INPUTPROCESSOR
        && profile.langid == TSF_ALPHA_LANGID
        && profile.clsid == TSF_ALPHA_CLSID
        && profile.guidProfile == TSF_ALPHA_PROFILE_GUID)
        .then_some(ProfileStatus {
            registered: true,
            enabled: profile.dwFlags & TF_IPP_FLAG_ENABLED != 0,
            active: profile.dwFlags & TF_IPP_FLAG_ACTIVE != 0,
            keyboard_category: profile.catid == GUID_TFCAT_TIP_KEYBOARD,
        })
}

#[cfg(windows)]
fn inspect_system_profile() -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::TextServices::{
        CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr, TF_INPUTPROCESSORPROFILE,
    };
    use windows::core::IUnknown;
    use ziranma_core::TSF_ALPHA_LANGID;

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
    // SAFETY: requests the documented TSF profile-manager interface from the
    // system in-process server; no mutating method is called below.
    let manager: ITfInputProcessorProfileMgr = unsafe {
        CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None::<&IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: enumeration is read-only and restricted to the fixed zh-CN id.
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
    Ok(found.unwrap_or_default())
}

#[cfg(not(windows))]
fn inspect_system_profile() -> Result<ProfileStatus, Box<dyn std::error::Error>> {
    Err("TSF profile inspection requires Windows".into())
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
    fn parser_requires_one_explicit_dll() {
        assert_eq!(parse_options(Vec::<String>::new()).unwrap(), Options::Help);
        assert_eq!(
            parse_options(["inspect", "--dll", "alpha.dll"].map(str::to_owned)).unwrap(),
            Options::Inspect {
                dll: PathBuf::from("alpha.dll")
            }
        );
        assert!(parse_options(["inspect"].map(str::to_owned)).is_err());
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
        );
        assert!(report.contains("证书目录：存在（未验证签名有效性）"));
        assert!(report.contains("COM 注册：未发现"));
        assert!(report.contains("系统语言配置：未发现"));
        assert!(report.contains("本次操作：只读"));
        assert!(!report.contains("下一步"));
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
            TF_IPP_FLAG_ENABLED, TF_PROFILETYPE_INPUTPROCESSOR,
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
                registered: true,
                enabled: true,
                active: true,
                keyboard_category: true,
            })
        );

        let mut wrong_profile = exact;
        wrong_profile.guidProfile = GUID::from_u128(1);
        assert_eq!(alpha_profile_status(&wrong_profile), None);
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
