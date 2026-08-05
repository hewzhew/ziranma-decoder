use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    for asset in [
        "app-cat.ico",
        "listening-cat.ico",
        "holding-wish-cat.ico",
        "organizing-cat.ico",
    ] {
        println!("cargo:rerun-if-changed=assets/wishpad/{asset}");
    }
    println!("cargo:rerun-if-changed=assets/wishpad/wishpad.rc");

    if env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(std::ffi::OsStr::new("windows"))
        || env::var_os("CARGO_CFG_TARGET_ENV").as_deref() != Some(std::ffi::OsStr::new("msvc"))
    {
        return;
    }

    let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x64",
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => {
            println!("cargo:warning=cat image resources skipped for this target architecture");
            return;
        }
    };
    let Some(resource_compiler) = find_resource_compiler(architecture) else {
        println!(
            "cargo:warning=Windows resource compiler not found; cat illustrations keep their runtime fallbacks"
        );
        return;
    };
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let asset_dir = manifest.join("assets").join("wishpad");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("build output directory")).join("wishpad.res");
    let status = Command::new(resource_compiler)
        .current_dir(&asset_dir)
        .arg("/nologo")
        .arg("/fo")
        .arg(&output)
        .arg("wishpad.rc")
        .status()
        .expect("run the Windows resource compiler");
    if !status.success() {
        panic!("Windows resource compiler failed for the wishpad image resources");
    }
    println!("cargo:rustc-link-arg-bin=wishpad={}", output.display());
    println!("cargo:rustc-link-arg-cdylib={}", output.display());
}

fn find_resource_compiler(architecture: &str) -> Option<PathBuf> {
    if let Some(explicit) = env::var_os("RC").map(PathBuf::from)
        && explicit.is_file()
    {
        return Some(explicit);
    }

    if let (Some(sdk), Some(version)) = (
        env::var_os("WindowsSdkDir").map(PathBuf::from),
        env::var_os("WindowsSDKVersion"),
    ) {
        let candidate = sdk
            .join("bin")
            .join(version.to_string_lossy().trim_end_matches(['\\', '/']))
            .join(architecture)
            .join("rc.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let program_files = env::var_os("ProgramFiles(x86)")
        .or_else(|| env::var_os("ProgramFiles"))
        .map(PathBuf::from)?;
    newest_sdk_compiler(
        &program_files.join("Windows Kits").join("10").join("bin"),
        architecture,
    )
}

fn newest_sdk_compiler(root: &Path, architecture: &str) -> Option<PathBuf> {
    let mut versions = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect::<Vec<_>>();
    versions.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
    versions
        .into_iter()
        .map(|entry| entry.path().join(architecture).join("rc.exe"))
        .find(|candidate| candidate.is_file())
}
