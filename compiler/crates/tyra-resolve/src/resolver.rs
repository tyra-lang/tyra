// Name resolver: walks the AST and resolves all name references.
// Two passes:
//   1. Collect: register all top-level declarations (forward reference support per §6.1)
//   2. Resolve: walk bodies and resolve references, report undefined names
//
// Note: Type annotations (e.g., `let x: MyType`) are NOT resolved here.
// Type name resolution is deferred to tyra-types, which has the full type
// context needed to resolve generics, abilities, and trait bounds.
//
// spec reference: §6.1 (top-level), §7.1 (bindings), §9 (functions), §13 (modules)

use tyra_ast::*;
use tyra_diagnostics::{Diagnostic, Label, Report, Span};

use crate::scope::{ScopeStack, Symbol};
use crate::{CompletionKind, DefIndex, SymbolList};

/// Resolve names in a source file. Reports errors for undefined names.
/// Returns a `DefIndex` (reference span → definition span) and a `SymbolList`
/// (all user-defined names + kind) for LSP completion.
pub fn resolve(file: &SourceFile, report: &mut Report) -> (DefIndex, SymbolList) {
    let symbol_list = collect_symbols(file);

    let mut scopes = ScopeStack::with_prelude();
    let mut def_index = DefIndex::new();

    // Pass 1: collect top-level declarations (§6.1 forward reference)
    collect_top_level(&file.items, &mut scopes, report);

    // Pass 1.5: validate ADR-0006 Rules 3-5 for top-level statements
    validate_top_level_restrictions(&file.items, report);

    // Pass 2: resolve references in all items
    for item in &file.items {
        resolve_item(item, &mut scopes, &mut def_index, report);
    }

    (def_index, symbol_list)
}

// ---------------------------------------------------------------------------
// Symbol collection (pre-pass for LSP completion)
// ---------------------------------------------------------------------------
// Walks the AST once to collect every user-defined name and its kind.
// This is a separate, simpler pass so the main resolve functions don't need
// to carry extra mutable state. Duplicate names (e.g. a shadowing `let x`)
// are deduplicated by `seen`.

fn record(
    name: &str,
    kind: CompletionKind,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    if seen.insert(name.to_string()) {
        out.push((name.to_string(), kind));
    }
}

fn collect_symbols(file: &SourceFile) -> SymbolList {
    let mut out = SymbolList::new();
    let mut seen = std::collections::HashSet::new();
    for item in &file.items {
        collect_sym_item(item, &mut out, &mut seen);
    }
    out
}

fn collect_sym_item(
    item: &Item,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    match item {
        Item::FnDef(f) => {
            record(&f.name, CompletionKind::Function, out, seen);
            for p in &f.params {
                record(&p.name, CompletionKind::Variable, out, seen);
            }
            collect_sym_stmts(&f.body, out, seen);
        }
        Item::ImplDef(imp) => {
            for m in &imp.methods {
                record(&m.name, CompletionKind::Function, out, seen);
                for p in &m.params {
                    record(&p.name, CompletionKind::Variable, out, seen);
                }
                collect_sym_stmts(&m.body, out, seen);
            }
        }
        Item::TraitDef(t) => {
            record(&t.name, CompletionKind::TypeDef, out, seen);
            for m in &t.methods {
                record(&m.name, CompletionKind::Function, out, seen);
            }
        }
        Item::ValueDef(v) => {
            record(&v.name, CompletionKind::TypeDef, out, seen);
        }
        Item::DataDef(d) => {
            record(&d.name, CompletionKind::TypeDef, out, seen);
        }
        Item::TypeDef(t) => {
            record(&t.name, CompletionKind::TypeDef, out, seen);
        }
        Item::Import(i) => {
            let local = i.alias.as_ref().unwrap_or_else(|| i.path.last().unwrap());
            record(local, CompletionKind::Module, out, seen);
        }
        Item::Stmt(s) => collect_sym_stmt(s, out, seen),
    }
}

fn collect_sym_stmts(
    stmts: &[Stmt],
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    for s in stmts {
        collect_sym_stmt(s, out, seen);
    }
}

