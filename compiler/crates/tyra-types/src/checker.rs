// Type checker: walks the AST and verifies type correctness.
//
// Current scope (Milestone 1a):
// - Literal type inference (Int, Float, String, Bool, Unit)
// - Arithmetic, comparison, and logical operator type checking
// - Function call argument type checking (count only for now; full type
//   checking of prelude signatures requires stdlib type info)
// - let/mut binding type annotation verification
// - Assignment mutability checking
//
// Deferred to later milestones:
// - Generics and type parameter inference
// - Ability auto-derivation (Eq, Hash, Ord, Debug)
// - Trait resolution
// - ? operator type verification (Result/Option return type checking)
// - Into trait handling
//
// spec reference: §8 (type system), §10.1 (operators), §12.2 (?)

use std::collections::{HashMap, HashSet};

use tyra_ast::*;
use tyra_diagnostics::{Diagnostic, Label, Report, Span};

use crate::ty::{Ty, types_compatible};

/// Type environment: maps names to their types.
#[derive(Debug)]
pub struct TypeEnv {
    bindings: Vec<HashMap<String, Ty>>,
    /// ADT variant names keyed by type name (§10.3 exhaustiveness).
    /// - User-defined: `type Color = | Red | Green | Blue` → "Color" → ["Red", "Green", "Blue"]
    /// - Prelude: "Option" → ["Some", "None"], "Result" → ["Ok", "Err"]
    adt_variants: HashMap<String, Vec<String>>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            bindings: vec![HashMap::new()],
            adt_variants: HashMap::new(),
        }
    }

    pub fn push(&mut self) {
        self.bindings.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.bindings.pop();
    }

    pub fn define(&mut self, name: String, ty: Ty) {
        self.bindings.last_mut().unwrap().insert(name, ty);
    }

    pub fn lookup(&self, name: &str) -> Option<&Ty> {
        for scope in self.bindings.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    /// Register an ADT with its variant names for exhaustiveness checking.
    pub fn register_adt(&mut self, type_name: String, variants: Vec<String>) {
        self.adt_variants.insert(type_name, variants);
    }

    /// Get variant names for an ADT type, if registered.
    pub fn adt_variants(&self, type_name: &str) -> Option<&Vec<String>> {
        self.adt_variants.get(type_name)
    }
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-check a source file.
pub fn check(file: &SourceFile, report: &mut Report) {
    let mut env = TypeEnv::new();
    register_prelude(&mut env);
    collect_top_level_types(&file.items, &mut env);

    for item in &file.items {
        check_item(item, &mut env, report);
    }
}

/// Register prelude function types.
fn register_prelude(env: &mut TypeEnv) {
    // print/println accept any Debug type: fn<T: Debug>(T) -> Unit
    // Since generics are not yet implemented, we use Ty::Error as the parameter
    // type to accept any argument without type mismatch errors.
    // TODO: Replace with proper generic constraint checking when generics are implemented.
    for name in &["print", "println", "eprint", "eprintln"] {
        env.define(
            name.to_string(),
            Ty::Fn(vec![Ty::Error], Box::new(Ty::Unit)),
        );
    }
    env.define(
        "panic".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Never)),
    );
    // parse::<T>(str) -> Option<T>: generic, use Error as escape hatch
    env.define(
        "parse".to_string(),
        Ty::Fn(vec![Ty::Error], Box::new(Ty::Error)),
    );

    // Prelude ADTs for §10.3 exhaustiveness checking.
    env.register_adt("Option".into(), vec!["Some".into(), "None".into()]);
    env.register_adt("Result".into(), vec!["Ok".into(), "Err".into()]);
}

