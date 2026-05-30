// tyra-driver: Compilation pipeline for the Tyra language.
//
// Pipeline: source -> lex -> parse -> resolve -> type check -> MIR -> LLVM IR -> binary
//
// spec reference: §19 (execution model)

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static BINARY_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub use tyra_ast::SourceFile;
pub use tyra_diagnostics::SourceId;
use tyra_diagnostics::{Report, SourceMap};
pub use tyra_resolve::{CompletionKind, DefIndex, SymbolList};
pub use tyra_resolve::{PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
pub use tyra_types::{Ty, TypeIndex};

/// Result of `check_in_memory` — all data produced by the lex→parse→resolve→typecheck pipeline.
pub struct CheckResult {
    pub report: Report,
    pub sources: SourceMap,
    pub type_index: TypeIndex,
    pub def_index: DefIndex,
    pub symbols: SymbolList,
    pub source_id: SourceId,
    pub ast: SourceFile,
}

/// Result of compilation.
pub struct CompileResult {
    pub success: bool,
    pub report: Report,
    pub sources: SourceMap,
    pub llvm_ir: Option<String>,
}

/// Check a Tyra source supplied as an in-memory string.
///
/// Runs lex → parse → auto-import → rename → (optional) import-resolve
/// → name-resolve → type-check.  Stops before MIR / LLVM codegen.
///
/// `SymbolList` is a flat list of all user-defined names collected by the
/// resolver, used by the LSP completion handler. Prelude names are not
/// included there — the LSP adds them from `PRELUDE_FUNCTIONS` etc.
///
/// If `workspace_dir` is `None`, filesystem import resolution is
/// skipped (suitable for LSP single-file diagnostics).
pub fn check_in_memory(
    file_name: String,
    source: String,
    workspace_dir: Option<&Path>,
) -> CheckResult {
    let mut sources = SourceMap::new();
    let mut report = Report::new();

    let source_id = sources.add(file_name, source);

    let mut ast = tyra_parser::parse(source_id, &sources, &mut report);
    if report.has_errors() {
        let empty_ast = ast;
        return CheckResult {
            report,
            sources,
            type_index: TypeIndex::new(),
            def_index: DefIndex::new(),
            symbols: SymbolList::new(),
            source_id,
            ast: empty_ast,
        };
    }

    auto_import_stdlib(&mut ast);
    desugar_test_blocks(&mut ast);
    rename_pattern_bindings(&mut ast);
    rename_let_shadows(&mut ast);

    if let Some(dir) = workspace_dir {
        resolve_imports(&mut ast, dir, &mut sources, &mut report);
        if report.has_errors() {
            let snapshot = ast;
            return CheckResult {
                report,
                sources,
                type_index: TypeIndex::new(),
                def_index: DefIndex::new(),
                symbols: SymbolList::new(),
                source_id,
                ast: snapshot,
            };
        }
    }

    let (def_index, symbol_list) = tyra_resolve::resolve(&ast, &mut report);
    if report.has_errors() {
        let snapshot = ast;
        return CheckResult {
            report,
            sources,
            type_index: TypeIndex::new(),
            def_index,
            symbols: symbol_list,
            source_id,
            ast: snapshot,
        };
    }

    let type_index = tyra_types::check(&ast, &mut report);
    CheckResult {
        report,
        sources,
        type_index,
        def_index,
        symbols: symbol_list,
        source_id,
        ast,
    }
}

/// Compile a Tyra source file to LLVM IR text (debug build, with DWARF).
pub fn compile_to_ir(source_path: &Path) -> CompileResult {
    compile_to_ir_impl(source_path, true, false).0
}

/// Like `compile_to_ir`, but generates coverage-instrumented IR and also
/// returns the covmap text (`<binary>.tyra-covmap` content).
fn compile_to_ir_coverage(source_path: &Path) -> (CompileResult, Option<String>) {
    compile_to_ir_impl(source_path, false, true)
}

/// Core compilation pipeline: parse → resolve → type check → MIR → codegen.
/// `debug_info = true` emits DWARF metadata for lldb (ADR-0014 §4a).
/// `coverage = true` uses `emit_llvm_ir_coverage` and returns the covmap text.
fn compile_to_ir_impl(
    source_path: &Path,
    debug_info: bool,
    coverage: bool,
) -> (CompileResult, Option<String>) {
    let mut sources = SourceMap::new();
    let mut report = Report::new();

    // Read source file
    let source = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(e) => {
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot read file `{}`: {e}",
                    source_path.display()
                ))
                .with_code("E0001"),
            );
            return (
                CompileResult {
                    success: false,
                    report,
                    sources,
                    llvm_ir: None,
                },
                None,
            );
        }
    };

    let source_id = sources.add(
        source_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        source,
    );

    // Parse
    let mut ast = tyra_parser::parse(source_id, &sources, &mut report);
    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
        );
    }

    // Auto-import obvious stdlib modules (string / list / io) when the
    // program calls `string.fn(...)` etc. but forgot the import. This
    // closes the most common E0200 hit in the AI-gen benchmark — the
    // model writes `string.trim(input)` directly without ever stating
    // `import string`. Adding the import is always safe (unused imports
    // are harmless) and converts a 5-error-per-run hot spot into an
    // auto-corrected program.
    auto_import_stdlib(&mut ast);
    desugar_test_blocks(&mut ast);

    // Alpha-rename match-pattern bindings to globally unique names. Two
    // sibling `when Some(v)` arms that bind values of different types
    // (e.g. `Option<String>` vs `Option<Int>`) would otherwise share a
    // single `%v` alloca and trip LLVM type-mismatch (E0500). Renaming
    // each pattern binding ensures one alloca per match arm.
    rename_pattern_bindings(&mut ast);

    // Alpha-rename `let X` / `mut X` shadows of any name already
    // introduced earlier in the same function. Mirrors the
    // function-wide alloca hoist in MIR (`collect_let_binding_counts_
    // in_stmts`): two `let X` with different types share a single
    // alloca slot, and LLVM rejects the second Store as
    // type-mismatched (E0500). Renaming the shadow produces two
    // distinct names, each with count == 1 → no hoist needed, no
    // type collision.
    rename_let_shadows(&mut ast);

    // Resolve imports: parse module files and merge exported items (§13)
    let main_dir = source_path.parent().unwrap_or(Path::new("."));
    resolve_imports(&mut ast, main_dir, &mut sources, &mut report);
    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
        );
    }

    // Name resolution
    let _ = tyra_resolve::resolve(&ast, &mut report);
    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
        );
    }

    // Type checking
    let _ = tyra_types::check(&ast, &mut report);
    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
        );
    }

    // MIR lowering
    let mir = tyra_mir::lower(&ast, &sources);
    for diag in mir.lower_errors.iter().cloned() {
        report.add(diag);
    }
    if report.has_errors() {
        return (
            CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            },
            None,
        );
    }

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
        );
    }

    // LLVM IR generation
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
        )
    }
}