fn collect_sym_stmt(
    stmt: &Stmt,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    match stmt {
        Stmt::Let(l) => {
            collect_sym_expr(&l.value, out, seen);
            record(&l.name, CompletionKind::Variable, out, seen);
        }
        Stmt::Mut(m) => {
            collect_sym_expr(&m.value, out, seen);
            record(&m.name, CompletionKind::Variable, out, seen);
        }
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                collect_sym_expr(v, out, seen);
            }
        }
        Stmt::Defer(d) => collect_sym_expr(&d.expr, out, seen),
        Stmt::Expr(e) => collect_sym_expr(&e.expr, out, seen),
        Stmt::Break(_) => {}
    }
}

fn collect_sym_expr(
    expr: &Expr,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    match &expr.kind {
        ExprKind::For(f) => {
            collect_sym_expr(&f.iter, out, seen);
            record(&f.binding, CompletionKind::Variable, out, seen);
            collect_sym_stmts(&f.body, out, seen);
        }
        ExprKind::Lambda(lam) => {
            for p in &lam.params {
                record(&p.name, CompletionKind::Variable, out, seen);
            }
            collect_sym_stmts(&lam.body, out, seen);
        }
        ExprKind::Match(m) => {
            collect_sym_expr(&m.subject, out, seen);
            for arm in &m.arms {
                collect_sym_pattern(&arm.pattern, out, seen);
                collect_sym_stmts(&arm.body, out, seen);
            }
        }
        ExprKind::If(i) => {
            collect_sym_expr(&i.condition, out, seen);
            collect_sym_stmts(&i.then_body, out, seen);
            if let Some(eb) = &i.else_body {
                collect_sym_else(eb, out, seen);
            }
        }
        ExprKind::While(w) => {
            collect_sym_expr(&w.condition, out, seen);
            collect_sym_stmts(&w.body, out, seen);
        }
        ExprKind::BinaryOp(l, _, r) => {
            collect_sym_expr(l, out, seen);
            collect_sym_expr(r, out, seen);
        }
        ExprKind::UnaryOp(_, e) => collect_sym_expr(e, out, seen),
        ExprKind::Assign(l, r) => {
            collect_sym_expr(l, out, seen);
            collect_sym_expr(r, out, seen);
        }
        ExprKind::Call(callee, args) => {
            collect_sym_expr(callee, out, seen);
            for arg in args {
                collect_sym_expr(&arg.value, out, seen);
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            collect_sym_expr(callee, out, seen);
            for arg in args {
                collect_sym_expr(&arg.value, out, seen);
            }
        }
        ExprKind::FieldAccess(e, _) => collect_sym_expr(e, out, seen),
        ExprKind::Index(e, i) => {
            collect_sym_expr(e, out, seen);
            collect_sym_expr(i, out, seen);
        }
        ExprKind::Propagate(e)
        | ExprKind::Await(e)
        | ExprKind::Spawn(e) => collect_sym_expr(e, out, seen),
        ExprKind::ListLit(items) => {
            for item in items {
                collect_sym_expr(item, out, seen);
            }
        }
        ExprKind::MapLit(entries) => {
            for (k, v) in entries {
                collect_sym_expr(k, out, seen);
                collect_sym_expr(v, out, seen);
            }
        }
        ExprKind::StringInterp(parts) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    collect_sym_expr(e, out, seen);
                }
            }
        }
        ExprKind::Ident(_)
        | ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => {}
    }
}

fn collect_sym_pattern(
    pat: &Pattern,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    match &pat.kind {
        PatternKind::Ident(name) => {
            record(name, CompletionKind::Variable, out, seen);
        }
        PatternKind::Constructor(_, fields) => {
            for f in fields {
                collect_sym_pattern(&f.pattern, out, seen);
            }
        }
        PatternKind::Wildcard
        | PatternKind::IntLit(_)
        | PatternKind::FloatLit(_)
        | PatternKind::StringLit(_)
        | PatternKind::BoolLit(_) => {}
    }
}

fn collect_sym_else(
    eb: &ElseBranch,
    out: &mut SymbolList,
    seen: &mut std::collections::HashSet<String>,
) {
    match eb {
        ElseBranch::Else(stmts) => collect_sym_stmts(stmts, out, seen),
        ElseBranch::ElseIf(i) => {
            collect_sym_expr(&i.condition, out, seen);
            collect_sym_stmts(&i.then_body, out, seen);
            if let Some(inner) = &i.else_body {
                collect_sym_else(inner, out, seen);
            }
        }
    }
}

