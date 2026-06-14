//! tyra-frontend: wasm-safe frontend pipeline (parse → check → MIR).
//!
//! Contains the pure, I/O-free stages of the Tyra compiler:
//! lexing, parsing, desugaring, import resolution (via the `ImportLoader`
//! trait), name resolution, type checking, and MIR lowering.
//!
//! The only impure seam — filesystem or bundled-source access — is abstracted
//! behind `ImportLoader`, injected by callers:
//!
//! - `FsImportLoader` (in `tyra-driver`): discovers `.ty` files via the
//!   ADR 0010 three-layer rule.
//! - `BundledImportLoader` (in `tyra-wasm`): returns sources embedded with
//!   `include_str!`.
//! - `NoImportLoader` (this crate): skips resolution entirely (LSP single-file
//!   mode, `workspace_dir = None` equivalent).

// Re-exports for callers that hold frontend types without depending on the
// sub-crates directly.
pub use tyra_ast::SourceFile;
pub use tyra_diagnostics::{Label, Report, SourceId, SourceMap};
pub use tyra_mir::Program;
pub use tyra_resolve::{CompletionKind, DefIndex, SymbolList};
pub use tyra_resolve::{PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
pub use tyra_types::{Ty, TypeIndex};

// ─────────────────────────────────────────────────────────────────────────────
// Import loader abstraction
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of resolving a dotted module path through an [`ImportLoader`].
///
/// Preserves the three-way distinction the native driver makes (E0200 /
/// E0217 / E0218) so diagnostic quality is identical across native and
/// Playground.  The loader returns *data only*; all diagnostic code
/// assignment, message templating, and span attachment is done by the
/// frontend (here), not by the loader implementation.
#[derive(Debug)]
pub enum ImportResolution {
    /// Exactly one candidate found.  `display_name` is used as the source-map
    /// label (a file path on native, a synthetic name for bundled modules).
    Found {
        display_name: String,
        source: String,
    },
    /// No candidate found → frontend emits E0200 "module not found".
    NotFound,
    /// Two or more candidates found → frontend emits E0217 E_IMPORT_AMBIGUOUS.
    /// `locations` are human-readable descriptions of each candidate.
    Ambiguous { locations: Vec<String> },
    /// A manifest dependency entry was rejected at compile time (ADR 0009 /
    /// ADR 0010) → frontend emits E0218.  `dep` is the dependency key name.
    DepError { dep: String, kind: DepErrorKind },
    /// Candidate was identified but could not be read → frontend emits E0200
    /// with `detail` appended.
    ReadError { detail: String },
}

/// Reason a dependency failed the compile-time gate (E0218).
#[derive(Debug)]
pub enum DepErrorKind {
    /// The dependency is a binary package (ADR 0009 E_DEP_NOT_IMPORTABLE).
    BinPackage,
    /// `package.name` in the cached manifest does not match the dependency key
    /// (ADR 0010 no-alias rule).  Run `tyra mod sync` to fix.
    NameMismatch,
}

/// Resolves a dotted module path to its source text.
///
/// The trait is object-safe (`dyn ImportLoader`) so callers can inject
/// implementations without generics.
///
/// Implementors must return *data only* — they must not emit diagnostics
/// themselves.
pub trait ImportLoader {
    /// Resolve a dotted module path such as `["float"]` or `["json"]` to its
    /// source text.
    ///
    /// `path` is the raw `imp.path` after alias lookup.  Built-in modules
    /// (`core.sys`, `core.tasks`) are filtered by the frontend before this
    /// method is called and will never appear here.
    fn resolve(&self, path: &[String]) -> ImportResolution;

    /// Return `true` to skip import resolution entirely (no E0200 emitted).
    ///
    /// Matches the legacy `workspace_dir = None` path in
    /// `tyra_driver::check_in_memory`: the resolver still runs but no module
    /// sources are merged — it is as if no `import` statements exist.
    ///
    /// Default: `false`.  Override to `true` only in `NoImportLoader`.
    fn skip_resolution(&self) -> bool {
        false
    }
}

/// An [`ImportLoader`] that skips import resolution entirely.
///
/// Used by `source_to_mir` and by the LSP for single-file diagnostics where
/// no workspace directory is available.  Equivalent to the legacy
/// `workspace_dir = None` path in `tyra_driver::check_in_memory`.
pub struct NoImportLoader;

impl ImportLoader for NoImportLoader {
    fn resolve(&self, _path: &[String]) -> ImportResolution {
        ImportResolution::NotFound
    }

    fn skip_resolution(&self) -> bool {
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public result types
// ─────────────────────────────────────────────────────────────────────────────

/// Output of the check phase (lex → parse → desugar → resolve → type-check).
///
/// This is the canonical result type shared by the driver, LSP, and
/// completions; `tyra_driver::CheckResult` is a re-export alias.
pub struct CheckOutput {
    pub report: Report,
    pub sources: SourceMap,
    pub type_index: TypeIndex,
    pub def_index: DefIndex,
    pub symbols: SymbolList,
    pub source_id: SourceId,
    pub ast: SourceFile,
}

/// Output of the complete frontend pipeline (check + MIR lowering).
pub struct FrontendResult {
    pub check: CheckOutput,
    /// `None` when the check phase produced errors or MIR lowering failed.
    pub program: Option<Program>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Run the check phase (lex → parse → desugar → import-resolve → name-resolve
/// → type-check) on a single in-memory source.
///
/// Import statements are resolved via `loader`; pass `&NoImportLoader` to skip
/// import resolution (suitable for single-file LSP diagnostics where no
/// workspace is available).
pub fn check_unit(
    main_name: &str,
    main_source: &str,
    loader: &dyn ImportLoader,
) -> CheckOutput {
    let mut sources = SourceMap::new();
    let mut report = Report::new();

    let source_id = sources.add(main_name.to_string(), main_source.to_string());

    let mut ast = tyra_parser::parse(source_id, &sources, &mut report);
    if report.has_errors() {
        return CheckOutput {
            report,
            sources,
            type_index: TypeIndex::new(),
            def_index: DefIndex::new(),
            symbols: SymbolList::new(),
            source_id,
            ast,
        };
    }

    auto_import_stdlib(&mut ast);
    desugar_test_blocks(&mut ast);
    rename_pattern_bindings(&mut ast);
    rename_let_shadows(&mut ast);

    if !loader.skip_resolution() {
        resolve_imports_with_loader(&mut ast, &mut sources, &mut report, loader);
        if report.has_errors() {
            return CheckOutput {
                report,
                sources,
                type_index: TypeIndex::new(),
                def_index: DefIndex::new(),
                symbols: SymbolList::new(),
                source_id,
                ast,
            };
        }
    }

    let (def_index, symbol_list) = tyra_resolve::resolve(&ast, &mut report);
    if report.has_errors() {
        return CheckOutput {
            report,
            sources,
            type_index: TypeIndex::new(),
            def_index,
            symbols: symbol_list,
            source_id,
            ast,
        };
    }

    let type_index = tyra_types::check(&ast, &mut report);
    CheckOutput {
        report,
        sources,
        type_index,
        def_index,
        symbols: symbol_list,
        source_id,
        ast,
    }
}

/// Run the complete frontend pipeline (check + MIR lowering) on a single
/// in-memory source.
///
/// Import statements are resolved via `loader`; pass `&NoImportLoader` to skip
/// import resolution.
///
/// `FrontendResult::program` is `None` when the check phase produced errors or
/// when MIR lowering emitted errors.
pub fn compile_unit(
    main_name: &str,
    main_source: &str,
    loader: &dyn ImportLoader,
) -> FrontendResult {
    let mut check = check_unit(main_name, main_source, loader);
    if check.report.has_errors() {
        return FrontendResult {
            check,
            program: None,
        };
    }

    // ADR-0029: rewrite `fn main() -> Result<Unit, E>` after type checking
    // but before MIR lowering.
    desugar_err_main(&mut check.ast, &mut check.sources, &mut check.report);
    if check.report.has_errors() {
        return FrontendResult {
            check,
            program: None,
        };
    }

    let program = tyra_mir::lower(&check.ast, &check.sources);
    for diag in program.lower_errors.iter().cloned() {
        check.report.add(diag);
    }
    if check.report.has_errors() {
        return FrontendResult {
            check,
            program: None,
        };
    }

    FrontendResult {
        check,
        program: Some(program),
    }
}

/// Convenience wrapper: run the complete frontend pipeline on a single source
/// string with no import resolution.
///
/// Equivalent to `compile_unit(file_name, source, &NoImportLoader)`.
pub fn source_to_mir(file_name: &str, source: &str) -> FrontendResult {
    compile_unit(file_name, source, &NoImportLoader)
}

// ─────────────────────────────────────────────────────────────────────────────
// Import resolution (pure — delegates data fetching to ImportLoader)
// ─────────────────────────────────────────────────────────────────────────────

fn resolve_imports_with_loader(
    ast: &mut tyra_ast::SourceFile,
    sources: &mut SourceMap,
    report: &mut Report,
    loader: &dyn ImportLoader,
) {
    use tyra_ast::Item;

    // Collect imports first (to avoid borrowing ast while mutating).
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

        // Built-in modules (core.sys, core.tasks) need no file resolution.
        if is_builtin_module(&imp.path.join(".")) {
            continue;
        }

        match loader.resolve(&imp.path) {
            ImportResolution::DepError { dep, kind } => {
                let msg = match kind {
                    DepErrorKind::BinPackage => format!(
                        "cannot import `{}`: dependency `{dep}` is a bin package \
                         and cannot be imported",
                        imp.path.join(".")
                    ),
                    DepErrorKind::NameMismatch => format!(
                        "cannot import `{}`: cached dependency `{dep}` has a name mismatch \
                         (package.name does not equal the dependency key); run `tyra mod sync`",
                        imp.path.join(".")
                    ),
                };
                report.add(
                    tyra_diagnostics::Diagnostic::error(msg)
                        .with_code("E0218")
                        .with_label(Label::new(imp.span, "this import")),
                );
                continue;
            }
            ImportResolution::NotFound => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "cannot import `{}`: module not found",
                        imp.path.join(".")
                    ))
                    .with_code("E0200")
                    .with_label(Label::new(imp.span, "this import")),
                );
                continue;
            }
            ImportResolution::Ambiguous { locations } => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "import `{}` is ambiguous: found in {} locations: {}",
                        imp.path.join("."),
                        locations.len(),
                        locations.join(", "),
                    ))
                    .with_code("E0217")
                    .with_label(Label::new(imp.span, "this import")),
                );
                continue;
            }
            ImportResolution::ReadError { detail } => {
                report.add(
                    tyra_diagnostics::Diagnostic::error(format!(
                        "cannot import `{}`: {detail}",
                        imp.path.join(".")
                    ))
                    .with_code("E0200")
                    .with_label(Label::new(imp.span, "this import")),
                );
                continue;
            }
            ImportResolution::Found {
                display_name,
                source,
            } => {
                let module_id = sources.add(display_name, source);
                let module_ast = tyra_parser::parse(module_id, sources, report);
                if report.has_errors() {
                    return;
                }

                // Merge exported items with mangled names.
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
                            // impl blocks are always included (no export on impl).
                            merged_items.push(Item::ImplDef(impl_def));
                        }
                        _ => {
                            // Non-exported items and statements are skipped.
                        }
                    }
                }
            }
        }
    }

    // Append merged items to the main AST.
    ast.items.extend(merged_items);
}

