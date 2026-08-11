// tyra-driver: Compilation pipeline for the Tyra language.
//
// Pipeline: source -> lex -> parse -> resolve -> type check -> MIR -> LLVM IR -> binary
//
// spec reference: §19 (execution model)

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use tyra_frontend::{DepErrorKind, ImportLoader, ImportResolution};

static BINARY_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub use tyra_ast::SourceFile;
pub use tyra_diagnostics::SourceId;
use tyra_diagnostics::{Report, SourceMap};
pub use tyra_resolve::{CompletionKind, DefIndex, SymbolList};
pub use tyra_resolve::{PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
pub use tyra_types::{Ty, TypeIndex};

/// Backward-compatible alias: callers that hold `tyra_driver::CheckResult` now
/// get `tyra_frontend::CheckOutput` (structurally identical).
pub use tyra_frontend::CheckOutput as CheckResult;

/// `is_builtin_module` is now in tyra-frontend; re-export so existing callers
/// (LSP, tyra-pkg) continue to compile without changes.
pub use tyra_frontend::is_builtin_module;

// ─────────────────────────────────────────────────────────────────────────────
// Public result types
// ─────────────────────────────────────────────────────────────────────────────

/// Result of compilation.
pub struct CompileResult {
    pub success: bool,
    pub report: Report,
    pub sources: SourceMap,
    pub llvm_ir: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Filesystem import loader (native, ADR 0010 three-layer rule)
// ─────────────────────────────────────────────────────────────────────────────

/// Import loader that resolves module paths from the filesystem.
///
/// Implements the ADR 0010 three-layer uniqueness rule:
///   (a) `<project_root>/src/` or `<main_dir>/` for script-mode
///   (b) path / git dependencies from `Tyra.toml`
///   (c) stdlib (discovered via `find_stdlib_dir`)
///
/// Returns an [`ImportResolution`] variant; all diagnostic emission is done
/// by the frontend (not here).
pub struct FsImportLoader {
    pub main_dir: std::path::PathBuf,
}

impl ImportLoader for FsImportLoader {
    fn resolve(&self, path: &[String]) -> ImportResolution {
        let project_root = tyra_manifest::find_project_root(&self.main_dir);
        let manifest = project_root
            .as_deref()
            .and_then(|r| tyra_manifest::load_manifest(r).ok());
        let local_base: std::path::PathBuf = project_root
            .as_deref()
            .map(|r| {
                let src = r.join("src");
                if src.is_dir() { src } else { r.to_path_buf() }
            })
            .unwrap_or_else(|| self.main_dir.to_path_buf());

        let mut dep_errors: Vec<(String, &'static str)> = Vec::new();
        let candidates = collect_import_candidates(
            path,
            &local_base,
            manifest.as_ref(),
            project_root.as_deref(),
            &self.main_dir,
            &mut dep_errors,
        );

        // dep_errors take priority; return the first one (multiple dep errors
        // for a single import path are extremely rare in practice).
        if let Some((dep, kind)) = dep_errors.into_iter().next() {
            return ImportResolution::DepError {
                dep,
                kind: if kind == "bin" {
                    DepErrorKind::BinPackage
                } else {
                    DepErrorKind::NameMismatch
                },
            };
        }

        match candidates.len() {
            0 => ImportResolution::NotFound,
            1 => {
                let module_path = candidates.into_iter().next().unwrap();
                match std::fs::read_to_string(&module_path) {
                    Ok(source) => ImportResolution::Found {
                        display_name: module_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        source,
                    },
                    Err(e) => ImportResolution::ReadError {
                        detail: format!("cannot read `{}`: {e}", module_path.display()),
                    },
                }
            }
            _ => ImportResolution::Ambiguous {
                locations: candidates
                    .iter()
                    .map(|p| format!("`{}`", p.display()))
                    .collect(),
            },
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API: check / compile
// ─────────────────────────────────────────────────────────────────────────────

/// Check a Tyra source supplied as an in-memory string.
///
/// Runs lex → parse → auto-import → rename → (optional) import-resolve
/// → name-resolve → type-check.  Stops before MIR / LLVM codegen.
///
/// `SymbolList` is a flat list of all user-defined names collected by the
/// resolver, used by the LSP completion handler.
///
/// If `workspace_dir` is `None`, filesystem import resolution is
/// skipped (suitable for LSP single-file diagnostics).
pub fn check_in_memory(
    file_name: String,
    source: String,
    workspace_dir: Option<&Path>,
) -> CheckResult {
    if let Some(dir) = workspace_dir {
        let loader = FsImportLoader {
            main_dir: dir.to_path_buf(),
        };
        tyra_frontend::check_unit(&file_name, &source, &loader)
    } else {
        tyra_frontend::check_unit(&file_name, &source, &tyra_frontend::NoImportLoader)
    }
}

/// Compile a Tyra source file to LLVM IR text (debug build, with DWARF).
pub fn compile_to_ir(source_path: &Path) -> CompileResult {
    compile_to_ir_impl(source_path, true, false).0
}

/// Core compilation pipeline: read source → frontend (parse → check → MIR) → codegen.
///
/// `debug_info = true` emits DWARF metadata for lldb (ADR-0014 §4a).
/// `coverage = true` uses `emit_llvm_ir_coverage` and returns the covmap text.
///
/// Also returns the lowered MIR `Program` on success, so binary-producing
/// callers (`compile_to_binary_opts`, `compile_to_binary_coverage`) can emit a
/// native object file from the *same* MIR via `tyra_codegen_llvm::emit_object*`
/// without re-running the frontend. The `CompileResult.llvm_ir` text field is
/// still populated exactly as before — only the object-emitting callers'
/// linking input changes from `.ll` text to a `.o` file.
///
/// The frontend pipeline (all AST passes, import resolution via `FsImportLoader`,
/// name resolution, type checking, MIR lowering) is now in `tyra-frontend` and
/// invoked as a single `compile_unit` call.
fn compile_to_ir_impl(
    source_path: &Path,
    debug_info: bool,
    coverage: bool,
) -> (CompileResult, Option<String>, Option<tyra_mir::Program>) {
    let mut boot_report = Report::new();

    // Read source file.
    let source = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            boot_report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot read file `{}`: {e}",
                    source_path.display()
                ))
                .with_code("E0001"),
            );
            return (
                CompileResult {
                    success: false,
                    report: boot_report,
                    sources: SourceMap::new(),
                    llvm_ir: None,
                },
                None,
                None,
            );
        }
    };

    let main_name: String = source_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into();
    let main_dir = source_path.parent().unwrap_or(Path::new("."));
    let loader = FsImportLoader {
        main_dir: main_dir.to_path_buf(),
    };

    // Frontend: parse → auto-import → desugar → import-resolve → name-resolve
    //           → type-check → desugar_err_main → MIR lowering.
    let fr = tyra_frontend::compile_unit(&main_name, &source, &loader);
    let sources = fr.check.sources;
    let mut report = fr.check.report;

    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
            None,
        );
    }

    let mir = fr
        .program
        .expect("compile_unit returned None program without errors — this is a bug");

    // ICE guard (E9001): refuse to invoke LLVM codegen on a MIR that still
    // carries `Ty::Error` or unresolved `Ty::Var`. Such types signal a bug in
    // the type checker; without this guard they would crash LLVM with an opaque
    // type-mismatch (the previous E0500 LLVM-IR failure mode).
    if let Err(diags) = tyra_codegen_llvm::check_no_type_errors(&mir) {
        for d in diags {
            report.add(d);
        }
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
            None,
        );
    }

    // LLVM IR generation.
    if coverage {
        let (llvm_ir, covmap_text) = tyra_codegen_llvm::emit_llvm_ir_coverage(&mir);
        (
            CompileResult {
                success: true,
                report,
                sources,
                llvm_ir: Some(llvm_ir),
            },
            Some(covmap_text),
            Some(mir),
        )
    } else if debug_info {
        let llvm_ir = tyra_codegen_llvm::emit_llvm_ir_debug(&mir);
        (
            CompileResult {
                success: true,
                report,
                sources,
                llvm_ir: Some(llvm_ir),
            },
            None,
            Some(mir),
        )
    } else {
        let llvm_ir = tyra_codegen_llvm::emit_llvm_ir(&mir);
        (
            CompileResult {
                success: true,
                report,
                sources,
                llvm_ir: Some(llvm_ir),
            },
            None,
            Some(mir),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Filesystem helpers (stdlib / runtime discovery)
// ─────────────────────────────────────────────────────────────────────────────

/// Confirm that `dir` is a real Tyra stdlib tree, not a user directory named "stdlib".
/// Uses `assert.ty` as a stable sentinel — it is part of the Tyra stdlib by spec.
fn is_tyra_stdlib(dir: &Path) -> bool {
    dir.is_dir() && dir.join("assert.ty").is_file()
}

/// Find the stdlib directory. Resolution order:
///
/// 1. `TYRA_STDLIB` environment variable (highest priority; useful in CI and custom installs)
/// 2. `<exe_dir>/stdlib/` — portable distribution: stdlib shipped next to the binary
/// 3. `<exe_dir>/../lib/tyra/stdlib/` — FHS install: `/usr/local/bin/tyra` + `/usr/local/lib/tyra/stdlib/`
/// 4. Walk up from `<exe_dir>` — dev checkout (`target/debug/tyra` → repo root → `stdlib/`)
/// 5. Walk up from `main_dir` — script mode without a known binary location
///
/// Each candidate is validated with [`is_tyra_stdlib`] to avoid false positives.
fn find_stdlib_dir(main_dir: &Path) -> Option<std::path::PathBuf> {
    // 1. Explicit env override.
    if let Ok(p) = std::env::var("TYRA_STDLIB") {
        let pb = std::path::PathBuf::from(p);
        if is_tyra_stdlib(&pb) {
            return Some(pb);
        }
    }

    // 2–4. Look relative to the running executable.
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        // 2. Portable: stdlib/ next to the binary.
        let beside = exe_dir.join("stdlib");
        if is_tyra_stdlib(&beside) {
            return Some(beside);
        }
        // 3. FHS: ../lib/tyra/stdlib/ relative to the binary directory.
        let fhs = exe_dir.join("..").join("lib").join("tyra").join("stdlib");
        if is_tyra_stdlib(&fhs) {
            return Some(fhs);
        }
        // 4. Walk up from the binary's directory.
        let mut dir = exe_dir.to_path_buf();
        loop {
            let candidate = dir.join("stdlib");
            if is_tyra_stdlib(&candidate) {
                return Some(candidate);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    // 5. Walk up from the source file's directory (script mode / no binary context).
    let mut dir = main_dir.to_path_buf();
    loop {
        let candidate = dir.join("stdlib");
        if is_tyra_stdlib(&candidate) {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Candidate locations for the Tyra runtime staticlib (`libtyra_runtime.a`).
fn runtime_staticlib_candidates() -> Vec<std::path::PathBuf> {
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    vec![
        exe_dir.join("libtyra_runtime.a"),
        exe_dir
            .join("..")
            .join("lib")
            .join("tyra")
            .join("libtyra_runtime.a"),
    ]
}

/// Find the Tyra runtime staticlib: the first existing candidate.
fn find_runtime_staticlib() -> Option<std::path::PathBuf> {
    runtime_staticlib_candidates()
        .into_iter()
        .find(|p| p.exists())
}

/// Human-readable list of the searched staticlib locations, for diagnostics.
fn runtime_staticlib_search_list() -> String {
    let candidates = runtime_staticlib_candidates();
    if candidates.is_empty() {
        return "<unknown>".into();
    }
    candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" or ")
}

// ─────────────────────────────────────────────────────────────────────────────
// Import candidate discovery (filesystem layer — used by FsImportLoader and
// the public resolve_import_file helper)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve `import a.b.c` to the `.ty` file path using the ADR 0010 rule.
///
/// Returns `None` for built-in modules, ambiguous imports (E0217), dep errors,
/// or paths that do not exist on disk. Returns `Some` only when exactly one
/// candidate is found across all three layers.
pub fn resolve_import_file(main_dir: &Path, path: &[String]) -> Option<std::path::PathBuf> {
    if is_builtin_module(&path.join(".")) {
        return None;
    }
    let project_root = tyra_manifest::find_project_root(main_dir);
    let manifest = project_root
        .as_deref()
        .and_then(|r| tyra_manifest::load_manifest(r).ok());
    let local_base: std::path::PathBuf = project_root
        .as_deref()
        .map(|r| {
            let src = r.join("src");
            if src.is_dir() { src } else { r.to_path_buf() }
        })
        .unwrap_or_else(|| main_dir.to_path_buf());
    let mut dep_errors: Vec<(String, &'static str)> = Vec::new();
    let candidates = collect_import_candidates(
        path,
        &local_base,
        manifest.as_ref(),
        project_root.as_deref(),
        main_dir,
        &mut dep_errors,
    );
    if !dep_errors.is_empty() {
        return None;
    }
    match candidates.len() {
        1 => Some(candidates.into_iter().next().unwrap()),
        _ => None,
    }
}

/// Collect all file-system candidates for an import path across three layers
/// (ADR 0010 uniqueness rule).
///
/// `dep_errors` receives `(dep_name, kind)` pairs for dependencies that must be
/// rejected at compile time. `kind` is `"bin"` (ADR 0009 E_DEP_NOT_IMPORTABLE)
/// or `"name-mismatch"` (ADR 0010 no-alias rule).
fn collect_import_candidates(
    path_segs: &[String],
    local_base: &Path,
    manifest: Option<&tyra_manifest::Manifest>,
    project_root: Option<&Path>,
    main_dir: &Path,
    dep_errors: &mut Vec<(String, &'static str)>,
) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    // Layer (a): local_base / a / b / c.ty
    {
        let mut p = local_base.to_path_buf();
        for seg in path_segs {
            p.push(seg);
        }
        p.set_extension("ty");
        if p.is_file() {
            candidates.push(p);
        }
    }

    // Layer (b): dependency whose name matches the first import segment
    if let (Some(m), Some(root)) = (manifest, project_root) {
        let first_seg = path_segs.first().map(String::as_str).unwrap_or("");
        for (dep_name, dep) in &m.dependencies {
            if dep_name.as_str() != first_seg {
                continue;
            }
            let dep_src = match (&dep.path, &dep.git, &dep.rev) {
                (Some(rel), _, _) => {
                    let dep_root = root.join(rel);
                    if is_bin_dep(&dep_root) {
                        dep_errors.push((dep_name.clone(), "bin"));
                        continue;
                    }
                    dep_root.join("src")
                }
                (None, Some(url), Some(rev)) => {
                    let dep_root = git_dep_cache_root(dep_name, url, rev);
                    if let Err(kind) = check_git_dep_root(dep_name, &dep_root) {
                        dep_errors.push((dep_name.clone(), kind));
                        continue;
                    }
                    dep_root.join("src")
                }
                _ => continue,
            };
            let mut dp = dep_src;
            for seg in path_segs {
                dp.push(seg);
            }
            dp.set_extension("ty");
            if dp.is_file() {
                candidates.push(dp);
            }
        }
    }

    // Layer (c): stdlib
    if let Some(stdlib) = find_stdlib_dir(main_dir) {
        let mut sp = stdlib;
        for seg in path_segs {
            sp.push(seg);
        }
        sp.set_extension("ty");
        if sp.is_file() {
            candidates.push(sp);
        }
    }

    candidates
}

/// Returns `true` when the dependency at `dep_root` is a bin package.
fn is_bin_dep(dep_root: &Path) -> bool {
    let Ok(manifest) = tyra_manifest::load_manifest(dep_root) else {
        return false;
    };
    let root_src = dep_root
        .join("src")
        .join(format!("{}.ty", manifest.package.name));
    let Ok(src) = std::fs::read_to_string(&root_src) else {
        return false;
    };
    tyra_manifest::is_bin_source(&src)
}

/// Compile-time validation for a cached git dependency root.
fn check_git_dep_root(dep_name: &str, dep_root: &std::path::Path) -> Result<(), &'static str> {
    let Ok(manifest) = tyra_manifest::load_manifest(dep_root) else {
        return Ok(());
    };
    if manifest.package.name != dep_name {
        return Err("name-mismatch");
    }
    let root_src = dep_root
        .join("src")
        .join(format!("{}.ty", manifest.package.name));
    let Ok(src) = std::fs::read_to_string(&root_src) else {
        return Ok(());
    };
    if tyra_manifest::is_bin_source(&src) {
        return Err("bin");
    }
    Ok(())
}

/// Canonical cache root for a git dependency (mirrors `tyra_pkg::cache_dir_for`).
///
/// `~/.tyra/cache/git/<dep_name>-<url_hash12>/<rev>/`
fn git_dep_cache_root(dep_name: &str, url: &str, rev: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let dir_name = format!("{dep_name}-{}", url_hash_12(url));
    home.join(".tyra")
        .join("cache")
        .join("git")
        .join(dir_name)
        .join(rev)
}

/// 12-character lowercase hex of FNV-1a(url). Mirrors `tyra_pkg::url_hash`.
fn url_hash_12(url: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:012x}", h & 0x0000_ffff_ffff_ffff)
}

// ─────────────────────────────────────────────────────────────────────────────
// Binary compilation
// ─────────────────────────────────────────────────────────────────────────────

/// Build a path for the intermediate object file `emit_object`/
/// `emit_object_coverage` write before linking, guaranteed distinct from
/// `output_path`.
///
/// This used to be `output_path.with_extension("o")`, which collides with
/// `output_path` itself whenever the caller asks for an output that already
/// ends in `.o` (`tyra build prog.ty -o probe.o`): the "temp" object *is*
/// the requested output, so the post-link cleanup (`remove_file(obj_path)`)
/// deletes the freshly linked binary while still reporting success. The same
/// naming also silently clobbers an unrelated pre-existing `<output>.o` in
/// the working directory (e.g. a user's own `foo.o` when building `-o foo`).
///
/// The pid + a per-process counter make the name unique across concurrent
/// `tyra` invocations and repeated calls within one process (driver tests
/// compile many programs in one test binary), and staying under
/// `output_path`'s own directory/stem keeps the intermediate on the same
/// filesystem as the final link target (avoiding a cross-device rename/copy,
/// unlike routing through `TMPDIR`).
fn intermediate_object_path(output_path: &Path) -> Result<std::path::PathBuf, String> {
    // `with_extension` is a no-op when the path has no file name (`/`, `.`,
    // `..`, ``, `dir/..`), which would make the intermediate object *be* the
    // requested output — the collision this helper exists to prevent. Such an
    // output path cannot name a binary anyway, so reject it as a diagnostic
    // rather than letting the invariant assert below fire on user input.
    if output_path.file_name().is_none() {
        return Err(format!(
            "invalid output path `{}`: expected a path naming a file",
            output_path.display()
        ));
    }
    let bin_id = BINARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = output_path.with_extension(format!("tyra-{}-{bin_id}.o", std::process::id()));
    assert_ne!(
        path.as_path(),
        output_path,
        "intermediate object path must never equal the requested output path"
    );
    Ok(path)
}

/// Compile a Tyra source file to a native binary (debug, `-O0`).
pub fn compile_to_binary(source_path: &Path, output_path: &Path) -> CompileResult {
    compile_to_binary_opts(source_path, output_path, false, false)
}

/// Compile a Tyra source file to a native binary (release, `-O2`).
pub fn compile_to_binary_release(source_path: &Path, output_path: &Path) -> CompileResult {
    compile_to_binary_opts(source_path, output_path, true, false)
}

/// Compile a Tyra source file to a fully static native binary (debug, `-O0`).
pub fn compile_to_binary_static(source_path: &Path, output_path: &Path) -> CompileResult {
    compile_to_binary_opts(source_path, output_path, false, true)
}

/// Compile a Tyra source file to a fully static native binary (release, `-O2`).
pub fn compile_to_binary_static_release(source_path: &Path, output_path: &Path) -> CompileResult {
    compile_to_binary_opts(source_path, output_path, true, true)
}

fn compile_to_binary_opts(
    source_path: &Path,
    output_path: &Path,
    release: bool,
    static_link: bool,
) -> CompileResult {
    // Debug builds emit DWARF for lldb; release builds skip debug info (ADR-0014 §4a).
    let (result, _covmap, mir) = if release {
        compile_to_ir_impl(source_path, false, false)
    } else {
        compile_to_ir_impl(source_path, true, false) // includes DWARF
    };
    if !result.success {
        return result;
    }
    let mir =
        mir.expect("compile_to_ir_impl returned success without a MIR program — this is a bug");

    // Emit a native object file directly from the in-process LLVM module via
    // inkwell's TargetMachine — no textual-IR round trip through an external
    // `clang`/`opt` process. `clang` is invoked below only as a *linker* over
    // this object, so it can never choke on IR syntax newer than its own LLVM
    // version (the compiler links against LLVM via inkwell and can emit e.g.
    // `getelementptr inbounds nuw`, LLVM 19+).
    let obj_path = match intermediate_object_path(output_path) {
        Ok(path) => path,
        Err(e) => {
            let mut report = result.report;
            report.add(tyra_diagnostics::Diagnostic::error(e).with_code("E0001"));
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    };
    if let Err(e) = tyra_codegen_llvm::emit_object(&mir, &obj_path, release, !release) {
        // Emission may have written a partial object before failing; never
        // leave it behind.
        let _ = std::fs::remove_file(&obj_path);
        let mut report = result.report;
        report.add(
            tyra_diagnostics::Diagnostic::error(format!(
                "cannot emit object file `{}`: {e}",
                obj_path.display()
            ))
            .with_code("E0001"),
        );
        return CompileResult {
            success: false,
            report,
            sources: result.sources,
            llvm_ir: result.llvm_ir,
        };
    }

    // Route to platform-specific linker invocation.
    #[cfg(target_os = "windows")]
    {
        build_link_cmd_windows(result, &obj_path, output_path, static_link)
    }
    #[cfg(not(target_os = "windows"))]
    {
        build_link_cmd_unix(result, &obj_path, output_path, static_link)
    }
}

/// Unix (Linux/macOS) linker path: link the pre-compiled object with libgc +
/// the runtime staticlib. `clang` is invoked purely as a linker here — the
/// object was already optimized (release) and carries its own DWARF (debug)
/// via `tyra_codegen_llvm::emit_object`, so no `-O*`/`-gdwarf-4` codegen flags
/// apply to a `.o` input (clang would ignore or misapply them).
#[cfg(not(target_os = "windows"))]
fn build_link_cmd_unix(
    result: CompileResult,
    obj_path: &Path,
    output_path: &Path,
    static_link: bool,
) -> CompileResult {
    let mut clang_args: Vec<String> = vec![
        obj_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
    ];
    // libgc: probe common install prefixes.
    for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
        let lib_dir = format!("{prefix}/lib");
        if std::path::Path::new(&lib_dir).is_dir() {
            clang_args.push(format!("-L{lib_dir}"));
            break;
        }
    }
    match find_runtime_staticlib() {
        Some(p) => {
            clang_args.push(p.to_string_lossy().into_owned());
        }
        None => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "Tyra runtime staticlib not found (searched {}).\n\
                     Build the full workspace with `cargo build` (not `-p tyra-cli`).",
                    runtime_staticlib_search_list()
                ))
                .with_code("E0502"),
            );
            let _ = std::fs::remove_file(obj_path);
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    }
    if static_link {
        clang_args.push("-static".into());
        clang_args.push("-lgc".into());
        if cfg!(target_os = "linux") {
            clang_args.push("-lpthread".into());
            clang_args.push("-lm".into());
        }
    } else {
        clang_args.push("-lgc".into());
        if cfg!(target_os = "linux") {
            clang_args.push("-lpthread".into());
            clang_args.push("-ldl".into());
            clang_args.push("-lm".into());
        }
    }

    let clang_result = Command::new("clang").args(&clang_args).output();

    // Clean up the temp object file
    let _ = std::fs::remove_file(obj_path);

    match clang_result {
        Ok(output) => {
            if output.status.success() {
                result
            } else {
                let mut report = result.report;
                let stderr = String::from_utf8_lossy(&output.stderr);
                let msg = if stderr.contains("-lgc")
                    || stderr.contains("library 'gc'")
                    || stderr.contains("cannot find -lgc")
                {
                    format!(
                        "libgc (Boehm GC) not found. Install with:\n  \
                         macOS: brew install bdw-gc\n  \
                         Debian/Ubuntu: apt install libgc-dev\n\n\
                         Original linker error:\n{stderr}"
                    )
                } else {
                    format!("clang failed: {stderr}")
                };
                report.add(tyra_diagnostics::Diagnostic::error(msg).with_code("E0500"));
                CompileResult {
                    success: false,
                    report,
                    sources: result.sources,
                    llvm_ir: result.llvm_ir,
                }
            }
        }
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot run clang: {e}. Is clang installed?"
                ))
                .with_code("E0500"),
            );
            CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            }
        }
    }
}