/// Extract the definition span from a symbol, if it has one.
/// `Prelude` symbols have no source span and are excluded.
fn symbol_span(sym: &Symbol) -> Option<Span> {
    match sym {
        Symbol::Local { span, .. } => Some(*span),
        Symbol::Param { span } => Some(*span),
        Symbol::Function { span } => Some(*span),
        Symbol::TypeDef { span } => Some(*span),
        Symbol::TraitDef { span } => Some(*span),
        Symbol::Import { span, .. } => Some(*span),
        Symbol::Prelude { .. } => None,
    }
}

/// ADR-0006 Rules 3-5: top-level statements may not contain ?, .await, or return.
fn validate_top_level_restrictions(items: &[Item], report: &mut Report) {
    for item in items {
        if let Item::Stmt(stmt) = item {
            check_stmt_restrictions(stmt, report);
        }
    }
}

fn check_stmt_restrictions(stmt: &Stmt, report: &mut Report) {
    match stmt {
        Stmt::Return(s) => {
            report.add(
                Diagnostic::error("`return` is not allowed in top-level statements")
                    .with_code("E0210")
                    .with_label(Label::new(s.span, "use explicit `fn main` for early return"))
                    .with_note("top-level statements are desugared to `fn main() -> Unit` (ADR-0006 Rule 5)"),
            );
        }
        Stmt::Let(s) => check_expr_restrictions(&s.value, report),
        Stmt::Mut(s) => check_expr_restrictions(&s.value, report),
        Stmt::Defer(s) => check_expr_restrictions(&s.expr, report),
        Stmt::Break(_) => {}
        Stmt::Expr(s) => check_expr_restrictions(&s.expr, report),
    }
}

fn check_expr_restrictions(expr: &Expr, report: &mut Report) {
    match &expr.kind {
        ExprKind::Propagate(_) => {
            report.add(
                Diagnostic::error("`?` is not allowed in top-level statements")
                    .with_code("E0211")
                    .with_label(Label::new(expr.span, "use explicit `fn main() -> Result<Unit, E>` for error propagation"))
                    .with_note("top-level statements are desugared to `fn main() -> Unit` (ADR-0006 Rule 3)"),
            );
        }
        ExprKind::Await(_) => {
            report.add(
                Diagnostic::error("`.await` is not allowed in top-level statements")
                    .with_code("E0212")
                    .with_label(Label::new(expr.span, "use explicit `async fn main()` for async operations"))
                    .with_note("top-level statements are desugared to `fn main() -> Unit` (ADR-0006 Rule 4)"),
            );
        }
        // Recurse into subexpressions
        ExprKind::BinaryOp(l, _, r) => {
            check_expr_restrictions(l, report);
            check_expr_restrictions(r, report);
        }
        ExprKind::UnaryOp(_, e) => check_expr_restrictions(e, report),
        ExprKind::Call(callee, args) => {
            check_expr_restrictions(callee, report);
            for arg in args {
                check_expr_restrictions(&arg.value, report);
            }
        }
        ExprKind::Assign(l, r) => {
            check_expr_restrictions(l, report);
            check_expr_restrictions(r, report);
        }
        ExprKind::FieldAccess(e, _) => check_expr_restrictions(e, report),
        ExprKind::Index(e, i) => {
            check_expr_restrictions(e, report);
            check_expr_restrictions(i, report);
        }
        ExprKind::If(if_expr) => {
            check_expr_restrictions(&if_expr.condition, report);
            // Bodies can contain return/? as they're inside the implicit main function
            // — but ? and .await in expression position within bodies are still forbidden
            // because the implicit main returns Unit.
            // For simplicity, we only check the immediate top-level expression, not nested bodies.
        }
        _ => {} // Literals, identifiers, etc. are fine
    }
}

