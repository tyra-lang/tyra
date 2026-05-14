// tyra-driver: Compilation pipeline for the Tyra language.
//
// Pipeline: source -> lex -> parse -> resolve -> type check -> MIR -> LLVM IR -> binary
//
// spec reference: §19 (execution model)

use std::path::Path;
use std::process::Command;

use tyra_diagnostics::{Report, SourceMap};
pub use tyra_ast::SourceFile;
pub use tyra_diagnostics::SourceId;
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
    CheckResult { report, sources, type_index, def_index, symbols: symbol_list, source_id, ast }
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

    // Auto-import obvious stdlib modules (string / list / io) when the
    // program calls `string.fn(...)` etc. but forgot the import. This
    // closes the most common E0200 hit in the AI-gen benchmark — the
    // model writes `string.trim(input)` directly without ever stating
    // `import string`. Adding the import is always safe (unused imports
    // are harmless) and converts a 5-error-per-run hot spot into an
    // auto-corrected program.
    auto_import_stdlib(&mut ast);

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
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // Name resolution
    let _ = tyra_resolve::resolve(&ast, &mut report);
    if report.has_errors() {
        return CompileResult {
            success: false,
            report,
            sources,
            llvm_ir: None,
        };
    }

    // Type checking
    let _ = tyra_types::check(&ast, &mut report);
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

/// Resolve `import a.b.c` to the `.tyra` file path the compiler would read,
/// using the same lookup order as `resolve_imports`:
///   1. `<main_dir>/a/b/c.tyra`
///   2. `<stdlib_dir>/a/b/c.tyra`  (via `TYRA_STDLIB` env or walk-up for `stdlib/`)
///
/// Returns `None` for built-in modules (`core.sys`, `core.tasks`) and paths
/// that do not exist on disk.
pub fn resolve_import_file(main_dir: &Path, path: &[String]) -> Option<std::path::PathBuf> {
    if is_builtin_module(&path.join(".")) {
        return None;
    }
    let mut p = main_dir.to_path_buf();
    for seg in path {
        p.push(seg);
    }
    p.set_extension("tyra");
    if p.is_file() {
        return Some(p);
    }
    let stdlib = find_stdlib_dir(main_dir)?;
    let mut sp = stdlib;
    for seg in path {
        sp.push(seg);
    }
    sp.set_extension("tyra");
    sp.is_file().then_some(sp)
}