/// Windows linker path: LLVM IR -> obj via `llc.exe`, then link via `lld-link.exe`.
#[cfg(target_os = "windows")]
fn build_link_cmd_windows(
    result: CompileResult,
    obj_path: &Path,
    output_path: &Path,
    _static_link: bool,
) -> CompileResult {
    // The object file is already emitted (optimized if `release`, DWARF if
    // debug) by `tyra_codegen_llvm::emit_object` before this function is
    // called — no `llc.exe` IR-to-object step is needed anymore. `lld-link.exe`
    // is invoked purely as a linker below.

    // Step 1: Resolve vcpkg install root.
    let vcpkg_dir: Option<std::path::PathBuf> = std::env::var("VCPKG_ROOT")
        .ok()
        .map(|root| {
            std::path::PathBuf::from(root)
                .join("installed")
                .join("x64-windows")
        })
        .or_else(|| {
            let cwd_candidate = std::path::PathBuf::from("vcpkg_installed").join("x64-windows");
            if cwd_candidate.is_dir() {
                Some(cwd_candidate)
            } else {
                None
            }
        })
        .or_else(|| {
            std::env::var("LIBGC_PREFIX")
                .ok()
                .map(std::path::PathBuf::from)
        });

    // Step 2: Locate tyra_runtime.lib next to the running compiler.
    let runtime_lib_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("tyra_runtime.lib")));

    // Step 3: Build lld-link.exe argument list.
    let exe_path = output_path.with_extension("exe");
    let mut link_args: Vec<String> = vec![
        obj_path.to_str().unwrap().into(),
        format!("/OUT:{}", exe_path.display()),
        "/DEFAULTLIB:gc.lib".into(),
        "/DEFAULTLIB:ucrt.lib".into(),
        "/DEFAULTLIB:msvcrt.lib".into(),
        "/DEFAULTLIB:vcruntime.lib".into(),
        "/DEFAULTLIB:kernel32.lib".into(),
        "/DEFAULTLIB:ole32.lib".into(),
        "/SUBSYSTEM:CONSOLE".into(),
    ];

    if let Some(ref vdir) = vcpkg_dir {
        let lib_dir = vdir.join("lib");
        if lib_dir.is_dir() {
            link_args.push(format!("/LIBPATH:{}", lib_dir.display()));
        }
    }

    match runtime_lib_path.as_ref() {
        Some(p) if p.exists() => {
            link_args.push(p.to_string_lossy().into_owned());
        }
        _ => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "Tyra runtime staticlib not found (expected at {}).\n\
                     Build the full workspace with `cargo build` (not `-p tyra-cli`).",
                    runtime_lib_path
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<unknown>".into())
                ))
                .with_code("E0502"),
            );
            let _ = std::fs::remove_file(obj_path);
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    }

    let link_result = Command::new("lld-link.exe").args(&link_args).output();
    let _ = std::fs::remove_file(obj_path);

    match link_result {
        Ok(output) if !output.status.success() => {
            let mut report = result.report;
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if stderr.contains("gc.lib") || stderr.contains("cannot open input file") {
                format!(
                    "gc.lib (Boehm GC) not found. Install via vcpkg:\n  \
                     vcpkg install bdw-gc:x64-windows\n\n\
                     Original linker error:\n{stderr}"
                )
            } else {
                format!("lld-link.exe failed: {stderr}")
            };
            report.add(tyra_diagnostics::Diagnostic::error(msg).with_code("E0500"));
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot run lld-link.exe: {e}. Is LLVM installed and on PATH?"
                ))
                .with_code("E0500"),
            );
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
        Ok(_) => {} // success — fall through to DLL copy
    }

    // Step 4: Copy gc.dll to the output directory.
    if let Some(ref vdir) = vcpkg_dir {
        let gc_dll_src = vdir.join("bin").join("gc.dll");
        if let Some(out_dir) = exe_path.parent() {
            let gc_dll_dst = out_dir.join("gc.dll");
            if let Err(e) = std::fs::copy(&gc_dll_src, &gc_dll_dst) {
                eprintln!(
                    "warning: could not copy gc.dll to output directory: {e}\n\
                     Ensure gc.dll is on PATH or in the same directory as the output binary.\n\
                     Expected source: {}",
                    gc_dll_src.display()
                );
            }
        }
    } else {
        eprintln!(
            "warning: VCPKG_ROOT not set and vcpkg_installed/ not found; \
             gc.dll was not copied to the output directory. \
             Set VCPKG_ROOT or ensure gc.dll is on PATH."
        );
    }

    result
}