/// Confirm that `dir` is a real Tyra stdlib tree, not a user directory named "stdlib".
/// Uses `assert.tyra` as a stable sentinel — it is part of the Tyra stdlib by spec.
fn is_tyra_stdlib(dir: &Path) -> bool {
    dir.is_dir() && dir.join("assert.tyra").is_file()
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
        // Catches `target/debug/tyra` in a source checkout: the walk reaches
        // `<repo>/stdlib/` before leaving the filesystem root.
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

/// Resolve import declarations by parsing module files and merging exported items.
///
/// Uses the ADR 0010 three-layer uniqueness rule:
///   (a) `<project_root>/src/` or `<main_dir>/` for script-mode files
///   (b) path / git dependencies from `Tyra.toml` (first import segment = dep name)
///   (c) stdlib (`TYRA_STDLIB` env or walk-up for `stdlib/`)
///
/// 0 candidates → E0200; 2+ candidates → E0217 E_IMPORT_AMBIGUOUS; 1 → use it.
fn resolve_imports(
    ast: &mut tyra_ast::SourceFile,
    main_dir: &Path,
    sources: &mut SourceMap,
    report: &mut Report,
) {
    use tyra_ast::Item;

    // Collect imports first (to avoid borrowing ast while mutating)
    let imports: Vec<_> = ast
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Import(imp) = item {
                Some(imp.clone())
            } else {
                None
            }
        })
        .collect();

    let mut merged_items = Vec::new();

    // Find project root and manifest (best-effort; None = script-mode)
    let project_root = tyra_manifest::find_project_root(main_dir);
    let manifest = project_root
        .as_deref()
        .and_then(|r| tyra_manifest::load_manifest(r).ok());

    // Layer (a) base: project_root/src if it exists, else main_dir (script-mode compat)
    let local_base: std::path::PathBuf = project_root
        .as_deref()
        .map(|r| {
            let src = r.join("src");
            if src.is_dir() { src } else { r.to_path_buf() }
        })
        .unwrap_or_else(|| main_dir.to_path_buf());

    for imp in &imports {
        let local_name = imp
            .alias
            .as_deref()
            .or_else(|| imp.path.last().map(String::as_str))
            .unwrap_or("_unknown");

        // Built-in modules (core.sys, etc.) require no file resolution.
        if is_builtin_module(&imp.path.join(".")) {
            continue;
        }

        // dep_errors: (dep_name, kind) where kind is "bin" or "name-mismatch".
        let mut dep_errors: Vec<(String, &'static str)> = Vec::new();
        let candidates = collect_import_candidates(
            &imp.path,
            &local_base,
            manifest.as_ref(),
            project_root.as_deref(),
            main_dir,
            &mut dep_errors,
        );
        for (dep, kind) in &dep_errors {
            let msg = if *kind == "bin" {
                format!(
                    "cannot import `{}`: dependency `{dep}` is a bin package \
                     and cannot be imported",
                    imp.path.join(".")
                )
            } else {
                format!(
                    "cannot import `{}`: cached dependency `{dep}` has a name mismatch \
                     (package.name does not equal the dependency key); run `tyra mod sync`",
                    imp.path.join(".")
                )
            };
            report.add(tyra_diagnostics::Diagnostic::error(msg).with_code("E0218"));
        }
        if !dep_errors.is_empty() {
            continue;
        }

        let module_path = match candidates.len() {
            0 => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "cannot import `{}`: module not found",
                        imp.path.join(".")
                    ))
                    .with_code("E0200"),
                );
                continue;
            }
            1 => candidates.into_iter().next().unwrap(),
            _ => {
                let locations = candidates
                    .iter()
                    .map(|p| format!("`{}`", p.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "import `{}` is ambiguous: found in {} locations: {}",
                        imp.path.join("."),
                        candidates.len(),
                        locations,
                    ))
                    .with_code("E0217"),
                );
                continue;
            }
        };

        let module_source = match std::fs::read_to_string(&module_path) {
            Ok(s) => s,
            Err(e) => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "cannot read `{}`: {e}",
                        module_path.display()
                    ))
                    .with_code("E0200"),
                );
                continue;
            }
        };

        let module_id = sources.add(
            module_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            module_source,
        );

        let module_ast = tyra_parser::parse(module_id, sources, report);
        if report.has_errors() {
            return;
        }

        // Merge exported items with mangled names
        for item in module_ast.items {
            match item {
                Item::FnDef(mut f) if f.is_export => {
                    f.name = format!("{local_name}__{}", f.name);
                    merged_items.push(Item::FnDef(f));
                }
                Item::ValueDef(v) if v.is_export => {
                    merged_items.push(Item::ValueDef(v));
                }
                Item::DataDef(d) if d.is_export => {
                    merged_items.push(Item::DataDef(d));
                }
                Item::TypeDef(t) if t.is_export => {
                    merged_items.push(Item::TypeDef(t));
                }
                Item::ImplDef(impl_def) => {
                    // impl blocks are always included (no export on impl)
                    merged_items.push(Item::ImplDef(impl_def));
                }
                _ => {
                    // Non-exported items and statements are skipped
                }
            }
        }
    }

    // Append merged items to the main AST
    ast.items.extend(merged_items);
}

/// Check if a module path refers to a compiler built-in module.
fn is_builtin_module(module_path: &str) -> bool {
    matches!(module_path, "core.sys" | "core.tasks")
}

/// Resolve `import a.b.c` to the `.tyra` file path using the ADR 0010 rule.
///
/// Returns `None` for built-in modules, ambiguous imports (E0217), or
/// paths that do not exist on disk. Returns `Some` only when exactly one
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
/// or `"name-mismatch"` (ADR 0010 no-alias rule). Callers emit E0218 for each
/// entry and skip the import. Both path deps and cached git deps are checked.
fn collect_import_candidates(
    path_segs: &[String],
    local_base: &Path,
    manifest: Option<&tyra_manifest::Manifest>,
    project_root: Option<&Path>,
    main_dir: &Path,
    dep_errors: &mut Vec<(String, &'static str)>,
) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    // Layer (a): local_base / a / b / c.tyra
    {
        let mut p = local_base.to_path_buf();
        for seg in path_segs {
            p.push(seg);
        }
        p.set_extension("tyra");
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
                    // Reject bin packages at compile time (ADR 0009 E_DEP_NOT_IMPORTABLE).
                    if is_bin_dep(&dep_root) {
                        dep_errors.push((dep_name.clone(), "bin"));
                        continue;
                    }
                    dep_root.join("src")
                }
                (None, Some(url), Some(rev)) => {
                    let dep_root = git_dep_cache_root(dep_name, url, rev);
                    // Compile-time guards for cached git deps (stale/manual caches):
                    // check both the no-alias rule (ADR 0010) and bin rejection (ADR 0009).
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
            dp.set_extension("tyra");
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
        sp.set_extension("tyra");
        if sp.is_file() {
            candidates.push(sp);
        }
    }

    candidates
}

/// Returns `true` when the dependency at `dep_root` is a bin package
/// (its root module contains `fn main` or top-level executable statements).
fn is_bin_dep(dep_root: &Path) -> bool {
    let Ok(manifest) = tyra_manifest::load_manifest(dep_root) else {
        return false;
    };
    let root_src = dep_root
        .join("src")
        .join(format!("{}.tyra", manifest.package.name));
    let Ok(src) = std::fs::read_to_string(&root_src) else {
        return false;
    };
    tyra_manifest::is_bin_source(&src)
}

/// Compile-time validation for a cached git dependency root.
///
/// Returns `Err` when:
/// - the manifest's `package.name` does not match `dep_name` (no-alias rule, ADR 0010), or
/// - the root module is a bin package (ADR 0009 E_DEP_NOT_IMPORTABLE).
///
/// Returns `Ok(())` when the cache entry is absent (not yet synced — caller
/// falls through to produce E0200) or when the manifest cannot be loaded.
fn check_git_dep_root(dep_name: &str, dep_root: &std::path::Path) -> Result<(), &'static str> {
    let Ok(manifest) = tyra_manifest::load_manifest(dep_root) else {
        return Ok(());
    };
    if manifest.package.name != dep_name {
        return Err("name-mismatch");
    }
    let root_src = dep_root
        .join("src")
        .join(format!("{}.tyra", manifest.package.name));
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

