// Build script for tyra-codegen-llvm.
//
// Detects the installed LLVM major version via `llvm-config --version` and
// warns if it does not match the active Cargo feature (llvm19-1 / llvm21-1 /
// llvm22-1). The actual llvm-sys linking is driven by the Cargo feature; this
// script only produces a helpful diagnostic.
//
// LLVM version → Cargo feature mapping:
//   19 → --features llvm19-1  (Alpine/musl CI)
//   20 → --features llvm20-1
//   21 → --features llvm21-1  (Linux/macOS CI; recommended for Windows)
//   22 → --features llvm22-1  (default; local dev with Homebrew LLVM 22)
//
// Windows note: LLVM 21 is the recommended version for Windows builds.
// Install via winget (`winget install LLVM.LLVM`) or vcpkg, then set
// LLVM_SYS_210_PREFIX to the LLVM installation prefix.

fn main() {
    let detected = detect_llvm_version();
    let selected = selected_feature();

    let detected_feature = match detected {
        Some(19) => Some("llvm19-1"),
        Some(20) => Some("llvm20-1"),
        Some(21) => Some("llvm21-1"),
        Some(22) => Some("llvm22-1"),
        Some(v) => {
            println!(
                "cargo:warning=tyra-codegen-llvm: detected LLVM {v}, which is not in the \
                 supported set (19–22). Build may fail at link time."
            );
            None
        }
        None => {
            println!(
                "cargo:warning=tyra-codegen-llvm: llvm-config not found on PATH. \
                 Ensure LLVM 19–22 is installed. On macOS: brew install llvm@21. \
                 On Windows: winget install LLVM.LLVM or use vcpkg."
            );
            None
        }
    };

    if let (Some(detected_feat), Some(selected_feat)) = (detected_feature, selected)
        && detected_feat != selected_feat
    {
        println!(
            "cargo:warning=tyra-codegen-llvm: detected LLVM feature `{detected_feat}` \
             but Cargo feature `{selected_feat}` is active. Pass \
             `--no-default-features --features {detected_feat}` to match the installed LLVM."
        );
    }

    // Re-run if LLVM environment changes.
    println!("cargo:rerun-if-env-changed=PATH");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_220_PREFIX");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_211_PREFIX");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_210_PREFIX");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_191_PREFIX");
}

fn detect_llvm_version() -> Option<u32> {
    // On Windows, try Windows-specific paths first before falling back to PATH.
    #[cfg(target_os = "windows")]
    for ver in [22u32, 21, 20, 19] {
        if let Some(p) = find_llvm_windows(ver) {
            if let Ok(out) = std::process::Command::new(&p).arg("--version").output()
                && out.status.success()
                && let Ok(s) = std::str::from_utf8(&out.stdout)
                && let Some(major) = s
                    .split('.')
                    .next()
                    .and_then(|n| n.trim().parse::<u32>().ok())
            {
                return Some(major);
            }
        }
    }

    let candidates = [
        "llvm-config",
        "llvm-config-22",
        "llvm-config-21",
        "llvm-config-20",
        "llvm-config-19",
    ];
    for cmd in &candidates {
        if let Ok(out) = std::process::Command::new(cmd).arg("--version").output()
            && out.status.success()
            && let Ok(s) = std::str::from_utf8(&out.stdout)
            && let Some(major) = s
                .split('.')
                .next()
                .and_then(|n| n.trim().parse::<u32>().ok())
        {
            return Some(major);
        }
    }
    None
}

/// Search for `llvm-config.exe` on Windows using common install locations.
///
/// Search order (highest priority first):
/// 1. `LLVM_SYS_{ver*10}_PREFIX` environment variable (e.g. LLVM_SYS_210_PREFIX for LLVM 21)
/// 2. vcpkg installed path (`vcpkg_installed/x64-windows/bin/`)
/// 3. Standard Windows installer path (`C:\Program Files\LLVM\bin\`)
/// 4. PATH lookup via `where llvm-config`
///
/// Note: LLVM 21 is the recommended Windows build target for Tyra.
#[cfg(target_os = "windows")]
fn find_llvm_windows(ver: u32) -> Option<std::path::PathBuf> {
    // 1. LLVM_SYS_{ver*10}_PREFIX env var (e.g., LLVM_SYS_210_PREFIX for LLVM 21)
    let env_key = format!("LLVM_SYS_{}_PREFIX", ver * 10);
    if let Ok(prefix) = std::env::var(&env_key) {
        let p = std::path::Path::new(&prefix)
            .join("bin")
            .join("llvm-config.exe");
        if p.exists() {
            return Some(p);
        }
    }
    // 2. vcpkg installed path (x64-windows triplet)
    let vcpkg_path = std::path::PathBuf::from("vcpkg_installed/x64-windows/bin/llvm-config.exe");
    if vcpkg_path.exists() {
        return Some(vcpkg_path);
    }
    // 3. Standard Windows install path (LLVM installer default)
    let standard = std::path::PathBuf::from(r"C:\Program Files\LLVM\bin\llvm-config.exe");
    if standard.exists() {
        return Some(standard);
    }
    // 4. PATH lookup via `where llvm-config`
    if let Ok(output) = std::process::Command::new("where")
        .arg("llvm-config")
        .output()
        && output.status.success()
        && let Ok(s) = std::str::from_utf8(&output.stdout)
    {
        let p = std::path::PathBuf::from(s.trim());
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Returns the active inkwell LLVM version feature name, if any.
fn selected_feature() -> Option<&'static str> {
    // Cargo sets CARGO_FEATURE_<NAME> for each active feature.
    if std::env::var("CARGO_FEATURE_LLVM22_1").is_ok() {
        Some("llvm22-1")
    } else if std::env::var("CARGO_FEATURE_LLVM21_1").is_ok() {
        Some("llvm21-1")
    } else if std::env::var("CARGO_FEATURE_LLVM20_1").is_ok() {
        Some("llvm20-1")
    } else if std::env::var("CARGO_FEATURE_LLVM19_1").is_ok() {
        Some("llvm19-1")
    } else {
        None
    }
}