/// Pass 1: register all top-level declarations for forward reference.
fn collect_top_level(items: &[Item], scopes: &mut ScopeStack, report: &mut Report) {
    for item in items {
        match item {
            Item::FnDef(f) => {
                define_or_report(
                    scopes,
                    &f.name,
                    Symbol::Function { span: f.span },
                    f.span,
                    report,
                );
            }
            Item::ValueDef(v) => {
                define_or_report(
                    scopes,
                    &v.name,
                    Symbol::TypeDef { span: v.span },
                    v.span,
                    report,
                );
            }
            Item::DataDef(d) => {
                define_or_report(
                    scopes,
                    &d.name,
                    Symbol::TypeDef { span: d.span },
                    d.span,
                    report,
                );
            }
            Item::TypeDef(t) => {
                define_or_report(
                    scopes,
                    &t.name,
                    Symbol::TypeDef { span: t.span },
                    t.span,
                    report,
                );
                // ADT variant constructors use qualified form (TypeName.Variant)
                // per §8.5. Only prelude variants (Some/None/Ok/Err) are
                // unqualified, and those are registered via PRELUDE_CONSTRUCTORS.
            }
            Item::TraitDef(t) => {
                define_or_report(
                    scopes,
                    &t.name,
                    Symbol::TraitDef { span: t.span },
                    t.span,
                    report,
                );
            }
            Item::Import(i) => {
                let local_name = i.alias.as_ref().unwrap_or_else(|| i.path.last().unwrap());
                define_or_report(
                    scopes,
                    local_name,
                    Symbol::Import {
                        path: i.path.clone(),
                        span: i.span,
                    },
                    i.span,
                    report,
                );
            }
            Item::ImplDef(_) | Item::Stmt(_) => {
                // impl and statements don't introduce top-level names
            }
        }
    }
}

/// Pass 2: resolve references in each item.
fn resolve_item(item: &Item, scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    match item {
        Item::FnDef(f) => resolve_fn(f, scopes, def_index, report),
        Item::ImplDef(imp) => {
            for method in &imp.methods {
                resolve_fn(method, scopes, def_index, report);
            }
        }
        Item::TraitDef(t) => {
            for method in &t.methods {
                resolve_fn(method, scopes, def_index, report);
            }
        }
        Item::Stmt(s) => resolve_stmt(s, scopes, def_index, report),
        // Type definitions are fully handled in pass 1
        Item::ValueDef(_) | Item::DataDef(_) | Item::TypeDef(_) | Item::Import(_) => {}
    }
}

fn resolve_fn(f: &FnDef, scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    scopes.push();
    // Bind `self` if present (§8.7 trait methods)
    if let Some(self_param) = &f.self_param {
        scopes.define(
            "self".to_string(),
            Symbol::Param {
                span: self_param.span,
            },
        );
    }
    for param in &f.params {
        scopes.define(param.name.clone(), Symbol::Param { span: param.span });
    }
    resolve_body(&f.body, scopes, def_index, report);
    scopes.pop();
}

fn resolve_body(stmts: &[Stmt], scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    for stmt in stmts {
        resolve_stmt(stmt, scopes, def_index, report);
    }
}

fn resolve_stmt(stmt: &Stmt, scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    match stmt {
        Stmt::Let(s) => {
            resolve_expr(&s.value, scopes, def_index, report);
            scopes.define(
                s.name.clone(),
                Symbol::Local {
                    mutable: false,
                    span: s.span,
                },
            );
        }
        Stmt::Mut(s) => {
            resolve_expr(&s.value, scopes, def_index, report);
            scopes.define(
                s.name.clone(),
                Symbol::Local {
                    mutable: true,
                    span: s.span,
                },
            );
        }
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                resolve_expr(v, scopes, def_index, report);
            }
        }
        Stmt::Defer(s) => {
            resolve_expr(&s.expr, scopes, def_index, report);
        }
        Stmt::Break(_) => {}
        Stmt::Expr(s) => {
            resolve_expr(&s.expr, scopes, def_index, report);
        }
    }
}