/// Add `import string` / `import list` / `import io` automatically when the
/// program calls those module's functions (`string.trim(s)`, `list.push(xs, v)`,
/// `io.read_line()`) without an explicit import. The AI-gen benchmark shows
/// the model frequently forgets these imports; auto-adding them is harmless
/// (unused imports do not affect output) and removes a class of E0200 hits.
fn auto_import_stdlib(ast: &mut tyra_ast::SourceFile) {
    use tyra_ast::{Expr, ExprKind, ImportDecl, Item, Stmt};

    const AUTO: &[&str] = &["string", "list", "io"];

    // Collect already-imported single-segment module names.
    let mut already: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &ast.items {
        if let Item::Import(imp) = item
            && imp.path.len() == 1
        {
            let local = imp.alias.as_deref().unwrap_or(&imp.path[0]);
            already.insert(local.to_string());
        }
    }

    // Walk the AST collecting module names referenced by `<module>.<fn>(...)`.
    let mut needed: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Method names that are unambiguous markers for the string stdlib.
    // If any of these appear as `<expr>.<method>(...)` we conservatively
    // assume the receiver is a String and import the string module — this
    // catches the common pattern `line.byte_at(i)` even though we cannot
    // tell at parse time that `line` is in fact a String. False positives
    // (an `impl` block defining its own `byte_at`) just produce one extra
    // unused import, which is harmless.
    const STRING_METHOD_HINTS: &[&str] = &[
        "byte_at",
        "substring",
        "from_byte",
        "parse_int",
        "parse_errno",
        "starts_with",
        "ends_with",
        "to_upper",
        "to_lower",
        "is_empty",
        "trim",
    ];

    fn walk_expr(e: &Expr, needed: &mut std::collections::HashSet<String>) {
        match &e.kind {
            ExprKind::Call(callee, args) => {
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    if let ExprKind::Ident(name) = &obj.kind
                        && matches!(name.as_str(), "string" | "list" | "io")
                    {
                        needed.insert(name.clone());
                    }
                    if STRING_METHOD_HINTS.contains(&method.as_str()) {
                        needed.insert("string".to_string());
                    }
                }
                walk_expr(callee, needed);
                for a in args {
                    walk_expr(&a.value, needed);
                }
            }
            ExprKind::TurbofishCall(callee, _, args) => {
                walk_expr(callee, needed);
                for a in args {
                    walk_expr(&a.value, needed);
                }
            }
            ExprKind::FieldAccess(obj, _) => walk_expr(obj, needed),
            ExprKind::BinaryOp(l, _, r) => {
                walk_expr(l, needed);
                walk_expr(r, needed);
            }
            ExprKind::UnaryOp(_, e) => walk_expr(e, needed),
            ExprKind::Assign(l, r) => {
                walk_expr(l, needed);
                walk_expr(r, needed);
            }
            ExprKind::If(i) => {
                walk_expr(&i.condition, needed);
                walk_stmts(&i.then_body, needed);
                if let Some(eb) = &i.else_body {
                    walk_else(eb, needed);
                }
            }
            ExprKind::Match(m) => {
                walk_expr(&m.subject, needed);
                for arm in &m.arms {
                    walk_stmts(&arm.body, needed);
                }
            }
            ExprKind::While(w) => {
                walk_expr(&w.condition, needed);
                walk_stmts(&w.body, needed);
            }
            ExprKind::For(f) => {
                walk_expr(&f.iter, needed);
                walk_stmts(&f.body, needed);
            }
            ExprKind::ListLit(items) => {
                for it in items {
                    walk_expr(it, needed);
                }
            }
            ExprKind::StringInterp(parts) => {
                for p in parts {
                    if let tyra_ast::StringPart::Expr(e) = p {
                        walk_expr(e, needed);
                    }
                }
            }
            ExprKind::Propagate(inner) => walk_expr(inner, needed),
            ExprKind::Await(inner) => walk_expr(inner, needed),
            ExprKind::Spawn(inner) => walk_expr(inner, needed),
            ExprKind::Index(base, idx) => {
                walk_expr(base, needed);
                walk_expr(idx, needed);
            }
            ExprKind::MapLit(pairs) => {
                for (k, v) in pairs {
                    walk_expr(k, needed);
                    walk_expr(v, needed);
                }
            }
            ExprKind::Lambda(l) => walk_stmts(&l.body, needed),
            _ => {}
        }
    }

    fn walk_stmts(stmts: &[Stmt], needed: &mut std::collections::HashSet<String>) {
        for s in stmts {
            walk_stmt(s, needed);
        }
    }

    fn walk_stmt(s: &Stmt, needed: &mut std::collections::HashSet<String>) {
        match s {
            Stmt::Let(l) => walk_expr(&l.value, needed),
            Stmt::Mut(m) => walk_expr(&m.value, needed),
            Stmt::Return(r) => {
                if let Some(v) = &r.value {
                    walk_expr(v, needed);
                }
            }
            Stmt::Expr(e) => walk_expr(&e.expr, needed),
            Stmt::Defer(d) => walk_expr(&d.expr, needed),
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn walk_else(eb: &tyra_ast::ElseBranch, needed: &mut std::collections::HashSet<String>) {
        match eb {
            tyra_ast::ElseBranch::Else(stmts) => walk_stmts(stmts, needed),
            tyra_ast::ElseBranch::ElseIf(i) => {
                walk_expr(&i.condition, needed);
                walk_stmts(&i.then_body, needed);
                if let Some(inner) = &i.else_body {
                    walk_else(inner, needed);
                }
            }
        }
    }

    for item in &ast.items {
        match item {
            Item::FnDef(f) => walk_stmts(&f.body, &mut needed),
            Item::Stmt(s) => walk_stmt(s, &mut needed),
            Item::ImplDef(impl_def) => {
                for m in &impl_def.methods {
                    walk_stmts(&m.body, &mut needed);
                }
            }
            _ => {}
        }
    }

    // Inject missing imports at the front of the items list so they are
    // resolved before any usage downstream.
    let mut to_add: Vec<&str> = Vec::new();
    for &m in AUTO {
        if needed.contains(m) && !already.contains(m) {
            to_add.push(m);
        }
    }
    if !to_add.is_empty() {
        // Reuse a span from an existing item so we have a valid SourceId.
        // The injected import is synthetic — diagnostic accuracy at this
        // span is not load-bearing — but a well-typed Span is required.
        let span = ast
            .items
            .iter()
            .find_map(|it| match it {
                Item::Import(i) => Some(i.span),
                Item::FnDef(f) => Some(f.span),
                Item::Stmt(s) => Some(stmt_span(s)),
                _ => None,
            })
            .unwrap_or(ast.span);
        let mut prefix: Vec<Item> = to_add
            .into_iter()
            .map(|m| {
                Item::Import(ImportDecl {
                    path: vec![m.to_string()],
                    alias: None,
                    span,
                })
            })
            .collect();
        prefix.append(&mut ast.items);
        ast.items = prefix;
    }
}

/// Alpha-rename match-pattern bindings to globally unique names.
///
/// AI-gen frequently produces code like:
///
/// ```tyra
/// let s = match io.read_line() when Some(v) v when None "" end
/// let n = match string.parse_int(s) when Some(v) v when None 0 end
/// ```
///
/// Both arms bind `v`, but `s` is `String` (ptr) and `n` is `Int`
/// (i64). The MIR pre-alloca pass creates one `%v` slot for the
/// function and Stores both ptr and i64 values into it — LLVM
/// rejects with E0500 type-mismatch.
///
/// Rename each pattern binding to `<orig>__p<N>` and substitute
/// references inside the arm body. Inner shadows (`let v = ...`
/// inside the arm) are not handled scope-perfectly today; they
/// would substitute through, but no production examples have hit
/// that combination yet. Tighten with proper scope tracking when
/// a real failure surfaces.
fn rename_pattern_bindings(ast: &mut tyra_ast::SourceFile) {
    use tyra_ast::{Expr, ExprKind, Item, MatchArm, Pattern, PatternField, PatternKind, Stmt};

    let mut counter: u32 = 0;

    fn fresh(orig: &str, counter: &mut u32) -> String {
        *counter += 1;
        format!("{orig}__p{counter}")
    }

    fn collect_idents(
        p: &mut PatternKind,
        renames: &mut std::collections::HashMap<String, String>,
        counter: &mut u32,
    ) {
        match p {
            // Skip renaming the wildcard discard `_`: it is not a binding,
            // so it must not generate a named alloca or substitute in the arm body.
            PatternKind::Ident(name) if name != "_" => {
                let new = fresh(name, counter);
                renames.insert(name.clone(), new.clone());
                *name = new;
            }
            PatternKind::Constructor(_, fields) => {
                for f in fields {
                    // For the shorthand `Some(v)` (parser desugars to
                    // `Some(v: v)`), match_lower uses `field_name` as
                    // the alloca destination. Keep field_name in sync
                    // with the rewritten Ident binding so the Store
                    // and Load both reference the same renamed slot.
                    let old_field = f.field_name.clone();
                    collect_idents(&mut f.pattern.kind, renames, counter);
                    if let PatternKind::Ident(new_name) = &f.pattern.kind
                        && f.field_name == old_field
                        && old_field != *new_name
                    {
                        f.field_name = new_name.clone();
                    }
                }
            }
            _ => {}
        }
    }

    fn substitute_in_expr(e: &mut Expr, renames: &std::collections::HashMap<String, String>) {
        match &mut e.kind {
            ExprKind::Ident(name) => {
                if let Some(new) = renames.get(name) {
                    *name = new.clone();
                }
            }
            ExprKind::Call(callee, args) => {
                substitute_in_expr(callee, renames);
                for a in args {
                    substitute_in_expr(&mut a.value, renames);
                }
            }
            ExprKind::TurbofishCall(callee, _, args) => {
                substitute_in_expr(callee, renames);
                for a in args {
                    substitute_in_expr(&mut a.value, renames);
                }
            }
            ExprKind::FieldAccess(obj, _) => substitute_in_expr(obj, renames),
            ExprKind::BinaryOp(l, _, r) => {
                substitute_in_expr(l, renames);
                substitute_in_expr(r, renames);
            }
            ExprKind::UnaryOp(_, e) => substitute_in_expr(e, renames),
            ExprKind::Assign(l, r) => {
                substitute_in_expr(l, renames);
                substitute_in_expr(r, renames);
            }
            ExprKind::If(i) => {
                substitute_in_expr(&mut i.condition, renames);
                substitute_in_stmts(&mut i.then_body, renames);
                if let Some(eb) = &mut i.else_body {
                    substitute_in_else(eb, renames);
                }
            }
            ExprKind::Match(m) => {
                substitute_in_expr(&mut m.subject, renames);
                for arm in &mut m.arms {
                    substitute_in_stmts(&mut arm.body, renames);
                }
            }
            ExprKind::While(w) => {
                substitute_in_expr(&mut w.condition, renames);
                substitute_in_stmts(&mut w.body, renames);
            }
            ExprKind::For(f) => {
                substitute_in_expr(&mut f.iter, renames);
                substitute_in_stmts(&mut f.body, renames);
            }
            ExprKind::ListLit(items) => {
                for it in items {
                    substitute_in_expr(it, renames);
                }
            }
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    substitute_in_expr(k, renames);
                    substitute_in_expr(v, renames);
                }
            }
            ExprKind::StringInterp(parts) => {
                for p in parts {
                    if let tyra_ast::StringPart::Expr(e) = p {
                        substitute_in_expr(e, renames);
                    }
                }
            }
            ExprKind::Index(obj, idx) => {
                substitute_in_expr(obj, renames);
                substitute_in_expr(idx, renames);
            }
            ExprKind::Propagate(e) | ExprKind::Await(e) | ExprKind::Spawn(e) => {
                substitute_in_expr(e, renames);
            }
            ExprKind::Lambda(lam) => substitute_in_stmts(&mut lam.body, renames),
            _ => {}
        }
    }

    fn substitute_in_stmts(
        stmts: &mut [Stmt],
        renames: &std::collections::HashMap<String, String>,
    ) {
        for s in stmts {
            substitute_in_stmt(s, renames);
        }
    }

    fn substitute_in_stmt(s: &mut Stmt, renames: &std::collections::HashMap<String, String>) {
        match s {
            Stmt::Let(l) => substitute_in_expr(&mut l.value, renames),
            Stmt::Mut(m) => substitute_in_expr(&mut m.value, renames),
            Stmt::Return(r) => {
                if let Some(v) = &mut r.value {
                    substitute_in_expr(v, renames);
                }
            }
            Stmt::Expr(e) => substitute_in_expr(&mut e.expr, renames),
            Stmt::Defer(d) => substitute_in_expr(&mut d.expr, renames),
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn substitute_in_else(
        eb: &mut tyra_ast::ElseBranch,
        renames: &std::collections::HashMap<String, String>,
    ) {
        match eb {
            tyra_ast::ElseBranch::Else(stmts) => substitute_in_stmts(stmts, renames),
            tyra_ast::ElseBranch::ElseIf(i) => {
                substitute_in_expr(&mut i.condition, renames);
                substitute_in_stmts(&mut i.then_body, renames);
                if let Some(inner) = &mut i.else_body {
                    substitute_in_else(inner, renames);
                }
            }
        }
    }

    fn process_arm(arm: &mut MatchArm, counter: &mut u32) {
        // First recurse into the arm body to handle nested matches with
        // their own pattern names; then collect this arm's renames and
        // apply them to the (already-recursed) body.
        process_stmts(&mut arm.body, counter);
        let mut renames: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        collect_idents(&mut arm.pattern.kind, &mut renames, counter);
        if !renames.is_empty() {
            substitute_in_stmts(&mut arm.body, &renames);
        }
    }

    fn process_expr(e: &mut Expr, counter: &mut u32) {
        match &mut e.kind {
            ExprKind::Match(m) => {
                process_expr(&mut m.subject, counter);
                for arm in &mut m.arms {
                    process_arm(arm, counter);
                }
            }
            ExprKind::Call(callee, args) => {
                process_expr(callee, counter);
                for a in args {
                    process_expr(&mut a.value, counter);
                }
            }
            ExprKind::TurbofishCall(callee, _, args) => {
                process_expr(callee, counter);
                for a in args {
                    process_expr(&mut a.value, counter);
                }
            }
            ExprKind::FieldAccess(obj, _) => process_expr(obj, counter),
            ExprKind::BinaryOp(l, _, r) => {
                process_expr(l, counter);
                process_expr(r, counter);
            }
            ExprKind::UnaryOp(_, e) => process_expr(e, counter),
            ExprKind::Assign(l, r) => {
                process_expr(l, counter);
                process_expr(r, counter);
            }
            ExprKind::If(i) => {
                process_expr(&mut i.condition, counter);
                process_stmts(&mut i.then_body, counter);
                if let Some(eb) = &mut i.else_body {
                    process_else(eb, counter);
                }
            }
            ExprKind::While(w) => {
                process_expr(&mut w.condition, counter);
                process_stmts(&mut w.body, counter);
            }
            ExprKind::For(f) => {
                process_expr(&mut f.iter, counter);
                process_stmts(&mut f.body, counter);
            }
            ExprKind::ListLit(items) => {
                for it in items {
                    process_expr(it, counter);
                }
            }
            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    process_expr(k, counter);
                    process_expr(v, counter);
                }
            }
            ExprKind::StringInterp(parts) => {
                for p in parts {
                    if let tyra_ast::StringPart::Expr(e) = p {
                        process_expr(e, counter);
                    }
                }
            }
            ExprKind::Index(obj, idx) => {
                process_expr(obj, counter);
                process_expr(idx, counter);
            }
            ExprKind::Propagate(e) | ExprKind::Await(e) | ExprKind::Spawn(e) => {
                process_expr(e, counter);
            }
            ExprKind::Lambda(lam) => process_stmts(&mut lam.body, counter),
            _ => {}
        }
    }

    fn process_stmts(stmts: &mut [Stmt], counter: &mut u32) {
        for s in stmts {
            process_stmt(s, counter);
        }
    }

    fn process_stmt(s: &mut Stmt, counter: &mut u32) {
        match s {
            Stmt::Let(l) => process_expr(&mut l.value, counter),
            Stmt::Mut(m) => process_expr(&mut m.value, counter),
            Stmt::Return(r) => {
                if let Some(v) = &mut r.value {
                    process_expr(v, counter);
                }
            }
            Stmt::Expr(e) => process_expr(&mut e.expr, counter),
            Stmt::Defer(d) => process_expr(&mut d.expr, counter),
            Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn process_else(eb: &mut tyra_ast::ElseBranch, counter: &mut u32) {
        match eb {
            tyra_ast::ElseBranch::Else(stmts) => process_stmts(stmts, counter),
            tyra_ast::ElseBranch::ElseIf(i) => {
                process_expr(&mut i.condition, counter);
                process_stmts(&mut i.then_body, counter);
                if let Some(inner) = &mut i.else_body {
                    process_else(inner, counter);
                }
            }
        }
    }

    let _ = (
        Pattern {
            kind: PatternKind::Wildcard,
            span: ast.span,
        },
        PatternField {
            field_name: String::new(),
            pattern: Pattern {
                kind: PatternKind::Wildcard,
                span: ast.span,
            },
            span: ast.span,
        },
    );

    for item in &mut ast.items {
        match item {
            Item::FnDef(f) => process_stmts(&mut f.body, &mut counter),
            Item::Stmt(s) => process_stmt(s, &mut counter),
            Item::ImplDef(impl_def) => {
                for m in &mut impl_def.methods {
                    process_stmts(&mut m.body, &mut counter);
                }
            }
            _ => {}
        }
    }
}

/// Rename `let X` / `mut X` whose name has already been introduced
/// earlier in the same function — by a prior let/mut, parameter,
/// match-pattern binding, or for-loop binding. The shadow is renamed
/// to `<orig>__l<N>` and references in the lexical scope of the new
/// binding are substituted to point at the renamed slot. References
/// to the *outer* binding, in scopes that don't see the shadow, are
/// untouched.
///
/// Without this pass two `let X` with different types collapse onto
/// a single function-scoped `%X` alloca and LLVM rejects with E0500
/// (`type i64 but expected '%struct.Option__Int'`, etc.). Companion
/// to `rename_pattern_bindings` which handles the same problem for
/// match-arm pattern bindings.
fn rename_let_shadows(ast: &mut tyra_ast::SourceFile) {
    use std::collections::{HashMap, HashSet};
    use tyra_ast::{ElseBranch, Expr, ExprKind, Item, Pattern, PatternKind, Stmt, StringPart};

    struct Pass {
        counter: u32,
        // Function-wide set of names already bound (mirrors MIR hoist).
        // Only ever grows during a single function walk.
        introduced: HashSet<String>,
    }

    impl Pass {
        fn fresh(&mut self, orig: &str) -> String {
            self.counter += 1;
            format!("{orig}__l{}", self.counter)
        }

        // Apply active renames to a bare Ident reference.
        fn rewrite_ident(name: &mut String, active: &HashMap<String, String>) {
            if let Some(new) = active.get(name) {
                *name = new.clone();
            }
        }

        fn walk_stmts(&mut self, stmts: &mut [Stmt], active: &mut HashMap<String, String>) {
            for stmt in stmts.iter_mut() {
                match stmt {
                    Stmt::Let(l) => {
                        // RHS is evaluated under the *outer* scope (a `let X`
                        // doesn't see itself). Walk it first; only after it's
                        // lowered do we register the new binding.
                        self.walk_expr(&mut l.value, active);
                        if self.introduced.contains(&l.name) {
                            let new = self.fresh(&l.name);
                            active.insert(l.name.clone(), new.clone());
                            l.name = new.clone();
                            self.introduced.insert(new);
                        } else {
                            self.introduced.insert(l.name.clone());
                        }
                    }
                    Stmt::Mut(m) => {
                        self.walk_expr(&mut m.value, active);
                        if self.introduced.contains(&m.name) {
                            let new = self.fresh(&m.name);
                            active.insert(m.name.clone(), new.clone());
                            m.name = new.clone();
                            self.introduced.insert(new);
                        } else {
                            self.introduced.insert(m.name.clone());
                        }
                    }
                    Stmt::Expr(e) => self.walk_expr(&mut e.expr, active),
                    Stmt::Return(r) => {
                        if let Some(v) = &mut r.value {
                            self.walk_expr(v, active);
                        }
                    }
                    Stmt::Defer(d) => self.walk_expr(&mut d.expr, active),
                    Stmt::Break(_) | Stmt::Continue(_) => {}
                }
            }
        }

        fn walk_expr(&mut self, e: &mut Expr, active: &mut HashMap<String, String>) {
            match &mut e.kind {
                ExprKind::Ident(name) => Self::rewrite_ident(name, active),
                ExprKind::Call(callee, args) => {
                    self.walk_expr(callee, active);
                    for a in args {
                        self.walk_expr(&mut a.value, active);
                    }
                }
                ExprKind::TurbofishCall(callee, _, args) => {
                    self.walk_expr(callee, active);
                    for a in args {
                        self.walk_expr(&mut a.value, active);
                    }
                }
                ExprKind::FieldAccess(obj, _) => self.walk_expr(obj, active),
                ExprKind::BinaryOp(l, _, r) => {
                    self.walk_expr(l, active);
                    self.walk_expr(r, active);
                }
                ExprKind::UnaryOp(_, e) => self.walk_expr(e, active),
                ExprKind::Assign(l, r) => {
                    self.walk_expr(l, active);
                    self.walk_expr(r, active);
                }
                ExprKind::If(i) => {
                    self.walk_expr(&mut i.condition, active);
                    let saved = active.clone();
                    self.walk_stmts(&mut i.then_body, active);
                    *active = saved.clone();
                    if let Some(eb) = &mut i.else_body {
                        self.walk_else(eb, active);
                        *active = saved;
                    }
                }
                ExprKind::Match(m) => {
                    self.walk_expr(&mut m.subject, active);
                    for arm in &mut m.arms {
                        let saved = active.clone();
                        // Pattern bindings already alpha-renamed to unique
                        // names by rename_pattern_bindings; still register
                        // them as introduced so a subsequent `let` of the
                        // same final name (rare, but possible if the user
                        // happened to pick `x__p1`) is detected as a shadow.
                        let mut pat_names: Vec<String> = Vec::new();
                        Self::collect_pattern_idents(&arm.pattern, &mut pat_names);
                        for n in &pat_names {
                            self.introduced.insert(n.clone());
                        }
                        self.walk_stmts(&mut arm.body, active);
                        *active = saved;
                    }
                }
                ExprKind::While(w) => {
                    self.walk_expr(&mut w.condition, active);
                    let saved = active.clone();
                    self.walk_stmts(&mut w.body, active);
                    *active = saved;
                }
                ExprKind::For(f) => {
                    self.walk_expr(&mut f.iter, active);
                    let saved = active.clone();
                    // Each for-binding lives in MIR as a function-wide alloca;
                    // treat it like a let for shadow-rename purposes.
                    for name in f.bindings.iter_mut() {
                        if self.introduced.contains(name.as_str()) {
                            let new = self.fresh(name);
                            active.insert(name.clone(), new.clone());
                            *name = new.clone();
                            self.introduced.insert(new);
                        } else {
                            self.introduced.insert(name.clone());
                        }
                    }
                    self.walk_stmts(&mut f.body, active);
                    *active = saved;
                }
                ExprKind::ListLit(items) => {
                    for it in items {
                        self.walk_expr(it, active);
                    }
                }
                ExprKind::MapLit(pairs) => {
                    for (k, v) in pairs {
                        self.walk_expr(k, active);
                        self.walk_expr(v, active);
                    }
                }
                ExprKind::StringInterp(parts) => {
                    for p in parts {
                        if let StringPart::Expr(e) = p {
                            self.walk_expr(e, active);
                        }
                    }
                }
                ExprKind::Index(obj, idx) => {
                    self.walk_expr(obj, active);
                    self.walk_expr(idx, active);
                }
                ExprKind::Propagate(inner) | ExprKind::Await(inner) | ExprKind::Spawn(inner) => {
                    self.walk_expr(inner, active);
                }
                ExprKind::Lambda(lam) => {
                    // Lambda introduces a fresh scope with its own params.
                    let saved = active.clone();
                    let saved_introduced = self.introduced.clone();
                    for p in &lam.params {
                        self.introduced.insert(p.name.clone());
                    }
                    self.walk_stmts(&mut lam.body, active);
                    *active = saved;
                    self.introduced = saved_introduced;
                }
                _ => {}
            }
        }

        fn walk_else(&mut self, eb: &mut ElseBranch, active: &mut HashMap<String, String>) {
            match eb {
                ElseBranch::Else(stmts) => self.walk_stmts(stmts, active),
                ElseBranch::ElseIf(i) => {
                    self.walk_expr(&mut i.condition, active);
                    let saved = active.clone();
                    self.walk_stmts(&mut i.then_body, active);
                    *active = saved.clone();
                    if let Some(inner) = &mut i.else_body {
                        self.walk_else(inner, active);
                        *active = saved;
                    }
                }
            }
        }

        fn collect_pattern_idents(p: &Pattern, out: &mut Vec<String>) {
            match &p.kind {
                PatternKind::Ident(name) => out.push(name.clone()),
                PatternKind::Constructor(_, fields) => {
                    for f in fields {
                        Self::collect_pattern_idents(&f.pattern, out);
                    }
                }
                _ => {}
            }
        }
    }

    // Each function body / impl method / top-level scope is independent —
    // shadowing only collides within a single MIR function (ADR-0006: top-
    // level Stmts are desugared to one synthetic `fn main`). The counter
    // is shared across scopes so renamed names stay globally unique.
    let mut pass = Pass {
        counter: 0,
        introduced: HashSet::new(),
    };
    for item in &mut ast.items {
        match item {
            Item::FnDef(f) => {
                pass.introduced.clear();
                for p in &f.params {
                    pass.introduced.insert(p.name.clone());
                }
                let mut active = HashMap::new();
                pass.walk_stmts(&mut f.body, &mut active);
            }
            Item::ImplDef(impl_def) => {
                for m in &mut impl_def.methods {
                    pass.introduced.clear();
                    for p in &m.params {
                        pass.introduced.insert(p.name.clone());
                    }
                    let mut active = HashMap::new();
                    pass.walk_stmts(&mut m.body, &mut active);
                }
            }
            // Item::Stmt handled in the second pass below — top-level
            // Stmts share a single MIR function so they need one
            // continuous `introduced` set.
            _ => {}
        }
    }
    // Walk top-level statements as a single scope, with a fresh
    // introduced set so prior function-local names don't bleed in.
    pass.introduced.clear();
    let mut active = HashMap::new();
    for item in &mut ast.items {
        if let Item::Stmt(s) = item {
            pass.walk_stmts(std::slice::from_mut(s), &mut active);
        }
    }
}

fn stmt_span(s: &tyra_ast::Stmt) -> tyra_ast::Span {
    use tyra_ast::Stmt;
    match s {
        Stmt::Let(l) => l.span,
        Stmt::Mut(m) => m.span,
        Stmt::Return(r) => r.span,
        Stmt::Expr(e) => e.span,
        Stmt::Defer(d) => d.span,
        Stmt::Break(b) => b.span,
        Stmt::Continue(c) => c.span,
    }
}

/// Desugar `test "name" [panics] ... end` blocks (ADR 0013) into regular `fn` definitions.
///
/// Each `Item::TestDef` is converted to an `Item::FnDef` with:
/// - Name: `test__<sanitized>` or `test_panics__<sanitized>` (double underscore prefix).
/// - Return type: `Result<Unit, String>` (matching synthesize_runner's expectation).
/// - Body: the original stmts followed by a synthetic `Ok(())` as the implicit return.
///
/// Call this after import resolution but before name resolution so that
/// the resolver, type-checker, MIR, and codegen never see `Item::TestDef`.
fn desugar_test_blocks(ast: &mut tyra_ast::SourceFile) {
    use tyra_ast::{
        Arg, Expr, ExprKind, ExprStmt, FnDef, Item, Stmt, TestDef, TypeExpr, TypeExprKind,
    };

    fn sanitize(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn make_ok_unit(span: tyra_diagnostics::Span) -> Stmt {
        Stmt::Expr(ExprStmt {
            expr: Expr {
                kind: ExprKind::Call(
                    Box::new(Expr {
                        kind: ExprKind::Ident("Ok".into()),
                        span,
                    }),
                    vec![Arg {
                        label: None,
                        value: Expr {
                            kind: ExprKind::UnitLit,
                            span,
                        },
                        span,
                    }],
                ),
                span,
            },
            span,
        })
    }

    fn desugar(td: TestDef) -> FnDef {
        let suffix = sanitize(&td.name);
        let fn_name = if td.expects_panic {
            format!("test_panics__{suffix}")
        } else {
            format!("test__{suffix}")
        };

        let span = td.span;
        let result_ty = TypeExpr {
            kind: TypeExprKind::Generic(
                "Result".into(),
                vec![
                    TypeExpr {
                        kind: TypeExprKind::Named("Unit".into()),
                        span,
                    },
                    TypeExpr {
                        kind: TypeExprKind::Named("String".into()),
                        span,
                    },
                ],
            ),
            span,
        };

        let mut body = td.body;
        body.push(make_ok_unit(span));

        FnDef {
            name: fn_name,
            type_params: vec![],
            self_param: None,
            params: vec![],
            return_type: Some(result_ty),
            body,
            is_async: false,
            is_export: false,
            span,
        }
    }

    let items = std::mem::take(&mut ast.items);
    ast.items = items
        .into_iter()
        .map(|item| match item {
            Item::TestDef(td) => Item::FnDef(desugar(td)),
            other => other,
        })
        .collect();
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
///
/// Links with `-static` so the result is a self-contained single binary.
/// Reliable on musl libc (Alpine Linux).  On glibc hosts, static linking
/// is unsupported due to `getaddrinfo` / NSS requirements — the flag is
/// accepted at the CLI level but results are not guaranteed.
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
    let result = if release {
        compile_to_ir_impl(source_path, false, false).0
    } else {
        compile_to_ir(source_path) // includes DWARF
    };
    if !result.success {
        return result;
    }

    let llvm_ir = result.llvm_ir.as_ref().unwrap();

    // Write LLVM IR to temp file
    let ir_path = output_path.with_extension("ll");
    if let Err(e) = std::fs::write(&ir_path, llvm_ir) {
        let mut report = result.report;
        report.add(
            tyra_diagnostics::Diagnostic::error(format!(
                "cannot write IR file `{}`: {e}",
                ir_path.display()
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
        build_link_cmd_windows(result, &ir_path, output_path, release, static_link)
    }
    #[cfg(not(target_os = "windows"))]
    {
        build_link_cmd_unix(result, &ir_path, output_path, release, static_link)
    }
}

/// Unix (Linux/macOS) linker path: compile IR via clang and link libgc + runtime staticlib.
#[cfg(not(target_os = "windows"))]
fn build_link_cmd_unix(
    result: CompileResult,
    ir_path: &Path,
    output_path: &Path,
    release: bool,
    static_link: bool,
) -> CompileResult {
    // Compile with clang, linking Boehm GC (libgc, ADR-0007) and the Tyra
    // async runtime staticlib (libtyra_runtime.a, M9). The runtime is built
    // by cargo into the same target/ directory as the `tyra` binary itself,
    // so we locate it relative to the current executable.
    let opt_flag = if release { "-O2" } else { "-O0" };
    let mut clang_args: Vec<String> = vec![
        ir_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
        opt_flag.into(),
    ];
    // Preserve DWARF metadata from the IR in debug builds (ADR-0014 §4a).
    if !release {
        clang_args.push("-gdwarf-4".into());
    }
    // libgc: probe common install prefixes. Homebrew on Apple Silicon and
    // Intel place libgc under different roots; Linux package managers use
    // the default search path.
    for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
        let lib_dir = format!("{prefix}/lib");
        if std::path::Path::new(&lib_dir).is_dir() {
            clang_args.push(format!("-L{lib_dir}"));
            break;
        }
    }
    // libtyra_runtime: locate via the running compiler's target dir. The
    // staticlib is produced by cargo alongside the `tyra` binary (workspace
    // target/{debug,release}/). If it is missing (e.g. the user copied only
    // the `tyra` binary without `libtyra_runtime.a`, or installed via
    // `cargo install` without the staticlib), surface an explicit Tyra
    // diagnostic instead of letting clang emit an unresolved-symbol error.
    let runtime_lib_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("libtyra_runtime.a")));
    match runtime_lib_path.as_ref() {
        Some(p) if p.exists() => {
            clang_args.push(p.to_string_lossy().into_owned());
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
            let _ = std::fs::remove_file(ir_path);
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    }
    if static_link {
        // Static linking: pass -static so the linker prefers libgc.a over
        // libgc.so.  On Alpine musl, `gc-dev` installs libgc.a and the
        // default search path resolves it automatically.  We don't pass
        // explicit -L because the prefix probing above already added it for
        // Homebrew paths, and on Alpine the system search path is sufficient.
        clang_args.push("-static".into());
        clang_args.push("-lgc".into());
        // musl includes pthread and math in libc; libdl does not exist as a
        // separate library on musl.  On glibc static builds these are still
        // separate, but static glibc is unsupported; we keep -lpthread -lm
        // for robustness and omit -ldl (breaks musl + is fragile on glibc).
        if cfg!(target_os = "linux") {
            clang_args.push("-lpthread".into());
            clang_args.push("-lm".into());
        }
    } else {
        clang_args.push("-lgc".into());
        // The Rust staticlib pulls in std, which on Unix needs pthread + dl.
        // `cfg!` evaluates against the compiling host's target. v0.1 only
        // supports host-target compilation; cross-compile will need target-
        // triple plumbing here.
        if cfg!(target_os = "linux") {
            clang_args.push("-lpthread".into());
            clang_args.push("-ldl".into());
            clang_args.push("-lm".into());
        }
    }

    let clang_result = Command::new("clang").args(&clang_args).output();

    // Clean up IR file
    let _ = std::fs::remove_file(ir_path);

    match clang_result {
        Ok(output) => {
            if output.status.success() {
                result
            } else {
                let mut report = result.report;
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Detect missing libgc and surface an actionable diagnostic
                // instead of the raw linker error.
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
///
/// Dependencies are resolved via vcpkg:
///   - `VCPKG_ROOT` env var  ->  `$VCPKG_ROOT/installed/x64-windows/`
///   - fallback: `./vcpkg_installed/x64-windows/` (relative to cwd)
///   - fallback: `LIBGC_PREFIX` env var  ->  `$LIBGC_PREFIX/`
///
/// After a successful link, `gc.dll` is copied from the vcpkg `bin/` directory
/// to the output directory so the resulting `.exe` can find it at runtime.
/// If `gc.dll` cannot be located, a warning is printed but the build succeeds
/// (gc.dll may be installed system-wide or copied manually).
#[cfg(target_os = "windows")]
fn build_link_cmd_windows(
    result: CompileResult,
    ir_path: &Path,
    output_path: &Path,
    release: bool,
    _static_link: bool,
) -> CompileResult {
    // Step 1: LLVM IR -> native object file via llc.
    let obj_path = output_path.with_extension("obj");
    let opt_level = if release { "-O2" } else { "-O0" };
    let llc_status = Command::new("llc.exe")
        .args([
            opt_level,
            "-filetype=obj",
            "-mtriple=x86_64-pc-windows-msvc",
            ir_path.to_str().unwrap(),
            "-o",
            obj_path.to_str().unwrap(),
        ])
        .status();

    // Clean up IR file regardless of llc outcome.
    let _ = std::fs::remove_file(ir_path);

    let llc_ok = match llc_status {
        Ok(s) => s.success(),
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot run llc.exe: {e}. Is LLVM installed and on PATH?"
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
    };
    if !llc_ok {
        let mut report = result.report;
        report.add(
            tyra_diagnostics::Diagnostic::error(
                "llc.exe failed to compile LLVM IR to object file.".to_string(),
            )
            .with_code("E0500"),
        );
        return CompileResult {
            success: false,
            report,
            sources: result.sources,
            llvm_ir: result.llvm_ir,
        };
    }

    // Step 2: Resolve vcpkg install root for gc.lib / gc.dll.
    // Priority: VCPKG_ROOT env -> ./vcpkg_installed (cwd) -> LIBGC_PREFIX env.
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

    // Step 3: Locate tyra_runtime.lib next to the running compiler.
    let runtime_lib_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("tyra_runtime.lib")));

    // Step 4: Build lld-link.exe argument list.
    // Output is always an .exe on Windows.
    //
    // CRT resolution: lld-link does not auto-discover CRT/Windows SDK libraries; the LIB
    // environment variable must cover them (set by VsDevCmd.bat / vcvarsall.bat in CI).
    // We explicitly request the standard MSVC dynamic CRT libraries so the linker can
    // resolve them even when only a subset of the LIB paths is populated:
    //   ucrt.lib     — Universal CRT (printf, malloc, …)
    //   msvcrt.lib   — MSVC runtime startup / glue
    //   vcruntime.lib — MSVC compiler-support intrinsics
    //   kernel32.lib — core Win32 API (ExitProcess, VirtualAlloc, …)
    //   ole32.lib    — COM basics required by some Windows init paths
    // These are /DEFAULTLIB (weaker than direct reference) so the linker uses them only
    // when the symbol would otherwise be unresolved.
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

    // Include runtime staticlib if present; emit diagnostic if missing.
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
            let _ = std::fs::remove_file(&obj_path);
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    }

    let link_result = Command::new("lld-link.exe").args(&link_args).output();

    // Clean up obj file.
    let _ = std::fs::remove_file(&obj_path);

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

    // Step 5: Copy gc.dll to the output directory so the .exe can find it at runtime.
    // Non-fatal: a warning is emitted if the DLL cannot be found.
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
///
/// Writes `<output_path>.tyra-covmap` alongside the binary.  Run the binary
/// with `TYRA_COV_DIR=<dir>` to get per-process `.covraw` counter files;
/// merge them and produce a report with `tyra_codegen_llvm::merge_covraw` +
/// `format_report`.
pub fn compile_to_binary_coverage(source_path: &Path, output_path: &Path) -> CompileResult {
    let (result, covmap_opt) = compile_to_ir_coverage(source_path);
    if !result.success {
        return result;
    }

    let covmap_text = covmap_opt.unwrap_or_default();
    let covmap_path = output_path.with_extension("tyra-covmap");
    if let Err(e) = std::fs::write(&covmap_path, &covmap_text) {
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

    let llvm_ir = result.llvm_ir.as_ref().unwrap();
    let ir_path = output_path.with_extension("ll");
    if let Err(e) = std::fs::write(&ir_path, llvm_ir) {
        let mut report = result.report;
        report.add(
            tyra_diagnostics::Diagnostic::error(format!(
                "cannot write IR `{}`: {e}",
                ir_path.display()
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

    // Build clang args (debug, not static — coverage binaries are always -O0).
    let mut clang_args: Vec<String> = vec![
        ir_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
        "-O0".into(),
    ];
    for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
        let lib_dir = format!("{prefix}/lib");
        if std::path::Path::new(&lib_dir).is_dir() {
            clang_args.push(format!("-L{lib_dir}"));
            break;
        }
    }
    let runtime_lib_path = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("libtyra_runtime.a")));
    match runtime_lib_path.as_ref() {
        Some(p) if p.exists() => {
            clang_args.push(p.to_string_lossy().into_owned());
        }
        _ => {
            let _ = std::fs::remove_file(&ir_path);
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(
                    "Tyra runtime staticlib not found for coverage build.",
                )
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
    let _ = std::fs::remove_file(&ir_path);

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

/// Result of running a pre-compiled binary.
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Run the binary at `binary_path` with the given `args`, optionally killing
/// it after `timeout_secs` seconds.  Does NOT compile anything and does NOT
/// delete the binary — that is left to the caller.
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

/// Compile and run a Tyra source file.
/// Result of running a Tyra program and capturing its stdout.
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

/// Compile `source_path` and run it, capturing stdout.
/// Unlike `run()`, this returns the program's standard output so callers
/// (e.g. the test runner) can parse it without requiring the binary to
/// communicate results via its exit code alone.
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
/// execution phase only (compilation is unlimited).  If the binary does not
/// exit within `timeout_secs`, it is killed and `timed_out: true` is returned.
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

pub fn run(source_path: &Path) -> CompileResult {
    run_opts(source_path, false)
}

pub fn run_release(source_path: &Path) -> CompileResult {
    run_opts(source_path, true)
}

fn run_opts(source_path: &Path, release: bool) -> CompileResult {
    let tmp_dir = std::env::temp_dir();
    let binary_path = tmp_dir.join(format!("tyra_run_{}", std::process::id()));

    let result = compile_to_binary_opts(source_path, &binary_path, release, false);
    if !result.success {
        return result;
    }

    // Execute the compiled binary
    let run_result = Command::new(&binary_path).status();

    // Clean up binary
    let _ = std::fs::remove_file(&binary_path);

    match run_result {
        Ok(status) => {
            if !status.success() {
                let mut report = result.report;
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "program exited with status {}",
                        status.code().unwrap_or(-1)
                    ))
                    .with_code("E0501"),
                );
                return CompileResult {
                    success: false,
                    report,
                    sources: result.sources,
                    llvm_ir: result.llvm_ir,
                };
            }
            result
        }
        Err(e) => {
            let mut report = result.report;
            report.add(
                tyra_diagnostics::Diagnostic::error(format!("cannot execute binary: {e}"))
                    .with_code("E0501"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_in_memory_clean_program() {
        let CheckResult { report, .. } = check_in_memory(
            "ok.tyra".into(),
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
            "bad.tyra".into(),
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
            check_in_memory("bad.tyra".into(), "let x = \n".into(), None);
        assert!(report.has_errors(), "expected parse error");
    }

    #[test]
    fn resolve_import_file_finds_local_and_skips_builtin() {
        use std::fs;
        let dir = std::env::temp_dir().join("tyra_driver_rif_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let foo_path = dir.join("foo.tyra");
        fs::write(&foo_path, "").unwrap();

        // local module resolves
        let got = resolve_import_file(&dir, &["foo".to_string()]);
        assert_eq!(
            got.as_deref(),
            Some(foo_path.as_path()),
            "should find foo.tyra"
        );

        // non-existent module
        let got = resolve_import_file(&dir, &["bar".to_string()]);
        assert!(got.is_none(), "should not find bar.tyra: {got:?}");

        // built-in module skipped
        let got = resolve_import_file(&dir, &["core".to_string(), "sys".to_string()]);
        assert!(got.is_none(), "core.sys is builtin, should return None");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_import_file_ambiguous_local_and_stdlib_returns_none() {
        // local/mymod.tyra  (layer a)
        // stdlib/mymod.tyra  (layer c, pinned via TYRA_STDLIB)
        // → 2 candidates → None
        use std::fs;

        // Drop-guard restores TYRA_STDLIB even if the test panics.
        struct EnvGuard {
            prev: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                // SAFETY: test binary runs tests in parallel threads, but this
                // particular variable is only written by this test (unique "mymod"
                // name ensures no other test in this binary touches TYRA_STDLIB for
                // the same import path). The guard guarantees restore on panic.
                // Add `#[serial]` (serial_test crate) if this assumption changes.
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
        fs::write(main_dir.join("mymod.tyra"), "export fn f() -> Unit\nend\n").unwrap();
        // The fake stdlib must pass is_tyra_stdlib (needs assert.tyra sentinel).
        let stdlib_dir = dir.path().join("stdlib");
        fs::create_dir(&stdlib_dir).unwrap();
        fs::write(stdlib_dir.join("assert.tyra"), "").unwrap();
        fs::write(
            stdlib_dir.join("mymod.tyra"),
            "export fn f() -> Unit\nend\n",
        )
        .unwrap();

        let _guard = EnvGuard {
            prev: std::env::var("TYRA_STDLIB").ok(),
        };
        // SAFETY: see EnvGuard::drop.
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
        // project/Tyra.toml  (dep mylib = { path = <lib_dir> })
        // project/src/main.tyra
        // <lib_dir>/Tyra.toml
        // <lib_dir>/src/mylib.tyra  (lib source)
        // resolve_import_file(project/src/, ["mylib"]) → Some(<lib_dir>/src/mylib.tyra)
        use std::fs;
        let project = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();

        fs::write(
            lib.path().join("Tyra.toml"),
            "[package]\nname    = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2026\"\n",
        )
        .unwrap();
        fs::create_dir(lib.path().join("src")).unwrap();
        let lib_src = lib.path().join("src/mylib.tyra");
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
        fs::write(src_dir.join("myapp.tyra"), "import mylib\n").unwrap();

        let result = resolve_import_file(&src_dir, &["mylib".to_string()]);
        assert!(result.is_some(), "path dep resolution must return Some");
        assert_eq!(
            result.unwrap().canonicalize().unwrap(),
            lib_src.canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_import_file_bin_path_dep_returns_none() {
        // A path dep whose root source has `fn main` must return None (E0218).
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
            bin_dep.path().join("src/mybin.tyra"),
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
        fs::write(src_dir.join("app.tyra"), "import mybin\n").unwrap();

        let result = resolve_import_file(&src_dir, &["mybin".to_string()]);
        assert!(
            result.is_none(),
            "bin path dep must return None, got {result:?}"
        );
    }

    #[test]
    fn auto_import_detects_module_call_inside_propagate() {
        // `string.parse_int(...)?` — module call wrapped in `?`.
        // Before the fix, walk_expr did not recurse into Propagate,
        // so `import string` was never injected → E0200.
        let CheckResult { report, .. } = check_in_memory(
            "p.tyra".into(),
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
        let CheckResult { report, .. } = check_in_memory("c.tyra".into(), src.into(), None);
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
        let CheckResult { report, .. } = check_in_memory("c.tyra".into(), src.into(), None);
        assert!(
            !report.has_errors(),
            "unexpected errors: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn continue_outside_loop_emits_e0215() {
        let CheckResult { report, .. } = check_in_memory(
            "bad.tyra".into(),
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
        // Lambda body is an independent frame; outer loop's depth must not bleed in.
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
        let CheckResult { report, .. } = check_in_memory("bad.tyra".into(), src.into(), None);
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
        // Lambda body is an independent frame; outer loop's depth must not bleed in.
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
        let CheckResult { report, .. } = check_in_memory("bad.tyra".into(), src.into(), None);
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
        // Lambda's return type annotation must be checked against the lambda body,
        // not against the enclosing function's return type.
        // Key: outer fn returns String so that without lambda isolation,
        // `return "hello"` inside `fn() -> Unit` would be silently accepted
        // (String == String from the outer frame). With correct isolation the
        // lambda's own `-> Unit` annotation is used and String != Unit → E0309.
        let src = concat!(
            "fn outer() -> String\n",
            "  let f = fn() -> Unit\n",
            "    return \"hello\"\n",
            "  end\n",
            "  \"ok\"\n",
            "end\n",
        );
        let CheckResult { report, .. } = check_in_memory("bad.tyra".into(), src.into(), None);
        assert!(report.has_errors(), "expected E0309 in lambda");
        let codes: Vec<&str> = report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(codes.contains(&"E0309"), "expected E0309, got: {codes:?}");
    }
}
