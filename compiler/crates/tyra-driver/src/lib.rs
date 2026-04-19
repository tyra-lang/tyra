// tyra-driver: Compilation pipeline for the Tyra language.
//
// Pipeline: source -> lex -> parse -> resolve -> type check -> MIR -> LLVM IR -> binary
//
// spec reference: §19 (execution model)

use std::path::Path;
use std::process::Command;

use tyra_diagnostics::{Report, SourceMap};

/// Result of compilation.
pub struct CompileResult {
    pub success: bool,
    pub report: Report,
    pub sources: SourceMap,
    pub llvm_ir: Option<String>,
}

/// Compile a Tyra source file to LLVM IR text.
pub fn compile_to_ir(source_path: &Path) -> CompileResult {
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
            return CompileResult {
                success: false,
                report,
                sources,
                llvm_ir: None,
            };
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
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // Resolve imports: parse module files and merge exported items (§13)
    let main_dir = source_path.parent().unwrap_or(Path::new("."));
    resolve_imports(&mut ast, main_dir, &mut sources, &mut report);
    if report.has_errors() {
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // Name resolution
    tyra_resolve::resolve(&ast, &mut report);
    if report.has_errors() {
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // Type checking
    tyra_types::check(&ast, &mut report);
    if report.has_errors() {
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // MIR lowering
    let mir = tyra_mir::lower(&ast);

    // LLVM IR generation
    let llvm_ir = tyra_codegen_llvm::emit_llvm_ir(&mir);

    CompileResult {
        success: true,
        report,
        sources,
        llvm_ir: Some(llvm_ir),
    }
}

/// Find the stdlib directory by walking up from `main_dir` looking for a `stdlib/` folder.
/// Also checks the `TYRA_STDLIB` environment variable first.
fn find_stdlib_dir(main_dir: &Path) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("TYRA_STDLIB") {
        let pb = std::path::PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    let mut dir = main_dir.to_path_buf();
    loop {
        let candidate = dir.join("stdlib");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Resolve import declarations by parsing module files and merging exported items.
/// `import math` → parse `<main_dir>/math.tyra`, merge exported fns as `math__fn_name`.
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

    for imp in &imports {
        let local_name = imp
            .alias
            .as_deref()
            .or_else(|| imp.path.last().map(String::as_str))
            .unwrap_or("_unknown");

        // Check for built-in modules (core.sys, etc.)
        let module_key = imp.path.join(".");
        if is_builtin_module(&module_key) {
            // Built-in modules don't need file resolution.
            // The lowering and codegen layers handle their functions as builtins.
            continue;
        }

        // Resolve file path: import a.b.c → <main_dir>/a/b/c.tyra
        // Fallback: search stdlib directory (found by walking up from main_dir).
        let mut module_path = main_dir.to_path_buf();
        for segment in &imp.path {
            module_path.push(segment);
        }
        module_path.set_extension("tyra");

        let module_source = if let Ok(s) = std::fs::read_to_string(&module_path) {
            s
        } else if let Some(stdlib_dir) = find_stdlib_dir(main_dir) {
            let mut stdlib_path = stdlib_dir;
            for segment in &imp.path {
                stdlib_path.push(segment);
            }
            stdlib_path.set_extension("tyra");
            match std::fs::read_to_string(&stdlib_path) {
                Ok(s) => {
                    module_path = stdlib_path;
                    s
                }
                Err(_) => {
                    report.add(
                        tyra_diagnostics::Diagnostic::error(format!(
                            "cannot import `{}`: module not found",
                            imp.path.join(".")
                        ))
                        .with_code("E0200"),
                    );
                    continue;
                }
            }
        } else {
            report.add(
                tyra_diagnostics::Diagnostic::error(format!(
                    "cannot import `{}`: module not found",
                    imp.path.join(".")
                ))
                .with_code("E0200"),
            );
            continue;
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

/// Compile a Tyra source file to a native binary.
pub fn compile_to_binary(source_path: &Path, output_path: &Path) -> CompileResult {
    let result = compile_to_ir(source_path);
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

    // Compile with clang, linking Boehm GC (libgc). See ADR-0007.
    let mut clang_args: Vec<String> = vec![
        ir_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
        "-O0".into(),
    ];
    // Probe common libgc install prefixes. Homebrew on Apple Silicon and Intel
    // place libgc under different roots; Linux package managers use the default
    // search path.
    for prefix in ["/opt/homebrew/opt/bdw-gc", "/usr/local/opt/bdw-gc"] {
        let lib_dir = format!("{prefix}/lib");
        if std::path::Path::new(&lib_dir).is_dir() {
            clang_args.push(format!("-L{lib_dir}"));
            break;
        }
    }
    clang_args.push("-lgc".into());

    let clang_result = Command::new("clang").args(&clang_args).output();

    // Clean up IR file
    let _ = std::fs::remove_file(&ir_path);

    match clang_result {
        Ok(output) => {
            if output.status.success() {
                result
            } else {
                let mut report = result.report;
                let stderr = String::from_utf8_lossy(&output.stderr);
                // Detect missing libgc and surface an actionable diagnostic
                // instead of the raw linker error.
                let msg = if stderr.contains("-lgc") || stderr.contains("library 'gc'")
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
                report.add(
                    tyra_diagnostics::Diagnostic::error(msg).with_code("E0500"),
                );
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

/// Compile and run a Tyra source file.
pub fn run(source_path: &Path) -> CompileResult {
    let tmp_dir = std::env::temp_dir();
    let binary_path = tmp_dir.join(format!("tyra_run_{}", std::process::id()));

    let result = compile_to_binary(source_path, &binary_path);
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