fn resolve_expr(expr: &Expr, scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    match &expr.kind {
        // Names that need resolution
        ExprKind::Ident(name) => {
            match scopes.lookup(name) {
                None => {
                    report.add(
                        Diagnostic::error(format!("undefined name `{name}`"))
                            .with_code("E0200")
                            .with_label(Label::new(expr.span, "not found in this scope")),
                    );
                }
                Some(sym) => {
                    if let Some(def_span) = symbol_span(sym) {
                        def_index.insert(expr.span, def_span);
                    }
                }
            }
        }

        // Recursive cases
        ExprKind::BinaryOp(l, _, r) => {
            resolve_expr(l, scopes, def_index, report);
            resolve_expr(r, scopes, def_index, report);
        }
        ExprKind::UnaryOp(_, e) => resolve_expr(e, scopes, def_index, report),
        ExprKind::Assign(l, r) => {
            resolve_expr(l, scopes, def_index, report);
            resolve_expr(r, scopes, def_index, report);
        }
        ExprKind::Call(callee, args) => {
            resolve_expr(callee, scopes, def_index, report);
            for arg in args {
                resolve_expr(&arg.value, scopes, def_index, report);
            }
        }
        ExprKind::TurbofishCall(callee, _, args) => {
            resolve_expr(callee, scopes, def_index, report);
            for arg in args {
                resolve_expr(&arg.value, scopes, def_index, report);
            }
        }
        ExprKind::FieldAccess(obj, _) => resolve_expr(obj, scopes, def_index, report),
        ExprKind::Index(obj, idx) => {
            resolve_expr(obj, scopes, def_index, report);
            resolve_expr(idx, scopes, def_index, report);
        }
        ExprKind::Propagate(e) => resolve_expr(e, scopes, def_index, report),
        ExprKind::Await(e) => resolve_expr(e, scopes, def_index, report),
        ExprKind::Spawn(e) => resolve_expr(e, scopes, def_index, report),

        // Control flow with new scopes
        ExprKind::If(if_expr) => resolve_if(if_expr, scopes, def_index, report),
        ExprKind::Match(m) => {
            resolve_expr(&m.subject, scopes, def_index, report);
            for arm in &m.arms {
                scopes.push();
                bind_pattern(&arm.pattern, scopes);
                resolve_body(&arm.body, scopes, def_index, report);
                scopes.pop();
            }
        }
        ExprKind::For(f) => {
            resolve_expr(&f.iter, scopes, def_index, report);
            scopes.push();
            scopes.define(
                f.binding.clone(),
                Symbol::Local {
                    mutable: false,
                    span: f.span,
                },
            );
            resolve_body(&f.body, scopes, def_index, report);
            scopes.pop();
        }
        ExprKind::While(w) => {
            resolve_expr(&w.condition, scopes, def_index, report);
            scopes.push();
            resolve_body(&w.body, scopes, def_index, report);
            scopes.pop();
        }
        ExprKind::Lambda(lam) => {
            scopes.push();
            for param in &lam.params {
                scopes.define(param.name.clone(), Symbol::Param { span: param.span });
            }
            resolve_body(&lam.body, scopes, def_index, report);
            scopes.pop();
        }

        // Literals and collections
        ExprKind::ListLit(items) => {
            for item in items {
                resolve_expr(item, scopes, def_index, report);
            }
        }
        ExprKind::MapLit(entries) => {
            for (k, v) in entries {
                resolve_expr(k, scopes, def_index, report);
                resolve_expr(v, scopes, def_index, report);
            }
        }
        ExprKind::StringInterp(parts) => {
            for part in parts {
                if let StringPart::Expr(e) = part {
                    resolve_expr(e, scopes, def_index, report);
                }
            }
        }

        // Leaves — no names to resolve
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit => {}
    }
}

fn resolve_if(if_expr: &IfExpr, scopes: &mut ScopeStack, def_index: &mut DefIndex, report: &mut Report) {
    resolve_expr(&if_expr.condition, scopes, def_index, report);
    scopes.push();
    resolve_body(&if_expr.then_body, scopes, def_index, report);
    scopes.pop();
    match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => {
            scopes.push();
            resolve_body(body, scopes, def_index, report);
            scopes.pop();
        }
        Some(ElseBranch::ElseIf(inner)) => {
            resolve_if(inner, scopes, def_index, report);
        }
        None => {}
    }
}

/// Bind names introduced by a pattern (for match arms).
fn bind_pattern(pat: &Pattern, scopes: &mut ScopeStack) {
    match &pat.kind {
        PatternKind::Ident(name) => {
            scopes.define(
                name.clone(),
                Symbol::Local {
                    mutable: false,
                    span: pat.span,
                },
            );
        }
        PatternKind::Constructor(_, fields) => {
            for field in fields {
                bind_pattern(&field.pattern, scopes);
            }
        }
        PatternKind::Wildcard
        | PatternKind::IntLit(_)
        | PatternKind::FloatLit(_)
        | PatternKind::StringLit(_)
        | PatternKind::BoolLit(_) => {}
    }
}

