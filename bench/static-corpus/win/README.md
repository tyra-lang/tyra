# Windows-specific Static Corpus

This directory contains corpus notes and guidance specific to the Windows
target (`x86_64-pc-windows-msvc`).

## Purpose

Document Windows-specific behavior that affects compilation or runtime, and
provide a reference for test authors who need to verify Windows compatibility.

## Path separator handling

Tyra source code does **not** need to handle path separator differences between
Windows (`\`) and Unix (`/`) at the language level.  Path manipulation in the
Tyra standard library (`stdlib/fs.tyra`) is delegated to `std::path::PathBuf`
in the Rust runtime, which transparently normalizes separators on each platform.

Consequence: a Tyra program that constructs or consumes file paths is portable
without any platform-specific branching in the source code.  No dedicated
corpus file is required to exercise this behavior — the existing
`bench/static-corpus/` positive corpus files compile and run identically on
Windows once the toolchain is set up (see `README.md` § Platform support).

## Adding Windows-specific corpus files

If a future change introduces genuinely Windows-specific behavior at the
*language* level (e.g., a Windows-only stdlib module, a platform conditional
syntax), add a `.tyra` corpus file here with the standard header:

```tyra
# SPEC_REF: §<section> — <description>
```

And register it in `bench/static-corpus/check.sh` with a platform guard:

```bash
if [ "$(uname -s)" = "Windows_NT" ] || [ -n "$WINDOWS_CI" ]; then
  # run Windows-specific corpus checks
fi
```

## Toolchain prerequisites (Windows)

| Tool | Source | Notes |
|------|--------|-------|
| `llc.exe` / `lld-link.exe` | LLVM 21 installer | Required for `tyra build` |
| `gc.lib` / `gc.dll` | vcpkg `bdwgc:x64-windows` | Boehm GC, ADR-0007 |
| `tyra_runtime.lib` | `cargo build --workspace` | Built alongside `tyra.exe` |

See `README.md` § Platform support (single source of truth for platform status).
Note that `release-gate-windows` in `.github/workflows/release-gate.yml` is
**tracking-only** in v0.8.0: it `cargo check`s the LLVM-free crates and does not
run a full LLVM build or any program in this directory, because the official
LLVM Windows installer omits the dev files required by `llvm-sys`. The files in
this directory are documentation / manual smoke-test fixtures.
