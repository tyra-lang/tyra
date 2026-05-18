// Generic function monomorphization — AST-level type substitution (§8.4).
//
// Walks a FnDef AST and replaces type parameter names with concrete types.
// Used by turbofish call lowering: `fn identity<T>(_ x: T) -> T` called as
// `identity::<Int>(42)` produces a monomorphized `identity__Int` function.

use std::collections::HashMap;

use tyra_ast::*;
use tyra_types::Ty;

/// Create a monomorphized copy of a FnDef with type parameters substituted.
/// Replaces all occurrences of type parameter names with their concrete types.
pub fn substitute_fn_def(
    f: &FnDef,
    subst: &HashMap<String, Ty>,
    mangled_name: &str,
) -> FnDef {
    let params = f
        .params
        .iter()
        .map(|p| Param {
            label: p.label.clone(),
            name: p.name.clone(),
            type_annotation: substitute_type_expr(&p.type_annotation, subst),
            span: p.span,
        })
        .collect();

    let return_type = f
        .return_type
        .as_ref()
        .map(|rt| substitute_type_expr(rt, subst));

    let body = f
        .body
        .iter()
        .map(|s| substitute_stmt(s, subst))
        .collect();

    FnDef {
        name: mangled_name.to_string(),
        type_params: vec![], // concrete, no longer generic
        self_param: f.self_param.clone(),
        params,
        return_type,
        body,
        is_async: f.is_async,
        is_export: f.is_export,
        span: f.span,
    }
}

/// Substitute type parameter names in a TypeExpr.
fn substitute_type_expr(te: &TypeExpr, subst: &HashMap<String, Ty>) -> TypeExpr {
    let kind = match &te.kind {
        TypeExprKind::Named(name) => {
            if let Some(concrete) = subst.get(name) {
                ty_to_type_expr_kind(concrete, te.span)
            } else {
                TypeExprKind::Named(name.clone())
            }
        }
        TypeExprKind::Generic(name, args) => {
            let new_args = args
                .iter()
                .map(|a| substitute_type_expr(a, subst))
                .collect();
            TypeExprKind::Generic(name.clone(), new_args)
        }
        TypeExprKind::Fn(params, ret) => {
            let new_params = params
                .iter()
                .map(|p| substitute_type_expr(p, subst))
                .collect();
            let new_ret = Box::new(substitute_type_expr(ret, subst));
            TypeExprKind::Fn(new_params, new_ret)
        }
    };
    TypeExpr { kind, span: te.span }
}

/// Convert an internal Ty back to an AST TypeExprKind for substitution.
/// Uses the provided span for any synthetic TypeExpr nodes needed.
fn ty_to_type_expr_kind(ty: &Ty, span: Span) -> TypeExprKind {
    match ty {
        Ty::Int => TypeExprKind::Named("Int".into()),
        Ty::Float => TypeExprKind::Named("Float".into()),
        Ty::Bool => TypeExprKind::Named("Bool".into()),
        Ty::String => TypeExprKind::Named("String".into()),
        Ty::Unit => TypeExprKind::Named("Unit".into()),
        Ty::Never => TypeExprKind::Named("Never".into()),
        Ty::Rune => TypeExprKind::Named("Rune".into()),
        Ty::Bytes => TypeExprKind::Named("Bytes".into()),
        Ty::Named(name) => TypeExprKind::Named(name.clone()),
        Ty::Generic(name, args) => {
            let te_args: Vec<TypeExpr> = args
                .iter()
                .map(|a| TypeExpr {
                    kind: ty_to_type_expr_kind(a, span),
                    span,
                })
                .collect();
            TypeExprKind::Generic(name.clone(), te_args)
        }
        _ => TypeExprKind::Named("Unknown".into()),
    }
}

/// Substitute type parameters in a statement.
fn substitute_stmt(stmt: &Stmt, subst: &HashMap<String, Ty>) -> Stmt {
    match stmt {
        Stmt::Let(s) => Stmt::Let(LetStmt {
            name: s.name.clone(),
            type_annotation: s
                .type_annotation
                .as_ref()
                .map(|t| substitute_type_expr(t, subst)),
            value: substitute_expr(&s.value, subst),
            span: s.span,
        }),
        Stmt::Mut(s) => Stmt::Mut(MutStmt {
            name: s.name.clone(),
            type_annotation: s
                .type_annotation
                .as_ref()
                .map(|t| substitute_type_expr(t, subst)),
            value: substitute_expr(&s.value, subst),
            span: s.span,
        }),
        Stmt::Return(s) => Stmt::Return(ReturnStmt {
            value: s.value.as_ref().map(|v| substitute_expr(v, subst)),
            span: s.span,
        }),
        Stmt::Defer(s) => Stmt::Defer(DeferStmt {
            expr: substitute_expr(&s.expr, subst),
            span: s.span,
        }),
        Stmt::Break(s) => Stmt::Break(BreakStmt { span: s.span }),
        Stmt::Continue(s) => Stmt::Continue(ContinueStmt { span: s.span }),
        Stmt::Expr(s) => Stmt::Expr(ExprStmt {
            expr: substitute_expr(&s.expr, subst),
            span: s.span,
        }),
    }
}