/// Collect top-level function signatures and ADT definitions for forward reference.
fn collect_top_level_types(items: &[Item], env: &mut TypeEnv) {
    for item in items {
        match item {
            Item::FnDef(f) => {
                let param_tys: Vec<Ty> = f
                    .params
                    .iter()
                    .map(|p| Ty::from_type_expr(&p.type_annotation))
                    .collect();
                let ret_ty = f
                    .return_type
                    .as_ref()
                    .map(Ty::from_type_expr)
                    .unwrap_or(Ty::Unit);
                env.define(f.name.clone(), Ty::Fn(param_tys, Box::new(ret_ty)));
            }
            Item::TypeDef(t) => {
                if let TypeDefKind::Adt(variants) = &t.kind {
                    let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                    env.register_adt(t.name.clone(), names);
                }
            }
            _ => {}
        }
    }
}

fn check_item(item: &Item, env: &mut TypeEnv, report: &mut Report) {
    match item {
        Item::FnDef(f) => check_fn(f, env, report),
        Item::Stmt(s) => {
            check_stmt(s, env, report);
        }
        // Type definitions, traits, impls — type checking deferred to later milestones
        _ => {}
    }
}

fn check_fn(f: &FnDef, env: &mut TypeEnv, report: &mut Report) {
    env.push();
    for param in &f.params {
        let ty = Ty::from_type_expr(&param.type_annotation);
        env.define(param.name.clone(), ty);
    }
    if f.self_param.is_some() {
        // self type is not known without the enclosing impl context
        // For now, use Error to avoid cascading type errors
        env.define("self".to_string(), Ty::Error);
    }

    // Walk body and track the last expression statement's type
    let mut last_expr_ty: Option<Ty> = None;
    let mut last_expr_span = None;
    for (i, stmt) in f.body.iter().enumerate() {
        check_stmt(stmt, env, report);
        // Cache the last expression statement's type (avoids double inference)
        if i + 1 == f.body.len() {
            if let Stmt::Expr(expr_stmt) = stmt {
                last_expr_ty = Some(infer_expr(&expr_stmt.expr, env, report));
                last_expr_span = Some(expr_stmt.expr.span);
            }
        }
    }

    // Return type verification: check that the last expression's type matches
    // the declared return type (if any).
    // NOTE: explicit `return` statements are not checked yet (future improvement).
    let declared_ret = f
        .return_type
        .as_ref()
        .map(Ty::from_type_expr)
        .unwrap_or(Ty::Unit);

    if declared_ret != Ty::Unit {
        if let (Some(actual_ty), Some(span)) = (last_expr_ty, last_expr_span) {
            // Skip if either side is Error (cascading), Never (bottom type),
            // Unit (if/else not yet type-unified), Named/Generic (not fully resolved)
            let skip = actual_ty.is_error()
                || declared_ret.is_error()
                || matches!(
                    actual_ty,
                    Ty::Never | Ty::Unit | Ty::Named(_) | Ty::Generic(_, _)
                )
                || matches!(declared_ret, Ty::Named(_) | Ty::Generic(_, _));
            if !skip && actual_ty != declared_ret {
                report.add(
                    Diagnostic::error(format!(
                        "return type mismatch: expected {}, found {}",
                        declared_ret.display_name(),
                        actual_ty.display_name()
                    ))
                    .with_code("E0309")
                    .with_label(Label::new(span, "this expression has the wrong type")),
                );
            }
        }
    }

    env.pop();
}

fn check_stmt(stmt: &Stmt, env: &mut TypeEnv, report: &mut Report) {
    match stmt {
        Stmt::Let(s) => {
            let value_ty = infer_expr(&s.value, env, report);
            if let Some(annotation) = &s.type_annotation {
                let expected = Ty::from_type_expr(annotation);
                check_type_match(&expected, &value_ty, s.span, report);
            }
            env.define(s.name.clone(), value_ty);
        }
        Stmt::Mut(s) => {
            let value_ty = infer_expr(&s.value, env, report);
            if let Some(annotation) = &s.type_annotation {
                let expected = Ty::from_type_expr(annotation);
                check_type_match(&expected, &value_ty, s.span, report);
            }
            env.define(s.name.clone(), value_ty);
        }
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                infer_expr(v, env, report);
            }
        }
        Stmt::Defer(s) => {
            infer_expr(&s.expr, env, report);
        }
        Stmt::Expr(s) => {
            infer_expr(&s.expr, env, report);
        }
    }
}