/// Compile a Tyra source file to a binary with coverage instrumentation.
pub fn compile_to_binary_coverage(source_path: &Path, output_path: &Path) -> CompileResult {
    let (result, _covmap_opt, mir) = compile_to_ir_impl(source_path, false, true);
    if !result.success {
        return result;
    }
    let mir = mir.expect(
        "compile_to_ir_impl returned success (coverage) without a MIR program — this is a bug",
    );

    // Emit the object directly (see `compile_to_binary_opts` for why: `clang`
    // is a linker only, never a parser of textual IR whose syntax may be
    // newer than the PATH `clang`'s own LLVM version). `emit_object_coverage`
    // builds the covmap from the same MIR the object is compiled from, so the
    // sidecar always describes exactly what got linked.
    let obj_path = match intermediate_object_path(output_path) {
        Ok(path) => path,
        Err(e) => {
            let mut report = result.report;
            report.add(tyra_diagnostics::Diagnostic::error(e).with_code("E0001"));
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: None,
            };
        }
    };
    let covmap_text = match tyra_codegen_llvm::emit_object_coverage(&mir, &obj_path) {
        Ok(text) => text,
        Err(e) => {
            // Emission may have written a partial object before failing;
            // never leave it behind.
            let _ = std::fs::remove_file(&obj_path);
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot emit object file `{}`: {e}",
                    obj_path.display()
                ))
                .with_code("E0001"),
            );
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: None,
            };
        }
    };

    let covmap_path = output_path.with_extension("tyra-covmap");
    if let Err(e) = std::fs::write(&covmap_path, &covmap_text) {
        let _ = std::fs::remove_file(&obj_path);
        let mut report = result.report;
        report.add(
            tyra_diagnostics::Diagnostic::error(format!(
                "cannot write covmap `{}`: {e}",
                covmap_path.display()
            ))
            .with_code("E0001"),
        );
        return CompileResult {
            success: false,
            report,
            sources: result.sources,
            llvm_ir: None,
        };
    }

    // Link only (coverage binaries are always -O0, already baked into the
    // object by `emit_object_coverage`; no `-O*` flag applies to a `.o` input).
    let mut clang_args: Vec<String> = vec![
        obj_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
    ];
    for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
        let lib_dir = format!("{prefix}/lib");
        if std::path::Path::new(&lib_dir).is_dir() {
            clang_args.push(format!("-L{lib_dir}"));
            break;
        }
    }
    match find_runtime_staticlib() {
        Some(p) => {
            clang_args.push(p.to_string_lossy().into_owned());
        }
        None => {
            let _ = std::fs::remove_file(&obj_path);
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "Tyra runtime staticlib not found for coverage build (searched {}).",
                    runtime_staticlib_search_list()
                ))
                .with_code("E0001"),
            );
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: None,
            };
        }
    }
    clang_args.push("-lgc".into());
    if cfg!(target_os = "linux") {
        clang_args.push("-lpthread".into());
        clang_args.push("-ldl".into());
        clang_args.push("-lm".into());
    }

    let clang_result = Command::new("clang").args(&clang_args).output();
    let _ = std::fs::remove_file(&obj_path);

    match clang_result {
        Ok(out) if out.status.success() => result,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!("clang failed: {stderr}"))
                    .with_code("E0500"),
            );
            CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: None,
            }
        }
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!("cannot run clang: {e}"))
                    .with_code("E0500"),
            );
            CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: None,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Run helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Result of running a pre-compiled binary.
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Run the binary at `binary_path` with the given `args`, optionally killing
/// it after `timeout_secs` seconds.
pub fn run_binary(binary_path: &Path, args: &[&str], timeout_secs: Option<u64>) -> RunOutcome {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let mut child = match Command::new(binary_path)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return RunOutcome {
                stdout: String::new(),
                stderr: format!("cannot execute binary: {e}"),
                exit_code: None,
                timed_out: false,
            };
        }
    };

    // Drain stdout and stderr on background threads to prevent pipe-buffer
    // deadlock when the binary emits large output.
    let stdout_drain = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).ok();
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let stderr_drain = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf).ok();
            String::from_utf8_lossy(&buf).into_owned()
        })
    });

    let (exit_code, timed_out) = if let Some(secs) = timeout_secs {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break (status.code(), false),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        child.kill().ok();
                        child.wait().ok();
                        break (None, true);
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    break (None, false);
                }
            }
        }
    } else {
        match child.wait() {
            Ok(status) => (status.code(), false),
            Err(_) => (None, false),
        }
    };

    let stdout = stdout_drain.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_drain.and_then(|h| h.join().ok()).unwrap_or_default();

    RunOutcome {
        stdout,
        stderr,
        exit_code,
        timed_out,
    }
}

