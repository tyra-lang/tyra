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
//   21 → --features llvm21-1  (Linux/macOS CI)
//   22 → --features llvm22-1  (default; local dev with Homebrew LLVM 22)

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
                 Ensure LLVM 19–22 is installed. On macOS: brew install llvm@21"
            );
            None
        }
    };

    if let (Some(detected_feat), Some(selected_feat)) = (detected_feature, selected) {
        if detected_feat != selected_feat {
            println!(
                "cargo:warning=tyra-codegen-llvm: detected LLVM feature `{detected_feat}` \
                 but Cargo feature `{selected_feat}` is active. Pass \
                 `--no-default-features --features {detected_feat}` to match the installed LLVM."
            );
        }
    }

    // Re-run if LLVM environment changes.
    println!("cargo:rerun-if-env-changed=PATH");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_220_PREFIX");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_211_PREFIX");
    println!("cargo:rerun-if-env-changed=LLVM_SYS_191_PREFIX");
}

fn detect_llvm_version() -> Option<u32> {
    let candidates = [
        "llvm-config",
        "llvm-config-22",
        "llvm-config-21",
        "llvm-config-20",
        "llvm-config-19",
    ];
    for cmd in &candidates {
        if let Ok(out) = std::process::Command::new(cmd).arg("--version").output() {
            if out.status.success() {
                if let Ok(s) = std::str::from_utf8(&out.stdout) {
                    if let Some(major) = s
                        .split('.')
                        .next()
                        .and_then(|n| n.trim().parse::<u32>().ok())
                    {
                        return Some(major);
                    }
                }
            }
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
