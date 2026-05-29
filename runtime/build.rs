// Tyra runtime links against Boehm GC (libgc, ADR-0007) unconditionally.
// Worker threads created by `tyra_task_spawn` register with the collector
// so their stacks participate in conservative scans; see `mod gc` in
// `src/lib.rs`.
//
// Search-path resolution, in order:
//
// Windows:
//   1. vcpkg manifest mode — reads vcpkg.json at repo root and resolves
//      `bdwgc` via the `vcpkg` crate (VCPKG_ROOT must be set in the
//      environment; GitHub Actions sets this when using lukka/run-vcpkg).
//   2. LIBGC_PREFIX env var — manual override; sets native search path and
//      links `gc` dynamically.
//   3. panic — fail fast with a clear message.
//
// macOS / Linux:
//   1. `pkg-config --libs-only-L bdw-gc` — the standard idiom on Linux
//      distros that ship `libgc-dev` / `gc-devel` / `bdw-gc` packages.
//      Works under Alpine, custom prefixes, and CI Docker images where
//      the library lives outside the default linker search path.
//   2. Homebrew prefixes on macOS (`/opt/homebrew/opt/bdw-gc` for Apple
//      Silicon, `/usr/local/opt/bdw-gc` for Intel). `pkg-config` is
//      usually absent on fresh macOS installs, so this fallback keeps
//      the build ergonomic.
//   3. None — rely on the system default search path. Fine for Debian /
//      Ubuntu `/usr/lib/x86_64-linux-gnu` and similar.
//
// Regardless of path resolution, `-lgc` is always emitted on non-Windows.

use std::process::Command;

fn main() {
    // Windows: try vcpkg first, then fall back to LIBGC_PREFIX env var.
    // The `vcpkg` crate reads VCPKG_ROOT and emits the appropriate
    // cargo:rustc-link-search / cargo:rustc-link-lib directives when it
    // succeeds, so we return immediately without emitting anything extra.
    #[cfg(target_os = "windows")]
    {
        // Try vcpkg first (manifest-mode: vcpkg.json at repo root)
        if vcpkg::find_package("bdwgc").is_ok() {
            return;
        }
        // Fallback: LIBGC_PREFIX env var
        if let Ok(prefix) = std::env::var("LIBGC_PREFIX") {
            println!("cargo:rustc-link-search=native={}/lib", prefix);
            println!("cargo:rustc-link-lib=dylib=gc");
            return;
        }
        panic!("Windows: bdwgc not found. Install via vcpkg or set LIBGC_PREFIX");
    }

    // macOS / Linux path (unchanged)
    #[cfg(not(target_os = "windows"))]
    {
        if probe_pkg_config() {
            println!("cargo:rustc-link-lib=gc");
            return;
        }
        for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
            let lib = format!("{prefix}/lib");
            if std::path::Path::new(&lib).is_dir() {
                println!("cargo:rustc-link-search={lib}");
                break;
            }
        }
        println!("cargo:rustc-link-lib=gc");
    }
}

/// Probe `pkg-config --libs-only-L bdw-gc`. Emits any `-L<dir>` flags as
/// `cargo:rustc-link-search=<dir>` and returns true on success.
fn probe_pkg_config() -> bool {
    let output = match Command::new("pkg-config")
        .args(["--libs-only-L", "bdw-gc"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for flag in text.split_whitespace() {
        if let Some(path) = flag.strip_prefix("-L") {
            println!("cargo:rustc-link-search={path}");
        }
    }
    true
}