/// Infer the type of an expression.
pub fn infer_expr(expr: &Expr, env: &mut TypeEnv, report: &mut Report) -> Ty {
    match &expr.kind {
        // Literals
        ExprKind::IntLit(_) => Ty::Int,
        ExprKind::FloatLit(_) => Ty::Float,
        ExprKind::StringLit(_) => Ty::String,
        ExprKind::StringInterp(_) => Ty::String,
        ExprKind::BoolLit(_) => Ty::Bool,
        ExprKind::UnitLit => Ty::Unit,
        ExprKind::ListLit(items) => {
            if items.is_empty() {
                Ty::Generic("List".into(), vec![Ty::Var(0)])
            } else {
                let elem_ty = infer_expr(&items[0], env, report);
                Ty::Generic("List".into(), vec![elem_ty])
            }
        }
        ExprKind::MapLit(entries) => {
            if entries.is_empty() {
                Ty::Generic("Map".into(), vec![Ty::Var(0), Ty::Var(1)])
            } else {
                let key_ty = infer_expr(&entries[0].0, env, report);
                let val_ty = infer_expr(&entries[0].1, env, report);
                Ty::Generic("Map".into(), vec![key_ty, val_ty])
            }
        }

        // Identifier lookup
        ExprKind::Ident(name) => env.lookup(name).cloned().unwrap_or(Ty::Error),

        // Field access — deferred (needs type info about the target)
        ExprKind::FieldAccess(obj, _) => {
            infer_expr(obj, env, report);
            Ty::Error // field resolution requires knowing the object's type definition
        }

        // Binary operations (§10.1)
        ExprKind::BinaryOp(left, op, right) => {
            let left_ty = infer_expr(left, env, report);
            let right_ty = infer_expr(right, env, report);
            infer_binop(*op, &left_ty, &right_ty, expr.span, report)
        }

        // Unary operations
        ExprKind::UnaryOp(op, operand) => {
            let ty = infer_expr(operand, env, report);
            match op {
                UnaryOp::Neg => {
                    if !matches!(ty, Ty::Int | Ty::Float | Ty::Error) {
                        report.add(
                            Diagnostic::error(format!(
                                "unary `-` requires Int or Float, found {}",
                                ty.display_name()
                            ))
                            .with_code("E0300")
                            .with_label(Label::new(expr.span, "cannot negate this type")),
                        );
                    }
                    ty
                }
                UnaryOp::Not => {
                    if !matches!(ty, Ty::Bool | Ty::Error) {
                        report.add(
                            Diagnostic::error(format!(
                                "`not` requires Bool, found {}",
                                ty.display_name()
                            ))
                            .with_code("E0300")
                            .with_label(Label::new(expr.span, "expected Bool")),
                        );
                    }
                    Ty::Bool
                }
            }
        }

        // Assignment
        ExprKind::Assign(lhs, rhs) => {
            infer_expr(lhs, env, report);
            infer_expr(rhs, env, report);
            Ty::Unit
        }

        // Function call
        ExprKind::Call(callee, args) => {
            let callee_ty = infer_expr(callee, env, report);
            match callee_ty {
                Ty::Fn(param_tys, ret_ty) => {
                    if args.len() != param_tys.len() {
                        report.add(
                            Diagnostic::error(format!(
                                "expected {} argument{}, found {}",
                                param_tys.len(),
                                if param_tys.len() == 1 { "" } else { "s" },
                                args.len()
                            ))
                            .with_code("E0301")
                            .with_label(Label::new(expr.span, "wrong number of arguments")),
                        );
                        // Still infer arg types to find errors in arguments
                        for arg in args {
                            infer_expr(&arg.value, env, report);
                        }
                    } else {
                        // Check each argument type against parameter type
                        for (arg, param_ty) in args.iter().zip(param_tys.iter()) {
                            let arg_ty = infer_expr(&arg.value, env, report);
                            check_type_match(param_ty, &arg_ty, arg.span, report);
                        }
                    }
                    *ret_ty
                }
                Ty::Error => {
                    for arg in args {
                        infer_expr(&arg.value, env, report);
                    }
                    Ty::Error
                }
                _ => {
                    // Could be a constructor call (e.g., Point(x: 1.0, y: 2.0))
                    // For now, accept and return Error
                    for arg in args {
                        infer_expr(&arg.value, env, report);
                    }
                    Ty::Error
                }
            }
        }

        ExprKind::TurbofishCall(callee, _, args) => {
            infer_expr(callee, env, report);
            for arg in args {
                infer_expr(&arg.value, env, report);
            }
            Ty::Error // turbofish resolution deferred
        }

        // Index
        ExprKind::Index(obj, idx) => {
            infer_expr(obj, env, report);
            infer_expr(idx, env, report);
            Ty::Error // element type resolution deferred
        }

        // Propagation (?)
        ExprKind::Propagate(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            match inner_ty {
                Ty::Generic(ref name, ref args) if name == "Option" && args.len() == 1 => {
                    args[0].clone()
                }
                Ty::Generic(ref name, ref args) if name == "Result" && args.len() == 2 => {
                    args[0].clone()
                }
                Ty::Error => Ty::Error,
                _ => {
                    report.add(
                        Diagnostic::error(format!(
                            "`?` requires Option or Result, found {}",
                            inner_ty.display_name()
                        ))
                        .with_code("E0302")
                        .with_label(Label::new(expr.span, "cannot use `?` on this type")),
                    );
                    Ty::Error
                }
            }
        }

        // Await (§14.3): v0.1 — accept any type (async is synchronous no-op)
        ExprKind::Await(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            match inner_ty {
                Ty::Generic(ref name, ref args) if name == "Task" && args.len() == 1 => {
                    args[0].clone()
                }
                // v0.1: async fn returns T directly (not Task<T>); .await is identity
                other => other,
            }
        }

        // Control flow
        ExprKind::If(if_expr) => check_if(if_expr, env, report),
        ExprKind::Match(m) => {
            let subject_ty = infer_expr(&m.subject, env, report);
            check_match_exhaustiveness(&subject_ty, m, env, report);
            let mut arm_ty = Ty::Unit;
            for arm in &m.arms {
                env.push();
                bind_pattern_types(&arm.pattern, env);
                for stmt in &arm.body {
                    check_stmt(stmt, env, report);
                }
                if let Some(last) = arm.body.last() {
                    arm_ty = stmt_type(last, env, report);
                }
                env.pop();
            }
            arm_ty
        }
        ExprKind::For(f) => {
            infer_expr(&f.iter, env, report);
            env.push();
            env.define(f.binding.clone(), Ty::Error); // element type unknown without generics
            for stmt in &f.body {
                check_stmt(stmt, env, report);
            }
            env.pop();
            Ty::Unit
        }
        ExprKind::While(w) => {
            let cond_ty = infer_expr(&w.condition, env, report);
            if !matches!(cond_ty, Ty::Bool | Ty::Error) {
                report.add(
                    Diagnostic::error(format!(
                        "while condition must be Bool, found {}",
                        cond_ty.display_name()
                    ))
                    .with_code("E0304")
                    .with_label(Label::new(expr.span, "expected Bool")),
                );
            }
            env.push();
            for stmt in &w.body {
                check_stmt(stmt, env, report);
            }
            env.pop();
            Ty::Unit
        }

        ExprKind::Lambda(lam) => {
            let param_tys: Vec<Ty> = lam
                .params
                .iter()
                .map(|p| Ty::from_type_expr(&p.type_annotation))
                .collect();
            let ret_ty = lam
                .return_type
                .as_ref()
                .map(Ty::from_type_expr)
                .unwrap_or(Ty::Unit);
            env.push();
            for (param, ty) in lam.params.iter().zip(&param_tys) {
                env.define(param.name.clone(), ty.clone());
            }
            for stmt in &lam.body {
                check_stmt(stmt, env, report);
            }
            env.pop();
            Ty::Fn(param_tys, Box::new(ret_ty))
        }

        ExprKind::Spawn(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            Ty::Generic("Task".into(), vec![inner_ty])
        }
    }
}