/// Result of compiling and running a Tyra source file (capturing stdout/stderr).
pub struct CapturedRunResult {
    pub report: Report,
    pub sources: SourceMap,
    /// Captured stdout from the process; None if compilation failed.
    pub stdout: Option<String>,
    /// Captured stderr from the process; None if compilation failed.
    pub stderr: Option<String>,
    /// Process exit code; None if compilation or exec failed.
    pub exit_code: Option<i32>,
    /// True when the binary was killed because it exceeded the timeout.
    pub timed_out: bool,
}

/// Compile `source_path` and run it, capturing stdout and stderr.
pub fn run_and_capture(source_path: &Path) -> CapturedRunResult {
    let tmp_dir = std::env::temp_dir();
    let bin_id = BINARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let binary_path = tmp_dir.join(format!("tyra_test_{}_{}", std::process::id(), bin_id));

    let compile = compile_to_binary(source_path, &binary_path);
    if !compile.success {
        return CapturedRunResult {
            report: compile.report,
            sources: compile.sources,
            stdout: None,
            stderr: None,
            exit_code: None,
            timed_out: false,
        };
    }

    let outcome = run_binary(&binary_path, &[], None);
    let _ = std::fs::remove_file(&binary_path);

    CapturedRunResult {
        report: compile.report,
        sources: compile.sources,
        stdout: Some(outcome.stdout),
        stderr: Some(outcome.stderr),
        exit_code: outcome.exit_code,
        timed_out: false,
    }
}