/// Add `import string` / `import list` / `import io` automatically when the
/// program calls those module's functions (`string.trim(s)`, `list.push(xs, v)`,
/// `io.read_line()`) without an explicit import. The AI-gen benchmark shows
/// the model frequently forgets these imports; auto-adding them is harmless
/// (unused imports do not affect output) and removes a class of E0200 hits.
fn auto_import_stdlib(ast: &mut tyra_ast::SourceFile) {
    use tyra_ast::{Expr, ExprKind, Item, ImportDecl, Stmt};

    const AUTO: &[&str] = &["string", "list", "io"];

    // Collect already-imported single-segment module names.
    let mut already: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for item in &ast.items {
        if let Item::Import(imp) = item {
            if imp.path.len() == 1 {
                let local = imp
                    .alias
                    .as_deref()
                    .unwrap_or(&imp.path[0]);
                already.insert(local.to_string());
            }
        }
    }

    // Walk the AST collecting module names referenced by `<module>.<fn>(...)`.
    let mut needed: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    // Method names that are unambiguous markers for the string stdlib.
    // If any of these appear as `<expr>.<method>(...)` we conservatively
    // assume the receiver is a String and import the string module — this
    // catches the common pattern `line.byte_at(i)` even though we cannot
    // tell at parse time that `line` is in fact a String. False positives
    // (an `impl` block defining its own `byte_at`) just produce one extra
    // unused import, which is harmless.
    const STRING_METHOD_HINTS: &[&str] = &[
        "byte_at", "substring", "from_byte", "parse_int", "parse_errno",
        "starts_with", "ends_with", "to_upper", "to_lower", "is_empty",
        "trim",
    ];

    fn walk_expr(e: &Expr, needed: &mut std::collections::HashSet<String>) {
        match &e.kind {
            ExprKind::Call(callee, args) => {
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    if let ExprKind::Ident(name) = &obj.kind {
                        if matches!(name.as_str(), "string" | "list" | "io") {
                            needed.insert(name.clone());
                        }
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
            Stmt::Break(_) => {}
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
                Item::Import(i) => Some(i.span.clone()),
                Item::FnDef(f) => Some(f.span.clone()),
                Item::Stmt(s) => Some(stmt_span(s)),
                _ => None,
            })
            .unwrap_or_else(|| ast.span.clone());
        let mut prefix: Vec<Item> = to_add
            .into_iter()
            .map(|m| {
                Item::Import(ImportDecl {
                    path: vec![m.to_string()],
                    alias: None,
                    span: span.clone(),
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
                    if let PatternKind::Ident(new_name) = &f.pattern.kind {
                        if f.field_name == old_field && old_field != *new_name {
                            f.field_name = new_name.clone();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn substitute_in_expr(
        e: &mut Expr,
        renames: &std::collections::HashMap<String, String>,
    ) {
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

    fn substitute_in_stmt(
        s: &mut Stmt,
        renames: &std::collections::HashMap<String, String>,
    ) {
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
            Stmt::Break(_) => {}
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
            Stmt::Break(_) => {}
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

    let _ = (Pattern { kind: PatternKind::Wildcard, span: ast.span.clone() },
             PatternField { field_name: String::new(),
                             pattern: Pattern { kind: PatternKind::Wildcard, span: ast.span.clone() },
                             span: ast.span.clone() });

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
                    Stmt::Break(_) => {}
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
                    // The for-binding lives in MIR as a single function-wide
                    // alloca too, so treat it like a let for shadow purposes.
                    if self.introduced.contains(&f.binding) {
                        let new = self.fresh(&f.binding);
                        active.insert(f.binding.clone(), new.clone());
                        f.binding = new.clone();
                        self.introduced.insert(new);
                    } else {
                        self.introduced.insert(f.binding.clone());
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
        Stmt::Let(l) => l.span.clone(),
        Stmt::Mut(m) => m.span.clone(),
        Stmt::Return(r) => r.span.clone(),
        Stmt::Expr(e) => e.span.clone(),
        Stmt::Defer(d) => d.span.clone(),
        Stmt::Break(b) => b.span.clone(),
    }
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

    // Compile with clang, linking Boehm GC (libgc, ADR-0007) and the Tyra
    // async runtime staticlib (libtyra_runtime.a, M9). The runtime is built
    // by cargo into the same target/ directory as the `tyra` binary itself,
    // so we locate it relative to the current executable.
    let mut clang_args: Vec<String> = vec![
        ir_path.to_str().unwrap().into(),
        "-o".into(),
        output_path.to_str().unwrap().into(),
        "-O0".into(),
    ];
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
    // target/{debug,release}/). If it is missing (e.g. the user installed
    // only the binary via `cargo install`, or built with
    // `cargo build -p tyra-cli`), surface an explicit Tyra diagnostic
    // instead of letting clang emit an unresolved-symbol error.
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
            let _ = std::fs::remove_file(&ir_path);
            return CompileResult {
                success: false,
                report,
                sources: result.sources,
                llvm_ir: result.llvm_ir,
            };
        }
    }
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
        assert!(!report.has_errors(), "unexpected errors: {:?}", report.diagnostics());
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
        let CheckResult { report, .. } = check_in_memory(
            "bad.tyra".into(),
            "let x = \n".into(),
            None,
        );
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
        assert_eq!(got.as_deref(), Some(foo_path.as_path()), "should find foo.tyra");

        // non-existent module
        let got = resolve_import_file(&dir, &["bar".to_string()]);
        assert!(got.is_none(), "should not find bar.tyra: {got:?}");

        // built-in module skipped
        let got = resolve_import_file(&dir, &["core".to_string(), "sys".to_string()]);
        assert!(got.is_none(), "core.sys is builtin, should return None");

        let _ = fs::remove_dir_all(&dir);
    }
}