/// Define a name or report duplicate definition error.
fn define_or_report(
    scopes: &mut ScopeStack,
    name: &str,
    symbol: Symbol,
    span: Span,
    report: &mut Report,
) {
    if scopes.defined_in_current(name) {
        // Allow shadowing prelude names (e.g., defining your own `print`)
        if let Some(Symbol::Prelude { .. }) = scopes.lookup(name) {
            scopes.define(name.to_string(), symbol);
            return;
        }
        report.add(
            Diagnostic::error(format!("duplicate definition of `{name}`"))
                .with_code("E0201")
                .with_label(Label::new(span, "already defined in this scope")),
        );
    } else {
        scopes.define(name.to_string(), symbol);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::{Report, SourceMap};

    fn resolve_str(source: &str) -> Report {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let ast = tyra_parser::parse(id, &sources, &mut report);
        if report.has_errors() {
            return report; // parse errors
        }
        let (_def_index, _symbols) = resolve(&ast, &mut report);
        report
    }

    fn resolve_with_symbols(source: &str) -> (Report, crate::SymbolList) {
        let mut sources = tyra_diagnostics::SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let ast = tyra_parser::parse(id, &sources, &mut report);
        if report.has_errors() {
            return (report, vec![]);
        }
        let (_def_index, symbols) = resolve(&ast, &mut report);
        (report, symbols)
    }

    #[test]
    fn symbol_list_includes_fn_let_var() {
        let src = "let x: Int = 1\nfn foo()\n  let y = x\nend\n";
        let (report, symbols) = resolve_with_symbols(src);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        let names: Vec<&str> = symbols.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"x"), "missing x in {names:?}");
        assert!(names.contains(&"foo"), "missing foo in {names:?}");
        assert!(names.contains(&"y"), "missing y in {names:?}");
    }

    #[test]
    fn symbol_list_kinds() {
        use crate::CompletionKind;
        let src = "import list\nfn greet()\nend\nlet v = 1\n";
        let (_, symbols) = resolve_with_symbols(src);
        let find = |name: &str| symbols.iter().find(|(n, _)| n == name).map(|(_, k)| k.clone());
        assert_eq!(find("greet"), Some(CompletionKind::Function));
        assert_eq!(find("v"), Some(CompletionKind::Variable));
        assert_eq!(find("list"), Some(CompletionKind::Module));
    }

    #[test]
    fn hello_world_resolves() {
        let report = resolve_str("print(\"hello\")\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn undefined_name_error() {
        let report = resolve_str("foo()\n");
        assert!(report.has_errors());
        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn let_binding_visible() {
        let report = resolve_str("let x = 42\nprint(x)\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn fn_def_forward_reference() {
        // §6.1: top-level declarations are forward-referenceable
        let report = resolve_str("greet()\nfn greet()\n  print(\"hi\")\nend\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn fn_params_in_scope() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn fn_params_not_leaked() {
        let source = "fn f(_ x: Int) -> Int\n  x\nend\nprint(x)\n";
        let report = resolve_str(source);
        assert!(report.has_errors()); // x not visible outside f
    }

    #[test]
    fn match_pattern_bindings() {
        let source = "match 1\nwhen x\n  print(x)\nend\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn for_binding_scoped() {
        let source = "let items = [1, 2]\nfor item in items\n  print(item)\nend\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn for_binding_not_leaked() {
        let source = "for item in [1]\n  print(item)\nend\nprint(item)\n";
        let report = resolve_str(source);
        assert!(report.has_errors()); // item not visible outside for
    }

    #[test]
    fn type_def_visible() {
        let source = "value Point\n  x: Int\nend\nlet p = Point(x: 1)\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn prelude_constructors_available() {
        let source = "let x = Some(42)\nlet y = None\nlet z = Ok(1)\nlet w = Err(0)\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn duplicate_definition_error() {
        let source = "fn f()\nend\nfn f()\nend\n";
        let report = resolve_str(source);
        assert!(report.has_errors());
    }

    #[test]
    fn shadow_prelude_name() {
        // User can shadow prelude names (e.g., define own `print`)
        let source = "fn print(_ msg: Int) -> Unit\nend\nprint(42)\n";
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn top_level_propagate_forbidden() {
        // ADR-0006 Rule 3: ? not allowed at top level
        let report = resolve_str("let x = Some(1)\nlet y = x?\n");
        assert!(report.has_errors());
    }

    #[test]
    fn top_level_return_forbidden() {
        // ADR-0006 Rule 5: return not allowed at top level
        let report = resolve_str("return\n");
        assert!(report.has_errors());
    }

    #[test]
    fn adt_variants_not_top_level() {
        // ADT variants use qualified form (§8.5), so bare `Red` should be undefined
        let source = "type Color =\n  | Red\n  | Blue\nlet c = Red\n";
        let report = resolve_str(source);
        assert!(report.has_errors()); // Red is not a top-level name
    }

    #[test]
    fn full_program_resolves() {
        let source = r#"fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end

let result = fib(10)
print(result)
"#;
        let report = resolve_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }
}