/// Compile `source_path` and run it with a wall-clock timeout on the binary
/// execution phase only (compilation is unlimited).
pub fn run_and_capture_with_timeout(source_path: &Path, timeout_secs: u64) -> CapturedRunResult {
    let tmp_dir = std::env::temp_dir();
    let bin_id = BINARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let binary_path = tmp_dir.join(format!("tyra_test_{}_{}", std::process::id(), bin_id));

    let compile = compile_to_binary(source_path, &binary_path);
    if !compile.success {
        return CapturedRunResult {
            report: compile.report,
            sources: compile.sources,
            stdout: None,
            stderr: None,
            exit_code: None,
            timed_out: false,
        };
    }

    let outcome = run_binary(&binary_path, &[], Some(timeout_secs));
    let _ = std::fs::remove_file(&binary_path);

    CapturedRunResult {
        report: compile.report,
        sources: compile.sources,
        stdout: Some(outcome.stdout),
        stderr: Some(outcome.stderr),
        exit_code: outcome.exit_code,
        timed_out: outcome.timed_out,
    }
}

/// Outcome of `tyra run` (ADR-0029): the compilation result plus the child
/// process exit status when the program actually ran.
pub struct ProgramRunResult {
    pub compile: CompileResult,
    /// `Some(code)` when the program ran to completion; `None` when it was
    /// killed by a signal or could not be executed.
    pub program_exit: Option<i32>,
}