fn check_if(if_expr: &IfExpr, env: &mut TypeEnv, report: &mut Report) -> Ty {
    let cond_ty = infer_expr(&if_expr.condition, env, report);
    if !matches!(cond_ty, Ty::Bool | Ty::Error) {
        report.add(
            Diagnostic::error(format!(
                "if condition must be Bool, found {}",
                cond_ty.display_name()
            ))
            .with_code("E0304")
            .with_label(Label::new(if_expr.span, "expected Bool")),
        );
    }

    env.push();
    for stmt in &if_expr.then_body {
        check_stmt(stmt, env, report);
    }
    env.pop();

    match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => {
            env.push();
            for stmt in body {
                check_stmt(stmt, env, report);
            }
            env.pop();
        }
        Some(ElseBranch::ElseIf(inner)) => {
            check_if(inner, env, report);
        }
        None => {}
    }
    Ty::Unit // if/else type unification deferred
}

/// Infer binary operator result type.
fn infer_binop(op: BinOp, left: &Ty, right: &Ty, span: Span, report: &mut Report) -> Ty {
    if left.is_error() || right.is_error() {
        return Ty::Error;
    }

    match op {
        // Arithmetic: Int/Float operands -> same type
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            if left == right && matches!(left, Ty::Int | Ty::Float) {
                left.clone()
            } else {
                report.add(
                    Diagnostic::error(format!(
                        "arithmetic operator requires matching Int or Float operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                Ty::Error
            }
        }
        // Equality: same type with Eq ability -> Bool
        BinOp::Eq | BinOp::NotEq => {
            // §7.2: Float does NOT have Eq (ADR-0002)
            if matches!(left, Ty::Float) || matches!(right, Ty::Float) {
                report.add(
                    Diagnostic::error("Float does not have Eq; use float module for comparison")
                        .with_code("E0306")
                        .with_label(Label::new(span, "Float has no Eq (ADR-0002)"))
                        .with_note("use float.eq() or float.approx_eq() instead"),
                );
                return Ty::Error;
            }
            // Operands must be the same type
            if left != right {
                report.add(
                    Diagnostic::error(format!(
                        "cannot compare {} with {} using ==",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                return Ty::Error;
            }
            Ty::Bool
        }
        // Reference equality: only valid for data types (§8.6)
        // For now, require same type on both sides
        BinOp::RefEq => {
            if left != right {
                report.add(
                    Diagnostic::error(format!(
                        "cannot compare {} with {} using ===",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                return Ty::Error;
            }
            Ty::Bool
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            // Allow Int, Float, and Error (escape hatch for value types not yet registered)
            if left == right && matches!(left, Ty::Int | Ty::Float | Ty::Error) {
                Ty::Bool
            } else {
                report.add(
                    Diagnostic::error(format!(
                        "comparison requires matching Int or Float operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                Ty::Error
            }
        }
        // Logical: Bool operands -> Bool (§10.1)
        BinOp::And | BinOp::Or => {
            if !matches!(left, Ty::Bool) || !matches!(right, Ty::Bool) {
                report.add(
                    Diagnostic::error(format!(
                        "logical operator requires Bool operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0307")
                    .with_label(Label::new(span, "expected Bool")),
                );
            }
            Ty::Bool
        }
    }
}

fn check_type_match(expected: &Ty, actual: &Ty, span: Span, report: &mut Report) {
    if !types_compatible(expected, actual) {
        report.add(
            Diagnostic::error(format!(
                "type mismatch: expected {}, found {}",
                expected.display_name(),
                actual.display_name()
            ))
            .with_code("E0308")
            .with_label(Label::new(
                span,
                format!("expected {}", expected.display_name()),
            )),
        );
    }
}

fn bind_pattern_types(pat: &Pattern, env: &mut TypeEnv) {
    match &pat.kind {
        PatternKind::Ident(name) => {
            env.define(name.clone(), Ty::Error); // actual type from match subject; deferred
        }
        PatternKind::Constructor(_, fields) => {
            for field in fields {
                bind_pattern_types(&field.pattern, env);
            }
        }
        _ => {}
    }
}

/// §10.3: `match` must be exhaustive.
/// Reports E0400 when an enumerable subject type (ADT, Option, Result, Bool)
/// has uncovered variants and no wildcard/ident catch-all arm.
///
/// Limitations (future work):
/// - Nested Constructor exhaustiveness (e.g. Err(NotFound) only) not checked
/// - Unknown Named types and generics other than Option/Result are skipped
/// - Literal exhaustiveness (Int/String) not checked
fn check_match_exhaustiveness(
    subject_ty: &Ty,
    match_expr: &MatchExpr,
    env: &TypeEnv,
    report: &mut Report,
) {
    // A wildcard or ident-binding arm is a catch-all — exhaustiveness is satisfied.
    // Rationale: in a match context, a bare lowercase ident is always a fresh
    // binding (not a constructor reference — those parse as `Constructor(name, _)`).
    // This mirrors the semantics of Rust and OCaml: `when x` binds `x` to the
    // subject and matches any value, identical to `when _` except that the value
    // is nameable inside the arm body.
    let has_catchall = match_expr.arms.iter().any(|arm| {
        matches!(arm.pattern.kind, PatternKind::Wildcard | PatternKind::Ident(_))
    });
    if has_catchall {
        return;
    }

    // Determine the expected variant set for the subject type.
    let (type_display, expected): (String, Vec<String>) = match subject_ty {
        Ty::Bool => ("Bool".into(), vec!["true".into(), "false".into()]),
        Ty::Named(name) => match env.adt_variants(name) {
            Some(vs) => (name.clone(), vs.clone()),
            None => return, // not an enumerable type → skip
        },
        Ty::Generic(name, _) if name == "Option" || name == "Result" => {
            match env.adt_variants(name) {
                Some(vs) => (name.clone(), vs.clone()),
                None => return,
            }
        }
        _ => return, // non-enumerable or unresolved type → skip
    };

    if expected.is_empty() {
        return;
    }

    // Collect variant names matched by Constructor/BoolLit patterns.
    let mut covered: HashSet<String> = HashSet::new();
    for arm in &match_expr.arms {
        match &arm.pattern.kind {
            PatternKind::Constructor(name, _) => {
                covered.insert(name.clone());
            }
            PatternKind::BoolLit(b) => {
                covered.insert(if *b { "true".into() } else { "false".into() });
            }
            _ => {}
        }
    }

    let missing: Vec<String> = expected
        .iter()
        .filter(|v| !covered.contains(v.as_str()))
        .cloned()
        .collect();

    if !missing.is_empty() {
        let quoted: Vec<String> = missing.iter().map(|v| format!("`{v}`")).collect();
        report.add(
            Diagnostic::error(format!(
                "non-exhaustive match on {type_display}: missing pattern {}",
                quoted.join(", ")
            ))
            .with_code("E0400")
            .with_label(Label::new(match_expr.span, "non-exhaustive patterns"))
            .with_note(format!(
                "not covered: {}. Add arms for these patterns or use `_` for a catch-all.",
                quoted.join(", ")
            )),
        );
    }
}

fn stmt_type(stmt: &Stmt, env: &mut TypeEnv, report: &mut Report) -> Ty {
    match stmt {
        Stmt::Expr(s) => infer_expr(&s.expr, env, report),
        _ => Ty::Unit,
    }
}
