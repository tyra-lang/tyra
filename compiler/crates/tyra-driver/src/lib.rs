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

        // Resolve file path: import a.b.c → <main_dir>/a/b/c.tyra
        let mut module_path = main_dir.to_path_buf();
        for segment in &imp.path {
            module_path.push(segment);
        }
        module_path.set_extension("tyra");

        let module_source = match std::fs::read_to_string(&module_path) {
            Ok(s) => s,
            Err(e) => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "cannot import `{}`: cannot read `{}`: {e}",
                        imp.path.join("."),
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

    // Compile with clang
    let clang_result = Command::new("clang")
        .args([
            ir_path.to_str().unwrap(),
            "-o",
            output_path.to_str().unwrap(),
            "-O0",
        ])
        .output();

    // Clean up IR file
    let _ = std::fs::remove_file(&ir_path);

    match clang_result {
        Ok(output) => {
            if output.status.success() {
                result
            } else {
                let mut report = result.report;
                let stderr = String::from_utf8_lossy(&output.stderr);
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!("clang failed: {stderr}"))
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