pub fn run(source_path: &Path) -> ProgramRunResult {
    run_opts(source_path, false)
}

pub fn run_release(source_path: &Path) -> ProgramRunResult {
    run_opts(source_path, true)
}

fn run_opts(source_path: &Path, release: bool) -> ProgramRunResult {
    let tmp_dir = std::env::temp_dir();
    let binary_path = tmp_dir.join(format!("tyra_run_{}", std::process::id()));

    let result = compile_to_binary_opts(source_path, &binary_path, release, false);
    if !result.success {
        return ProgramRunResult {
            compile: result,
            program_exit: None,
        };
    }

    let run_result = Command::new(&binary_path).status();
    let _ = std::fs::remove_file(&binary_path);

    match run_result {
        Ok(status) => {
            match status.code() {
                // ADR-0029: a nonzero exit code is a normal program outcome.
                // E0501 is reserved for abnormal termination: panic (101) and
                // signal kills (no exit code).
                Some(101) => {
                    let mut report = result.report;
                    report.add(
                        tyra_diagnostics::Diagnostic::error(
                            "program exited with status 101".to_string(),
                        )
                        .with_code("E0501"),
                    );
                    ProgramRunResult {
                        compile: CompileResult {
                            success: false,
                            report,
                            sources: result.sources,
                            llvm_ir: result.llvm_ir,
                        },
                        program_exit: Some(101),
                    }
                }
                Some(code) => ProgramRunResult {
                    compile: result,
                    program_exit: Some(code),
                },
                None => {
                    let mut report = result.report;
                    report.add(
                        tyra_diagnostics::Diagnostic::error(
                            "program terminated by signal".to_string(),
                        )
                        .with_code("E0501"),
                    );
                    ProgramRunResult {
                        compile: CompileResult {
                            success: false,
                            report,
                            sources: result.sources,
                            llvm_ir: result.llvm_ir,
                        },
                        program_exit: None,
                    }
                }
            }
        }
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!("cannot execute binary: {e}"))
                    .with_code("E0501"),
            );
            ProgramRunResult {
                compile: CompileResult {
                    success: false,
                    report,
                    sources: result.sources,
                    llvm_ir: result.llvm_ir,
                },
                program_exit: None,
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_staticlib_candidates_cover_portable_and_fhs_layouts() {
        let candidates = runtime_staticlib_candidates();
        assert_eq!(candidates.len(), 2, "expected exe-dir + FHS candidates");
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(candidates[0], exe_dir.join("libtyra_runtime.a"));
        assert_eq!(
            candidates[1],
            exe_dir
                .join("..")
                .join("lib")
                .join("tyra")
                .join("libtyra_runtime.a")
        );
    }

    #[test]
    fn runtime_staticlib_search_list_names_both_locations() {
        let list = runtime_staticlib_search_list();
        assert!(list.contains("libtyra_runtime.a"));
        assert!(list.contains(" or "), "should list both candidates: {list}");
    }

    #[test]
    fn intermediate_object_path_rejects_paths_without_a_file_name() {
        for bad in ["/", ".", "..", "", "examples/.."] {
            let result = intermediate_object_path(Path::new(bad));
            assert!(
                result.is_err(),
                "expected an error for output path `{bad}`, got: {result:?}"
            );
        }
    }

    #[test]
    fn intermediate_object_path_accepts_paths_with_a_file_name() {
        for good in ["foo", "foo.o", "dir/foo"] {
            let output = Path::new(good);
            let path =
                intermediate_object_path(output).expect("expected Ok for a path with a file name");
            assert_ne!(
                path.as_path(),
                output,
                "intermediate path must differ from the output path"
            );
        }
    }

    #[test]
    fn check_in_memory_clean_program() {
        let CheckResult { report, .. } = check_in_memory(
            "ok.ty".into(),
            "fn main() -> Unit\n  print(\"hello\")\nend\n".into(),
            None,
        );
        assert!(
            !report.has_errors(),
            "unexpected errors: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn check_in_memory_reports_e0110_for_import_in_fn() {
        let CheckResult { report, .. } = check_in_memory(
            "bad.ty".into(),
            "fn f() -> Int\n  import foo\n  0\nend\n".into(),
            None,
        );
        assert!(report.has_errors());
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0110"), "expected E0110, got: {codes:?}");
    }

    #[test]
    fn check_in_memory_reports_parse_error() {
        let CheckResult { report, .. } =
            check_in_memory("bad.ty".into(), "let x = \n".into(), None);
        assert!(report.has_errors(), "expected parse error");
    }

    #[test]
    fn resolve_import_file_finds_local_and_skips_builtin() {
        use std::fs;
        let dir = std::env::temp_dir().join("tyra_driver_rif_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.ty");
        fs::write(&foo_path, "").unwrap();

        // local module resolves
        let got = resolve_import_file(&dir, &["foo".to_string()]);
        assert_eq!(
            got.as_deref(),
            Some(foo_path.as_path()),
            "should find foo.ty"
        );

        // non-existent module
        let got = resolve_import_file(&dir, &["bar".to_string()]);
        assert!(got.is_none(), "should not find bar.ty: {got:?}");

        // built-in module skipped
        let got = resolve_import_file(&dir, &["core".to_string(), "sys".to_string()]);
        assert!(got.is_none(), "core.sys is builtin, should return None");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_import_file_ambiguous_local_and_stdlib_returns_none() {
        use std::fs;

        struct EnvGuard {
            prev: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                unsafe {
                    match self.prev.take() {
                        Some(v) => std::env::set_var("TYRA_STDLIB", v),
                        None => std::env::remove_var("TYRA_STDLIB"),
                    }
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let main_dir = dir.path();
        fs::write(main_dir.join("mymod.ty"), "export fn f() -> Unit\nend\n").unwrap();
        // The fake stdlib must pass is_tyra_stdlib (needs assert.ty sentinel).
        let stdlib_dir = dir.path().join("stdlib");
        fs::create_dir(&stdlib_dir).unwrap();
        fs::write(stdlib_dir.join("assert.ty"), "").unwrap();
        fs::write(
            stdlib_dir.join("mymod.ty"),
            "export fn f() -> Unit\nend\n",
        )
        .unwrap();

        let _guard = EnvGuard {
            prev: std::env::var("TYRA_STDLIB").ok(),
        };
        unsafe {
            std::env::set_var("TYRA_STDLIB", &stdlib_dir);
        }
        let result = resolve_import_file(main_dir, &["mymod".to_string()]);

        assert!(
            result.is_none(),
            "ambiguous local+stdlib import must return None, got {result:?}"
        );
    }

    #[test]
    fn resolve_import_file_path_dep_returns_some() {
        use std::fs;
        let project = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();

        fs::write(
            lib.path().join("Tyra.toml"),
            "[package]\nname    = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .unwrap();
        fs::create_dir(lib.path().join("src")).unwrap();
        let lib_src = lib.path().join("src/mylib.ty");
        fs::write(&lib_src, "export fn greet(n: String) -> String\n  n\nend\n").unwrap();

        fs::write(
            project.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"myapp\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nmylib = {{ path = \"{}\" }}\n",
                lib.path().display()
            ),
        )
        .unwrap();
        let src_dir = project.path().join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("myapp.ty"), "import mylib\n").unwrap();

        let result = resolve_import_file(&src_dir, &["mylib".to_string()]);
        assert!(result.is_some(), "path dep resolution must return Some");
        assert_eq!(
            result.unwrap().canonicalize().unwrap(),
            lib_src.canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_import_file_bin_path_dep_returns_none() {
        use std::fs;
        let project = tempfile::tempdir().unwrap();
        let bin_dep = tempfile::tempdir().unwrap();

        fs::write(
            bin_dep.path().join("Tyra.toml"),
            "[package]\nname    = \"mybin\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .unwrap();
        fs::create_dir(bin_dep.path().join("src")).unwrap();
        fs::write(
            bin_dep.path().join("src/mybin.ty"),
            "fn main() -> Unit\n  print(\"hi\")\nend\n",
        )
        .unwrap();

        fs::write(
            project.path().join("Tyra.toml"),
            format!(
                "[package]\nname    = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\
                 \n[dependencies]\nmybin = {{ path = \"{}\" }}\n",
                bin_dep.path().display()
            ),
        )
        .unwrap();
        let src_dir = project.path().join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("app.ty"), "import mybin\n").unwrap();

        let result = resolve_import_file(&src_dir, &["mybin".to_string()]);
        assert!(
            result.is_none(),
            "bin path dep must return None, got {result:?}"
        );
    }

    #[test]
    fn auto_import_detects_module_call_inside_propagate() {
        // `string.parse_int(...)?` — module call wrapped in `?`.
        let CheckResult { report, .. } = check_in_memory(
            "p.ty".into(),
            "import io\n\nfn main() -> Result<Unit, String>\n  \
             let line = match io.read_line() when Some(s) s when None \"\" end\n  \
             let n = string.parse_int(string.trim(line)).ok_or(\"bad\")?\n  \
             print(\"#{n}\")\n  Ok(())\nend\n"
                .into(),
            None,
        );
        assert!(
            !report.has_errors(),
            "expected clean compile, got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn continue_inside_while_is_valid() {
        let src = concat!(
            "fn main() -> Unit\n",
            "  mut i = 0\n",
            "  while i < 5\n",
            "    i = i + 1\n",
            "    if i == 3\n",
            "      continue\n",
            "    end\n",
            "    print(\"done\")\n",
            "  end\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("c.ty".into(), src.into(), None);
        assert!(
            !report.has_errors(),
            "unexpected errors: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn continue_inside_for_is_valid() {
        let src = concat!(
            "fn main() -> Unit\n",
            "  let xs = [1, 2, 3, 4, 5]\n",
            "  for i in xs\n",
            "    if i == 3\n",
            "      continue\n",
            "    end\n",
            "    print(\"done\")\n",
            "  end\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("c.ty".into(), src.into(), None);
        assert!(
            !report.has_errors(),
            "unexpected errors: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn continue_outside_loop_emits_e0215() {
        let CheckResult { report, .. } = check_in_memory(
            "bad.ty".into(),
            "fn main() -> Unit\n  continue\nend\n".into(),
            None,
        );
        assert!(report.has_errors(), "expected E0215 error");
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0215"), "expected E0215, got: {codes:?}");
    }

    #[test]
    fn continue_inside_lambda_in_loop_emits_e0215() {
        let src = concat!(
            "fn main() -> Unit\n",
            "  while true\n",
            "    let f = fn() -> Unit\n",
            "      continue\n",
            "    end\n",
            "    break\n",
            "  end\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("bad.ty".into(), src.into(), None);
        assert!(report.has_errors(), "expected E0215 inside lambda");
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0215"), "expected E0215, got: {codes:?}");
    }

    #[test]
    fn break_inside_lambda_in_loop_emits_e0214() {
        let src = concat!(
            "fn main() -> Unit\n",
            "  while true\n",
            "    let f = fn() -> Unit\n",
            "      break\n",
            "    end\n",
            "    break\n",
            "  end\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("bad.ty".into(), src.into(), None);
        assert!(report.has_errors(), "expected E0214 inside lambda");
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0214"), "expected E0214, got: {codes:?}");
    }

    #[test]
    fn lambda_return_type_mismatch_emits_e0309() {
        let src = concat!(
            "fn outer() -> String\n",
            "  let f = fn() -> Unit\n",
            "    return \"hello\"\n",
            "  end\n",
            "  \"ok\"\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("bad.ty".into(), src.into(), None);
        assert!(report.has_errors(), "expected E0309 in lambda");
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0309"), "expected E0309, got: {codes:?}");
    }

    // --- Regression: object emission must not depend on the PATH `clang`'s
    // --- LLVM version (root cause: textual IR containing LLVM-19+ syntax like
    // --- `getelementptr ... nuw`, parsed by a `clang` that may predate it).

    /// Locate the `tyra` binary built alongside this test binary
    /// (`target/{debug,release}/tyra`, i.e. one directory up from
    /// `target/{debug,release}/deps/<this test binary>`). Mirrors the
    /// `find_tyra_binary` helper in `tyra-cli`'s own subprocess tests
    /// (main.rs). Calling through the real CLI binary (rather than the
    /// driver functions in-process) matters here for two reasons: (1) the
    /// runtime staticlib search in `find_runtime_staticlib` is relative to
    /// `current_exe()`, which only lands next to `libtyra_runtime.a` for the
    /// real binary, not a `cargo test` binary under `target/*/deps/`; and (2)
    /// spawning a child process lets us override `PATH` for just that child,
    /// instead of mutating the whole (parallel-test-sharing) process env.
    fn find_tyra_binary() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let profile_dir = exe.parent()?.parent()?;
        let tyra = profile_dir.join("tyra");
        if tyra.exists() { Some(tyra) } else { None }
    }

    #[test]
    #[ignore = "requires pre-built tyra binary — run with: cargo build && cargo test -p tyra-driver -- --ignored"]
    fn field_access_compiles_with_a_clang_that_rejects_ll_files() {
        let Some(tyra) = find_tyra_binary() else {
            eprintln!("SKIP: tyra binary not found — run `cargo build` first");
            return;
        };

        // Resolve the real clang so the shim can still delegate to it once it
        // has confirmed no `.ll` file was ever handed to it.
        let which_out = std::process::Command::new("which")
            .arg("clang")
            .output()
            .expect("failed to run `which clang`");
        assert!(
            which_out.status.success(),
            "clang must be on PATH to run this test"
        );
        let real_clang = String::from_utf8_lossy(&which_out.stdout)
            .trim()
            .to_string();

        // A `clang` shim that simulates a stock/older clang unable to parse
        // LLVM-19+ IR syntax: it rejects any `.ll` argument outright, exactly
        // as a real old clang would on `getelementptr inbounds nuw`. Any
        // other invocation (i.e. linking a `.o`) is forwarded to real clang.
        let shim_dir =
            std::env::temp_dir().join(format!("tyra_nuw_clang_shim_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&shim_dir);
        std::fs::create_dir_all(&shim_dir).unwrap();
        let shim_path = shim_dir.join("clang");
        std::fs::write(
            &shim_path,
            format!(
                "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    *.ll)\n      echo \"error: expected type\" >&2\n      exit 1\n      ;;\n  esac\ndone\nexec \"{real_clang}\" \"$@\"\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&shim_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&shim_path, perms).unwrap();
        }

        let src_dir = std::env::temp_dir().join(format!("tyra_nuw_src_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&src_dir);
        std::fs::create_dir_all(&src_dir).unwrap();
        let src_path = src_dir.join("point.ty");
        std::fs::write(
            &src_path,
            "data Point\n  x: Int\n  y: Int\nend\n\n\
             fn main() -> Unit\n  let p = Point(x: 1, y: 2)\n  println(\"#{p.x}\")\nend\n",
        )
        .unwrap();
        let out_path = src_dir.join("point_bin");

        let old_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{old_path}", shim_dir.display());

        let build_output = std::process::Command::new(&tyra)
            .args([
                "build",
                src_path.to_str().unwrap(),
                "-o",
                out_path.to_str().unwrap(),
            ])
            .env("PATH", &new_path)
            .output()
            .expect("failed to invoke tyra binary");

        assert!(
            build_output.status.success(),
            "expected compilation to succeed with a clang shim that rejects `.ll` \
             files (i.e. the driver must hand clang an object file, never textual \
             IR):\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let run_output = std::process::Command::new(&out_path)
            .output()
            .expect("failed to run compiled binary");
        assert!(
            run_output.status.success(),
            "compiled binary exited with failure: {:?}",
            run_output
        );
        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(
            stdout.trim(),
            "1",
            "expected `p.x` field access to print 1, got: {stdout:?}"
        );

        let _ = std::fs::remove_dir_all(&shim_dir);
        let _ = std::fs::remove_dir_all(&src_dir);
    }
}