// ─────────────────────────────────────────────────────────────────────────────
// AST transformation passes (moved from tyra-driver)
// ─────────────────────────────────────────────────────────────────────────────

/// Check if a module path refers to a compiler built-in module.
pub fn is_builtin_module(module_path: &str) -> bool {
    matches!(module_path, "core.sys" | "core.tasks")
}

/// Add `import string` / `import list` / `import io` automatically when the
/// program calls those module's functions (`string.trim(s)`, `list.push(xs, v)`,
/// `io.read_line()`) without an explicit import. The AI-gen benchmark shows
/// the model frequently forgets these imports; auto-adding them is harmless
/// (unused imports do not affect output) and removes a class of E0200 hits.
pub fn auto_import_stdlib(ast: &mut tyra_ast::SourceFile) {
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
            Stmt::TupleLet(l) => walk_expr(&l.value, needed),
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

fn stmt_span(s: &tyra_ast::Stmt) -> tyra_ast::Span {
    use tyra_ast::Stmt;
    match s {
        Stmt::Let(l) => l.span,
        Stmt::TupleLet(l) => l.span,
        Stmt::Mut(m) => m.span,
        Stmt::Return(r) => r.span,
        Stmt::Expr(e) => e.span,
        Stmt::Defer(d) => d.span,
        Stmt::Break(b) => b.span,
        Stmt::Continue(c) => c.span,
    }
}

/// Desugar `test "name" [panics] ... end` blocks (ADR 0013) into regular `fn`
/// definitions.
///
/// Each `Item::TestDef` is converted to an `Item::FnDef` with:
/// - Name: `test__<sanitized>` or `test_panics__<sanitized>`.
/// - Return type: `Result<Unit, String>`.
/// - Body: the original stmts followed by a synthetic `Ok(())`.
///
/// Call this after import resolution but before name resolution so that
/// the resolver, type-checker, MIR, and codegen never see `Item::TestDef`.
pub fn desugar_test_blocks(ast: &mut tyra_ast::SourceFile) {
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
/// references inside the arm body.
pub fn rename_pattern_bindings(ast: &mut tyra_ast::SourceFile) {
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
            Stmt::TupleLet(l) => substitute_in_expr(&mut l.value, renames),
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
            Stmt::TupleLet(l) => process_expr(&mut l.value, counter),
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

/// Rename `let X` / `mut X` whose name has already been introduced earlier in
/// the same function to `<orig>__l<N>`, substituting references in scope.
///
/// Without this pass two `let X` with different types collapse onto a single
/// function-scoped `%X` alloca and LLVM rejects with E0500.  Companion to
/// `rename_pattern_bindings` which handles the same problem for match-arm
/// pattern bindings.
pub fn rename_let_shadows(ast: &mut tyra_ast::SourceFile) {
    use std::collections::{HashMap, HashSet};
    use tyra_ast::{ElseBranch, Expr, ExprKind, Item, Pattern, PatternKind, Stmt, StringPart};

    struct Pass {
        counter: u32,
        // Function-wide set of names already bound (mirrors MIR hoist).
        introduced: HashSet<String>,
    }

    impl Pass {
        fn fresh(&mut self, orig: &str) -> String {
            self.counter += 1;
            format!("{orig}__l{}", self.counter)
        }

        fn rewrite_ident(name: &mut String, active: &HashMap<String, String>) {
            if let Some(new) = active.get(name) {
                *name = new.clone();
            }
        }

        fn walk_stmts(&mut self, stmts: &mut [Stmt], active: &mut HashMap<String, String>) {
            for stmt in stmts.iter_mut() {
                match stmt {
                    Stmt::Let(l) => {
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
                    Stmt::TupleLet(l) => {
                        self.walk_expr(&mut l.value, active);
                        for name in &mut l.bindings {
                            if self.introduced.contains(name.as_str()) {
                                let new = self.fresh(name);
                                active.insert(name.clone(), new.clone());
                                *name = new.clone();
                                self.introduced.insert(new);
                            } else {
                                self.introduced.insert(name.clone());
                            }
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
                        // same final name is detected as a shadow.
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
                    for name in f.bindings.idents_mut() {
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
                ExprKind::Propagate(inner)
                | ExprKind::Await(inner)
                | ExprKind::Spawn(inner) => {
                    self.walk_expr(inner, active);
                }
                ExprKind::Lambda(lam) => {
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
    // shadowing only collides within a single MIR function.
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
            _ => {}
        }
    }
    // Walk top-level statements as a single scope.
    pass.introduced.clear();
    let mut active = HashMap::new();
    for item in &mut ast.items {
        if let Item::Stmt(s) = item {
            pass.walk_stmts(std::slice::from_mut(s), &mut active);
        }
    }
}

/// If `type_name` is a locally-defined non-generic ADT, return its variants.
/// Used by `desugar_err_main` to emit variant-name display for Err payloads.
fn find_local_adt_variants<'a>(
    items: &'a [tyra_ast::Item],
    type_name: &str,
) -> Option<&'a Vec<tyra_ast::Variant>> {
    use tyra_ast::{Item, TypeDefKind};
    for item in items {
        if let Item::TypeDef(t) = item
            && t.name == type_name
            && t.type_params.is_empty()
            && let TypeDefKind::Adt(variants) = &t.kind
        {
            return Some(variants);
        }
    }
    None
}

/// Generate Tyra statements that bind `__tyra_err_str` to the variant name of
/// the ADT value in `binding` via a match, then call eprintln with it.
fn adt_variant_name_report(variants: &[tyra_ast::Variant], binding: &str) -> String {
    let arms = variants
        .iter()
        .map(|v| {
            if v.fields.is_empty() {
                format!("  when {}\n    \"{}\"", v.name, v.name)
            } else {
                let wildcards = v
                    .fields
                    .iter()
                    .map(|f| format!("{}: _", f.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("  when {}({})\n    \"{}\"", v.name, wildcards, v.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "let __tyra_err_str = match {binding}\n{arms}\n  end\neprintln(\"error: #{{__tyra_err_str}}\")"
    )
}

/// Minimal type-expression renderer for ADR-0029's type-name fallback.
fn type_expr_display(te: &tyra_ast::TypeExpr) -> String {
    use tyra_ast::TypeExprKind;
    match &te.kind {
        TypeExprKind::Named(n) => n.clone(),
        TypeExprKind::Generic(n, args) => {
            let inner = args
                .iter()
                .map(type_expr_display)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{n}<{inner}>")
        }
        _ => "?".to_string(),
    }
}

/// ADR-0029: `fn main() -> Result<Unit, E>` returning `Err` must report to
/// stderr and exit 1.  The original main is renamed `__tyra_main_inner` and a
/// synthesized wrapper becomes the entry point.
///
/// Must run AFTER type checking and BEFORE MIR lowering.
pub fn desugar_err_main(
    ast: &mut tyra_ast::SourceFile,
    sources: &mut SourceMap,
    report: &mut Report,
) {
    use tyra_ast::{Item, TypeExprKind};

    let (err_te, is_async) = {
        let Some(f) = ast.items.iter().find_map(|item| {
            if let Item::FnDef(f) = item
                && f.name == "main"
            {
                Some(f)
            } else {
                None
            }
        }) else {
            return;
        };
        let Some(ret) = &f.return_type else { return };
        let TypeExprKind::Generic(ret_name, ret_args) = &ret.kind else {
            return;
        };
        if ret_name != "Result" || ret_args.len() != 2 {
            return;
        }
        (ret_args[1].clone(), f.is_async)
    };

    // Displayable judged by the SAME boundary the checker uses
    // (Ty::is_interp_displayable, cf. E0314/E0319).
    let err_ty = tyra_types::Ty::from_type_expr(&err_te);
    let displayable = err_ty.is_interp_displayable();
    let err_report = if displayable {
        r##"eprintln("error: #{__tyra_err}")"##.to_string()
    } else if let tyra_types::Ty::Named(ref type_name) = err_ty {
        if let Some(variants) = find_local_adt_variants(&ast.items, type_name) {
            adt_variant_name_report(variants, "__tyra_err")
        } else {
            format!(
                r##"eprintln("error: main returned Err({})")"##,
                type_expr_display(&err_te)
            )
        }
    } else {
        format!(
            r##"eprintln("error: main returned Err({})")"##,
            type_expr_display(&err_te)
        )
    };

    // Rename the definition itself…
    for item in &mut ast.items {
        if let Item::FnDef(f) = item
            && f.name == "main"
        {
            f.name = "__tyra_main_inner".into();
            break;
        }
    }
    // …then follow the rename through every free `main` reference in the file.
    rename_free_fn_refs(ast, "main", "__tyra_main_inner");

    let call = if is_async {
        "__tyra_main_inner().await"
    } else {
        "__tyra_main_inner()"
    };
    let async_kw = if is_async { "async " } else { "" };
    let wrapper_src = format!(
        "{async_kw}fn main() -> Unit
  match {call}
  when Ok(_)
    ()
  when Err(__tyra_err)
    {err_report}
    sys__exit(1)
  end
end
"
    );
    let sid = sources.add("<err-main-wrapper>".into(), wrapper_src);
    let wrapper_ast = tyra_parser::parse(sid, sources, report);
    ast.items.extend(wrapper_ast.items);
}

/// Rewrite every FREE reference to function `target` into `replacement` across
/// all function bodies, impl methods, and top-level statements.  Used by
/// ADR-0029's main rename.
pub fn rename_free_fn_refs(ast: &mut tyra_ast::SourceFile, target: &str, replacement: &str) {
    use std::collections::HashSet;
    use tyra_ast::{ElseBranch, Expr, ExprKind, Item, Pattern, PatternKind, Stmt, StringPart};

    struct Pass<'a> {
        target: &'a str,
        replacement: &'a str,
    }

    impl Pass<'_> {
        fn collect_pattern_idents(p: &Pattern, out: &mut HashSet<String>) {
            match &p.kind {
                PatternKind::Ident(name) => {
                    out.insert(name.clone());
                }
                PatternKind::Constructor(_, fields) => {
                    for f in fields {
                        Self::collect_pattern_idents(&f.pattern, out);
                    }
                }
                PatternKind::Tuple(elems) => {
                    for e in elems {
                        Self::collect_pattern_idents(e, out);
                    }
                }
                _ => {}
            }
        }

        fn walk_stmts(&self, stmts: &mut [Stmt], bound: &mut HashSet<String>) {
            for stmt in stmts.iter_mut() {
                match stmt {
                    Stmt::Let(l) => {
                        self.walk_expr(&mut l.value, bound);
                        bound.insert(l.name.clone());
                    }
                    Stmt::Mut(m) => {
                        self.walk_expr(&mut m.value, bound);
                        bound.insert(m.name.clone());
                    }
                    Stmt::TupleLet(l) => {
                        self.walk_expr(&mut l.value, bound);
                        for name in &l.bindings {
                            bound.insert(name.clone());
                        }
                    }
                    Stmt::Expr(e) => self.walk_expr(&mut e.expr, bound),
                    Stmt::Return(r) => {
                        if let Some(v) = &mut r.value {
                            self.walk_expr(v, bound);
                        }
                    }
                    Stmt::Defer(d) => self.walk_expr(&mut d.expr, bound),
                    Stmt::Break(_) | Stmt::Continue(_) => {}
                }
            }
        }

        fn walk_expr(&self, e: &mut Expr, bound: &mut HashSet<String>) {
            match &mut e.kind {
                ExprKind::Ident(name) => {
                    if name == self.target && !bound.contains(self.target) {
                        *name = self.replacement.to_string();
                    }
                }
                ExprKind::Call(callee, args) => {
                    self.walk_expr(callee, bound);
                    for a in args {
                        self.walk_expr(&mut a.value, bound);
                    }
                }
                ExprKind::TurbofishCall(callee, _, args) => {
                    self.walk_expr(callee, bound);
                    for a in args {
                        self.walk_expr(&mut a.value, bound);
                    }
                }
                ExprKind::FieldAccess(obj, _) => self.walk_expr(obj, bound),
                ExprKind::BinaryOp(l, _, r) => {
                    self.walk_expr(l, bound);
                    self.walk_expr(r, bound);
                }
                ExprKind::UnaryOp(_, inner) => self.walk_expr(inner, bound),
                ExprKind::Assign(l, r) => {
                    self.walk_expr(l, bound);
                    self.walk_expr(r, bound);
                }
                ExprKind::If(i) => {
                    self.walk_expr(&mut i.condition, bound);
                    let saved = bound.clone();
                    self.walk_stmts(&mut i.then_body, bound);
                    *bound = saved.clone();
                    if let Some(eb) = &mut i.else_body {
                        self.walk_else(eb, bound);
                        *bound = saved;
                    }
                }
                ExprKind::Match(m) => {
                    self.walk_expr(&mut m.subject, bound);
                    for arm in &mut m.arms {
                        let mut arm_bound = bound.clone();
                        Self::collect_pattern_idents(&arm.pattern, &mut arm_bound);
                        self.walk_stmts(&mut arm.body, &mut arm_bound);
                    }
                }
                ExprKind::While(w) => {
                    self.walk_expr(&mut w.condition, bound);
                    let saved = bound.clone();
                    self.walk_stmts(&mut w.body, bound);
                    *bound = saved;
                }
                ExprKind::For(f) => {
                    self.walk_expr(&mut f.iter, bound);
                    let mut body_bound = bound.clone();
                    for name in f.bindings.idents() {
                        body_bound.insert(name.clone());
                    }
                    self.walk_stmts(&mut f.body, &mut body_bound);
                }
                ExprKind::ListLit(items) => {
                    for it in items {
                        self.walk_expr(it, bound);
                    }
                }
                ExprKind::MapLit(pairs) => {
                    for (k, v) in pairs {
                        self.walk_expr(k, bound);
                        self.walk_expr(v, bound);
                    }
                }
                ExprKind::StringInterp(parts) => {
                    for p in parts {
                        if let StringPart::Expr(inner) = p {
                            self.walk_expr(inner, bound);
                        }
                    }
                }
                ExprKind::Index(obj, idx) => {
                    self.walk_expr(obj, bound);
                    self.walk_expr(idx, bound);
                }
                ExprKind::Propagate(inner)
                | ExprKind::Await(inner)
                | ExprKind::Spawn(inner) => {
                    self.walk_expr(inner, bound);
                }
                ExprKind::Tuple(items) => {
                    for it in items {
                        self.walk_expr(it, bound);
                    }
                }
                ExprKind::Lambda(lam) => {
                    let mut lam_bound = bound.clone();
                    for p in &lam.params {
                        lam_bound.insert(p.name.clone());
                    }
                    self.walk_stmts(&mut lam.body, &mut lam_bound);
                }
                _ => {}
            }
        }

        fn walk_else(&self, eb: &mut ElseBranch, bound: &mut HashSet<String>) {
            match eb {
                ElseBranch::Else(stmts) => self.walk_stmts(stmts, bound),
                ElseBranch::ElseIf(i) => {
                    self.walk_expr(&mut i.condition, bound);
                    let saved = bound.clone();
                    self.walk_stmts(&mut i.then_body, bound);
                    *bound = saved.clone();
                    if let Some(inner) = &mut i.else_body {
                        self.walk_else(inner, bound);
                        *bound = saved;
                    }
                }
            }
        }
    }

    let pass = Pass {
        target,
        replacement,
    };
    for item in &mut ast.items {
        match item {
            Item::FnDef(f) => {
                let mut bound: HashSet<String> =
                    f.params.iter().map(|p| p.name.clone()).collect();
                pass.walk_stmts(&mut f.body, &mut bound);
            }
            Item::ImplDef(impl_def) => {
                for method in &mut impl_def.methods {
                    let mut bound: HashSet<String> =
                        method.params.iter().map(|p| p.name.clone()).collect();
                    bound.insert("self".to_string());
                    pass.walk_stmts(&mut method.body, &mut bound);
                }
            }
            Item::Stmt(stmt) => {
                let mut bound = HashSet::new();
                pass.walk_stmts(std::slice::from_mut(stmt), &mut bound);
            }
            _ => {}
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mock loader helpers ───────────────────────────────────────────────────

    struct NotFoundLoader;
    impl ImportLoader for NotFoundLoader {
        fn resolve(&self, _path: &[String]) -> ImportResolution {
            ImportResolution::NotFound
        }
    }

    struct FoundLoader {
        source: String,
    }
    impl ImportLoader for FoundLoader {
        fn resolve(&self, _path: &[String]) -> ImportResolution {
            ImportResolution::Found {
                display_name: "mock.ty".into(),
                source: self.source.clone(),
            }
        }
    }

    struct AmbiguousLoader;
    impl ImportLoader for AmbiguousLoader {
        fn resolve(&self, _path: &[String]) -> ImportResolution {
            ImportResolution::Ambiguous {
                locations: vec!["`a/mock.ty`".into(), "`stdlib/mock.ty`".into()],
            }
        }
    }

    struct DepErrorLoader;
    impl ImportLoader for DepErrorLoader {
        fn resolve(&self, _path: &[String]) -> ImportResolution {
            ImportResolution::DepError {
                dep: "mybin".into(),
                kind: DepErrorKind::BinPackage,
            }
        }
    }

    // ── compile_unit / source_to_mir ─────────────────────────────────────────

    #[test]
    fn source_to_mir_basic_program() {
        let fr = source_to_mir("test.ty", "fn main() -> Unit\n  print(\"hi\")\nend\n");
        assert!(
            !fr.check.report.has_errors(),
            "unexpected errors: {:?}",
            fr.check.report.diagnostics()
        );
        assert!(fr.program.is_some(), "expected a Program");
    }

    #[test]
    fn compile_unit_no_import_loader_skips_import_resolution() {
        // With NoImportLoader, `import unknown_module` must NOT produce E0200.
        let fr = compile_unit(
            "test.ty",
            "import unknown_module\nfn main() -> Unit\n  print(\"hi\")\nend\n",
            &NoImportLoader,
        );
        let codes: Vec<&str> = fr
            .check
            .report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(
            !codes.contains(&"E0200"),
            "NoImportLoader must skip resolution — no E0200 expected, got: {codes:?}"
        );
    }

    #[test]
    fn compile_unit_with_found_loader_resolves_import() {
        let module_src = "export fn greet() -> Unit\n  print(\"hello from mock\")\nend\n";
        let main_src =
            "import mock\nfn main() -> Unit\n  mock__greet()\nend\n";
        let fr = compile_unit("main.ty", main_src, &FoundLoader { source: module_src.into() });
        assert!(
            !fr.check.report.has_errors(),
            "expected clean compile with FoundLoader, got: {:?}",
            fr.check.report.diagnostics()
        );
        assert!(fr.program.is_some());
    }

    // ── import diagnostic mapping (E0200 / E0217 / E0218) with spans ────────

    #[test]
    fn not_found_loader_emits_e0200_with_span() {
        let fr = compile_unit(
            "main.ty",
            "import missing\nfn main() -> Unit\n  ()\nend\n",
            &NotFoundLoader,
        );
        let diags = fr.check.report.diagnostics();
        let e0200 = diags.iter().find(|d| d.code.as_deref() == Some("E0200"));
        assert!(e0200.is_some(), "expected E0200, got: {diags:?}");
        assert!(
            !e0200.unwrap().labels.is_empty(),
            "E0200 must have a span label"
        );
    }

    #[test]
    fn ambiguous_loader_emits_e0217_with_span() {
        let fr = compile_unit(
            "main.ty",
            "import mock\nfn main() -> Unit\n  ()\nend\n",
            &AmbiguousLoader,
        );
        let diags = fr.check.report.diagnostics();
        let e0217 = diags.iter().find(|d| d.code.as_deref() == Some("E0217"));
        assert!(e0217.is_some(), "expected E0217, got: {diags:?}");
        assert!(
            !e0217.unwrap().labels.is_empty(),
            "E0217 must have a span label"
        );
    }

    #[test]
    fn dep_error_loader_emits_e0218_with_span() {
        let fr = compile_unit(
            "main.ty",
            "import mybin\nfn main() -> Unit\n  ()\nend\n",
            &DepErrorLoader,
        );
        let diags = fr.check.report.diagnostics();
        let e0218 = diags.iter().find(|d| d.code.as_deref() == Some("E0218"));
        assert!(e0218.is_some(), "expected E0218, got: {diags:?}");
        assert!(
            !e0218.unwrap().labels.is_empty(),
            "E0218 must have a span label"
        );
    }

    #[test]
    fn builtin_module_skipped_by_loader() {
        // core.sys is a built-in; the loader must not be called for it,
        // so no E0200 even with NotFoundLoader.
        let fr = compile_unit(
            "main.ty",
            "import core.sys\nfn main() -> Unit\n  ()\nend\n",
            &NotFoundLoader,
        );
        let codes: Vec<&str> = fr
            .check
            .report
            .diagnostics()
            .iter()
            .filter_map(|d| d.code.as_deref())
            .collect();
        assert!(
            !codes.contains(&"E0200"),
            "core.sys is builtin — loader must not be called, no E0200 expected, got: {codes:?}"
        );
    }

    // ── check_unit ───────────────────────────────────────────────────────────

    #[test]
    fn check_unit_returns_type_index_and_symbols() {
        let out = check_unit(
            "test.ty",
            "fn add(a: Int, b: Int) -> Int\n  a + b\nend\n",
            &NoImportLoader,
        );
        assert!(!out.report.has_errors(), "{:?}", out.report.diagnostics());
        // SymbolList = Vec<(String, CompletionKind)>; check `add` is present.
        assert!(
            out.symbols.iter().any(|(name, _)| name == "add"),
            "expected `add` in symbols"
        );
    }
}