/// Substitute type parameters in an expression (deep walk).
fn substitute_expr(expr: &Expr, subst: &HashMap<String, Ty>) -> Expr {
    let kind = match &expr.kind {
        // Leaves — no substitution needed
        ExprKind::IntLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::StringLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::UnitLit
        | ExprKind::Ident(_) => expr.kind.clone(),

        // Recursive variants
        ExprKind::StringInterp(parts) => {
            let new_parts = parts
                .iter()
                .map(|p| match p {
                    StringPart::Lit(s) => StringPart::Lit(s.clone()),
                    StringPart::Expr(e) => StringPart::Expr(substitute_expr(e, subst)),
                })
                .collect();
            ExprKind::StringInterp(new_parts)
        }
        ExprKind::ListLit(items) => {
            ExprKind::ListLit(items.iter().map(|e| substitute_expr(e, subst)).collect())
        }
        ExprKind::MapLit(entries) => {
            let new_entries = entries
                .iter()
                .map(|(k, v)| (substitute_expr(k, subst), substitute_expr(v, subst)))
                .collect();
            ExprKind::MapLit(new_entries)
        }
        ExprKind::FieldAccess(obj, field) => {
            ExprKind::FieldAccess(Box::new(substitute_expr(obj, subst)), field.clone())
        }
        ExprKind::BinaryOp(lhs, op, rhs) => ExprKind::BinaryOp(
            Box::new(substitute_expr(lhs, subst)),
            *op,
            Box::new(substitute_expr(rhs, subst)),
        ),
        ExprKind::UnaryOp(op, operand) => {
            ExprKind::UnaryOp(*op, Box::new(substitute_expr(operand, subst)))
        }
        ExprKind::Assign(lhs, rhs) => ExprKind::Assign(
            Box::new(substitute_expr(lhs, subst)),
            Box::new(substitute_expr(rhs, subst)),
        ),
        ExprKind::Call(callee, args) => ExprKind::Call(
            Box::new(substitute_expr(callee, subst)),
            substitute_args(args, subst),
        ),
        ExprKind::TurbofishCall(callee, type_args, args) => {
            let new_type_args = type_args
                .iter()
                .map(|t| substitute_type_expr(t, subst))
                .collect();
            ExprKind::TurbofishCall(
                Box::new(substitute_expr(callee, subst)),
                new_type_args,
                substitute_args(args, subst),
            )
        }
        ExprKind::Index(obj, idx) => ExprKind::Index(
            Box::new(substitute_expr(obj, subst)),
            Box::new(substitute_expr(idx, subst)),
        ),
        ExprKind::Propagate(inner) => {
            ExprKind::Propagate(Box::new(substitute_expr(inner, subst)))
        }
        ExprKind::Await(inner) => ExprKind::Await(Box::new(substitute_expr(inner, subst))),
        ExprKind::Spawn(inner) => ExprKind::Spawn(Box::new(substitute_expr(inner, subst))),
        ExprKind::If(if_expr) => {
            ExprKind::If(Box::new(substitute_if_expr(if_expr, subst)))
        }
        ExprKind::Match(m) => {
            ExprKind::Match(Box::new(substitute_match_expr(m, subst)))
        }
        ExprKind::For(f) => ExprKind::For(Box::new(ForExpr {
            binding: f.binding.clone(),
            iter: substitute_expr(&f.iter, subst),
            body: f.body.iter().map(|s| substitute_stmt(s, subst)).collect(),
            span: f.span,
        })),
        ExprKind::While(w) => ExprKind::While(Box::new(WhileExpr {
            condition: substitute_expr(&w.condition, subst),
            body: w.body.iter().map(|s| substitute_stmt(s, subst)).collect(),
            span: w.span,
        })),
        ExprKind::Lambda(l) => {
            let new_params = l
                .params
                .iter()
                .map(|p| Param {
                    label: p.label.clone(),
                    name: p.name.clone(),
                    type_annotation: substitute_type_expr(&p.type_annotation, subst),
                    span: p.span,
                })
                .collect();
            ExprKind::Lambda(Box::new(LambdaExpr {
                params: new_params,
                return_type: l
                    .return_type
                    .as_ref()
                    .map(|rt| substitute_type_expr(rt, subst)),
                body: l.body.iter().map(|s| substitute_stmt(s, subst)).collect(),
                span: l.span,
            }))
        }
    };
    Expr {
        kind,
        span: expr.span,
    }
}

fn substitute_args(args: &[Arg], subst: &HashMap<String, Ty>) -> Vec<Arg> {
    args.iter()
        .map(|a| Arg {
            label: a.label.clone(),
            value: substitute_expr(&a.value, subst),
            span: a.span,
        })
        .collect()
}

fn substitute_if_expr(ie: &IfExpr, subst: &HashMap<String, Ty>) -> IfExpr {
    IfExpr {
        condition: substitute_expr(&ie.condition, subst),
        then_body: ie
            .then_body
            .iter()
            .map(|s| substitute_stmt(s, subst))
            .collect(),
        else_body: ie.else_body.as_ref().map(|eb| match eb {
            ElseBranch::Else(stmts) => {
                ElseBranch::Else(stmts.iter().map(|s| substitute_stmt(s, subst)).collect())
            }
            ElseBranch::ElseIf(inner) => {
                ElseBranch::ElseIf(Box::new(substitute_if_expr(inner, subst)))
            }
        }),
        span: ie.span,
    }
}

fn substitute_match_expr(m: &MatchExpr, subst: &HashMap<String, Ty>) -> MatchExpr {
    MatchExpr {
        subject: substitute_expr(&m.subject, subst),
        arms: m
            .arms
            .iter()
            .map(|arm| MatchArm {
                pattern: arm.pattern.clone(), // patterns don't contain type expressions
                body: arm.body.iter().map(|s| substitute_stmt(s, subst)).collect(),
                span: arm.span,
            })
            .collect(),
        span: m.span,
    }
}
