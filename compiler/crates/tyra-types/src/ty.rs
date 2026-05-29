// Internal type representation for the Tyra type checker.
// spec reference: §7.2 (primitives), §8 (type system), §9.4 (function types)
// ADR reference: docs/design/0020-hm-inference.md (HM rank-1 inference, v0.8)

use std::collections::HashMap;

/// Abilities a type can have (§8). Auto-derived for value/data/ADT per
/// the spec's structural rules; primitives get theirs from the prelude.
///
/// - `Eq`: supports `==` / `!=`
/// - `Hash`: can be a key in Set/Map (implies Eq)
/// - `Ord`: supports `<` / `<=` / `>` / `>=`
/// - `Debug`: supports string-interpolation and auto-Debug formatting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ability {
    Eq,
    Hash,
    Ord,
    Debug,
}

/// The internal representation of a Tyra type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    // -- Primitives (§7.2) --
    Int,
    Float,
    Bool,
    String,
    Rune,
    Bytes,
    Unit,
    Never,

    // -- Composite types --
    /// Named user type: value, data, or ADT. Identified by name for now.
    Named(String),

    /// Generic type application: `List<Int>`, `Option<String>`, `Result<T, E>`
    Generic(String, Vec<Ty>),

    /// Function type: `fn(Int, Int) -> Bool` (§9.4)
    Fn(Vec<Ty>, Box<Ty>),

    /// Type variable (for inference): not yet resolved
    Var(u32),

    /// Error sentinel: used when type checking fails to avoid cascading errors
    Error,
}

impl Ty {
    /// Check if this is a primitive type.
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            Ty::Int
                | Ty::Float
                | Ty::Bool
                | Ty::String
                | Ty::Rune
                | Ty::Bytes
                | Ty::Unit
                | Ty::Never
        )
    }

    /// Check if this is the Never type (bottom type, subtype of everything).
    pub fn is_never(&self) -> bool {
        matches!(self, Ty::Never)
    }

    /// Check if this is an error sentinel.
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }

    /// Check if this is an Option<T> type.
    pub fn is_option(&self) -> bool {
        matches!(self, Ty::Generic(name, args) if name == "Option" && args.len() == 1)
    }

    /// Check if this is a Result<T, E> type.
    pub fn is_result(&self) -> bool {
        matches!(self, Ty::Generic(name, args) if name == "Result" && args.len() == 2)
    }

    /// Extract the inner type T from Option<T>.
    pub fn option_inner(&self) -> Option<&Ty> {
        match self {
            Ty::Generic(name, args) if name == "Option" && args.len() == 1 => Some(&args[0]),
            _ => None,
        }
    }

    /// Extract the Ok type T from Result<T, E>.
    pub fn result_ok_type(&self) -> Option<&Ty> {
        match self {
            Ty::Generic(name, args) if name == "Result" && args.len() == 2 => Some(&args[0]),
            _ => None,
        }
    }

    /// Extract the Err type E from Result<T, E>.
    pub fn result_err_type(&self) -> Option<&Ty> {
        match self {
            Ty::Generic(name, args) if name == "Result" && args.len() == 2 => Some(&args[1]),
            _ => None,
        }
    }

    /// Check if this is a List<T> type.
    pub fn is_list(&self) -> bool {
        matches!(self, Ty::Generic(name, args) if name == "List" && args.len() == 1)
    }

    /// Extract the element type T from List<T>.
    pub fn list_elem(&self) -> Option<&Ty> {
        match self {
            Ty::Generic(name, args) if name == "List" && args.len() == 1 => Some(&args[0]),
            _ => None,
        }
    }

    pub fn is_set(&self) -> bool {
        matches!(self, Ty::Generic(name, args) if name == "Set" && args.len() == 1)
    }

    pub fn set_elem(&self) -> Option<&Ty> {
        match self {
            Ty::Generic(name, args) if name == "Set" && args.len() == 1 => Some(&args[0]),
            _ => None,
        }
    }

    /// Generate a monomorphized name for codegen.
    /// e.g., Option<Int> → "Option__Int", Result<String, AppError> → "Result__String__AppError"
    pub fn monomorphized_name(&self) -> String {
        match self {
            Ty::Generic(name, args) => {
                let arg_names: Vec<String> = args.iter().map(|a| a.monomorphized_name()).collect();
                format!("{}__{}", name, arg_names.join("__"))
            }
            Ty::Int => "Int".into(),
            Ty::Float => "Float".into(),
            Ty::Bool => "Bool".into(),
            Ty::String => "String".into(),
            Ty::Rune => "Rune".into(),
            Ty::Bytes => "Bytes".into(),
            Ty::Unit => "Unit".into(),
            Ty::Never => "Never".into(),
            Ty::Named(name) => name.clone(),
            _ => "Unknown".into(),
        }
    }

    /// Human-readable type name for diagnostics.
    pub fn display_name(&self) -> String {
        match self {
            Ty::Int => "Int".into(),
            Ty::Float => "Float".into(),
            Ty::Bool => "Bool".into(),
            Ty::String => "String".into(),
            Ty::Rune => "Rune".into(),
            Ty::Bytes => "Bytes".into(),
            Ty::Unit => "Unit".into(),
            Ty::Never => "Never".into(),
            Ty::Named(name) => name.clone(),
            Ty::Generic(name, args) => {
                let args_str: Vec<_> = args.iter().map(|a| a.display_name()).collect();
                format!("{}<{}>", name, args_str.join(", "))
            }
            Ty::Fn(params, ret) => {
                let params_str: Vec<_> = params.iter().map(|p| p.display_name()).collect();
                format!("fn({}) -> {}", params_str.join(", "), ret.display_name())
            }
            Ty::Var(id) => format!("?{id}"),
            Ty::Error => "<error>".into(),
        }
    }

    /// Resolve a type expression from the AST into an internal Ty.
    pub fn from_type_expr(expr: &tyra_ast::TypeExpr) -> Ty {
        match &expr.kind {
            tyra_ast::TypeExprKind::Named(name) => match name.as_str() {
                "Int" => Ty::Int,
                "Float" => Ty::Float,
                "Bool" => Ty::Bool,
                "String" => Ty::String,
                "Rune" => Ty::Rune,
                "Bytes" => Ty::Bytes,
                "Unit" => Ty::Unit,
                "Never" => Ty::Never,
                _ => Ty::Named(name.clone()),
            },
            tyra_ast::TypeExprKind::Generic(name, args) => {
                let resolved_args: Vec<Ty> = args.iter().map(Ty::from_type_expr).collect();
                Ty::Generic(name.clone(), resolved_args)
            }
            tyra_ast::TypeExprKind::Fn(params, ret) => {
                let param_tys: Vec<Ty> = params.iter().map(Ty::from_type_expr).collect();
                Ty::Fn(param_tys, Box::new(Ty::from_type_expr(ret)))
            }
        }
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_name())
    }
}

// ============================================================================
// Hindley-Milner rank-1 type inference (ADR 0020)
// ============================================================================
//
// `Ty::Var(u32)` carries a `TyVarId` payload (the `u32` is the wrapped id).
// We keep the variant as `Ty::Var(u32)` rather than `Ty::Var(TyVarId)` to
// avoid touching every downstream pattern-match in tyra-mir / tyra-codegen-llvm
// — `TyVarId` is a thin newtype that converts to/from `u32` cheaply.

/// Unique identifier for a type-inference variable.
///
/// Each fresh variable produced during type checking gets a distinct id;
/// `Substitution` maps these ids to their resolved types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TyVarId(pub u32);

impl TyVarId {
    /// Wrap this id in a `Ty::Var` variant.
    pub fn into_ty(self) -> Ty {
        Ty::Var(self.0)
    }
}

impl From<u32> for TyVarId {
    fn from(v: u32) -> Self {
        TyVarId(v)
    }
}

impl From<TyVarId> for u32 {
    fn from(v: TyVarId) -> u32 {
        v.0
    }
}

/// Substitution map: maps each `TyVarId` to its resolved type.
///
/// Built up incrementally by `unify`.  Use `apply` to obtain a ground type
/// after unification completes.
#[derive(Debug, Default, Clone)]
pub struct Substitution {
    map: HashMap<TyVarId, Ty>,
}

impl Substitution {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk a type one level: if it is a variable with a binding, follow
    /// the chain until we hit a non-variable or an unbound variable.
    pub fn walk(&self, ty: &Ty) -> Ty {
        let mut current = ty.clone();
        loop {
            match current {
                Ty::Var(id) => match self.map.get(&TyVarId(id)) {
                    Some(t) => current = t.clone(),
                    None => return Ty::Var(id),
                },
                other => return other,
            }
        }
    }

    /// Bind a variable to a type. Caller is responsible for performing the
    /// occurs check before calling.
    pub fn bind(&mut self, id: TyVarId, ty: Ty) {
        self.map.insert(id, ty);
    }

    /// Apply the substitution deeply to a type, recursing into composite
    /// types so the result contains no resolved-but-unwalked variables.
    pub fn apply(&self, ty: &Ty) -> Ty {
        match self.walk(ty) {
            Ty::Generic(name, args) => {
                Ty::Generic(name, args.iter().map(|a| self.apply(a)).collect())
            }
            Ty::Fn(params, ret) => Ty::Fn(
                params.iter().map(|p| self.apply(p)).collect(),
                Box::new(self.apply(&ret)),
            ),
            t => t,
        }
    }

    /// Has this id been bound?
    pub fn is_bound(&self, id: TyVarId) -> bool {
        self.map.contains_key(&id)
    }
}

/// Errors that can arise during unification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError {
    /// `expected` and `found` are structurally incompatible.
    Mismatch { expected: Ty, found: Ty },
    /// Binding `id` to `ty` would create an infinite type.
    OccursCheck { id: TyVarId, ty: Ty },
}

/// Does the variable `id` occur anywhere in `ty` (after walking)?
///
/// Required to reject infinite types like `a = List<a>`.
fn occurs(id: TyVarId, ty: &Ty, subst: &Substitution) -> bool {
    let ty = subst.walk(ty);
    match &ty {
        Ty::Var(other) => *other == id.0,
        Ty::Generic(_, args) => args.iter().any(|a| occurs(id, a, subst)),
        Ty::Fn(params, ret) => {
            params.iter().any(|p| occurs(id, p, subst)) || occurs(id, ret, subst)
        }
        _ => false,
    }
}

/// Unify two types under the current substitution.
///
/// On success the substitution is updated in place; on failure the
/// substitution may be partially updated (callers that want atomicity
/// should clone first).
///
/// `Ty::Error` is treated as compatible with anything to avoid cascading
/// diagnostics — the upstream error already produced a report.
/// `Ty::Never` is similarly compatible (bottom type).
pub fn unify(a: &Ty, b: &Ty, subst: &mut Substitution) -> Result<(), UnifyError> {
    let a = subst.walk(a);
    let b = subst.walk(b);

    // Error / Never short-circuit: don't cascade.
    if matches!(a, Ty::Error) || matches!(b, Ty::Error) {
        return Ok(());
    }
    if matches!(a, Ty::Never) || matches!(b, Ty::Never) {
        return Ok(());
    }

    match (&a, &b) {
        // Same variable on both sides
        (Ty::Var(ia), Ty::Var(ib)) if ia == ib => Ok(()),
        // Variable on left → bind (with occurs check)
        (Ty::Var(id), _) => {
            let tvar = TyVarId(*id);
            if occurs(tvar, &b, subst) {
                return Err(UnifyError::OccursCheck { id: tvar, ty: b });
            }
            subst.bind(tvar, b);
            Ok(())
        }
        // Variable on right → bind (with occurs check)
        (_, Ty::Var(id)) => {
            let tvar = TyVarId(*id);
            if occurs(tvar, &a, subst) {
                return Err(UnifyError::OccursCheck { id: tvar, ty: a });
            }
            subst.bind(tvar, a);
            Ok(())
        }
        // Concrete primitives
        (Ty::Int, Ty::Int)
        | (Ty::Float, Ty::Float)
        | (Ty::Bool, Ty::Bool)
        | (Ty::String, Ty::String)
        | (Ty::Rune, Ty::Rune)
        | (Ty::Bytes, Ty::Bytes)
        | (Ty::Unit, Ty::Unit) => Ok(()),
        // Named types: must match by name (no parameters yet).
        (Ty::Named(n1), Ty::Named(n2)) if n1 == n2 => Ok(()),
        // Generic types: head + arity match, recursively unify args.
        (Ty::Generic(n1, a1), Ty::Generic(n2, a2)) if n1 == n2 && a1.len() == a2.len() => {
            for (x, y) in a1.iter().zip(a2.iter()) {
                unify(x, y, subst)?;
            }
            Ok(())
        }
        // Function types: arity match, recursively unify params + return.
        (Ty::Fn(p1, r1), Ty::Fn(p2, r2)) if p1.len() == p2.len() => {
            for (x, y) in p1.iter().zip(p2.iter()) {
                unify(x, y, subst)?;
            }
            unify(r1, r2, subst)
        }
        // Anything else is a structural mismatch.
        _ => Err(UnifyError::Mismatch {
            expected: a,
            found: b,
        }),
    }
}

/// Check if two types are compatible (assignable).
///
/// This is the public predicate used throughout the checker. It is a thin
/// wrapper around `unify`: two types are compatible iff they unify under
/// a fresh substitution.
///
/// Special cases preserved from the v0.7 behavior:
/// - `Ty::Never` is compatible with everything (bottom type).
/// - `Ty::Error` is compatible with everything (avoids cascading
///   diagnostics — the upstream error already produced a report).
/// - `Ty::Var` unifies with anything by binding the substitution; since
///   the substitution is discarded here, this behaves as "wildcard"
///   compatibility, matching the v0.7 semantics that lets
///   `let xs: List<Int> = []` type-check (the empty list literal has
///   type `List<Var(_)>`).
///
/// For diagnostics that *require* a binding to be remembered (and the
/// `unify` failure mode to be inspected), callers should invoke `unify`
/// directly with a persistent `Substitution`.
pub fn types_compatible(expected: &Ty, actual: &Ty) -> bool {
    let mut subst = Substitution::new();
    unify(expected, actual, &mut subst).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_display() {
        assert_eq!(Ty::Int.display_name(), "Int");
        assert_eq!(Ty::String.display_name(), "String");
        assert_eq!(Ty::Unit.display_name(), "Unit");
    }

    #[test]
    fn generic_display() {
        let t = Ty::Generic("List".into(), vec![Ty::Int]);
        assert_eq!(t.display_name(), "List<Int>");

        let t = Ty::Generic(
            "Result".into(),
            vec![Ty::String, Ty::Named("AppError".into())],
        );
        assert_eq!(t.display_name(), "Result<String, AppError>");
    }

    #[test]
    fn fn_display() {
        let t = Ty::Fn(vec![Ty::Int, Ty::Int], Box::new(Ty::Bool));
        assert_eq!(t.display_name(), "fn(Int, Int) -> Bool");
    }

    #[test]
    fn never_compatible_with_everything() {
        assert!(types_compatible(&Ty::Int, &Ty::Never));
        assert!(types_compatible(&Ty::String, &Ty::Never));
        assert!(types_compatible(&Ty::Bool, &Ty::Never));
    }

    #[test]
    fn error_compatible_with_everything() {
        assert!(types_compatible(&Ty::Int, &Ty::Error));
        assert!(types_compatible(&Ty::Error, &Ty::Int));
    }

    #[test]
    fn same_types_compatible() {
        assert!(types_compatible(&Ty::Int, &Ty::Int));
        assert!(types_compatible(&Ty::String, &Ty::String));
    }

    #[test]
    fn different_types_not_compatible() {
        assert!(!types_compatible(&Ty::Int, &Ty::String));
        assert!(!types_compatible(&Ty::Bool, &Ty::Float));
    }

    #[test]
    fn is_option() {
        let opt = Ty::Generic("Option".into(), vec![Ty::Int]);
        assert!(opt.is_option());
        assert!(!opt.is_result());
        assert!(!Ty::Int.is_option());
    }

    #[test]
    fn is_result() {
        let res = Ty::Generic(
            "Result".into(),
            vec![Ty::String, Ty::Named("AppError".into())],
        );
        assert!(res.is_result());
        assert!(!res.is_option());
    }

    #[test]
    fn option_inner_type() {
        let opt = Ty::Generic("Option".into(), vec![Ty::Int]);
        assert_eq!(opt.option_inner(), Some(&Ty::Int));
        assert_eq!(Ty::Int.option_inner(), None);
    }

    #[test]
    fn result_types() {
        let res = Ty::Generic("Result".into(), vec![Ty::String, Ty::Named("Err".into())]);
        assert_eq!(res.result_ok_type(), Some(&Ty::String));
        assert_eq!(res.result_err_type(), Some(&Ty::Named("Err".into())));
        assert_eq!(Ty::Int.result_ok_type(), None);
    }

    #[test]
    fn is_list() {
        let list = Ty::Generic("List".into(), vec![Ty::Int]);
        assert!(list.is_list());
        assert!(!list.is_option());
        assert!(!Ty::Int.is_list());
    }

    #[test]
    fn list_elem_type() {
        let list = Ty::Generic("List".into(), vec![Ty::String]);
        assert_eq!(list.list_elem(), Some(&Ty::String));
        assert_eq!(Ty::Int.list_elem(), None);

        let list_named = Ty::Generic("List".into(), vec![Ty::Named("User".into())]);
        assert_eq!(list_named.list_elem(), Some(&Ty::Named("User".into())));
    }

    #[test]
    fn monomorphized_name() {
        let opt = Ty::Generic("Option".into(), vec![Ty::Int]);
        assert_eq!(opt.monomorphized_name(), "Option__Int");

        let res = Ty::Generic(
            "Result".into(),
            vec![Ty::String, Ty::Named("AppError".into())],
        );
        assert_eq!(res.monomorphized_name(), "Result__String__AppError");

        assert_eq!(Ty::Int.monomorphized_name(), "Int");
        assert_eq!(Ty::Named("User".into()).monomorphized_name(), "User");

        // Nested generics
        let nested = Ty::Generic(
            "Option".into(),
            vec![Ty::Generic("List".into(), vec![Ty::Int])],
        );
        assert_eq!(nested.monomorphized_name(), "Option__List__Int");
    }

    // ========================================================================
    // HM unification (ADR 0020)
    // ========================================================================

    #[test]
    fn unify_concrete_same() {
        let mut s = Substitution::new();
        assert!(unify(&Ty::Int, &Ty::Int, &mut s).is_ok());
    }

    #[test]
    fn unify_concrete_mismatch() {
        let mut s = Substitution::new();
        let err = unify(&Ty::Int, &Ty::String, &mut s).unwrap_err();
        assert!(matches!(err, UnifyError::Mismatch { .. }));
    }

    #[test]
    fn unify_var_with_concrete_binds() {
        let mut s = Substitution::new();
        let v = Ty::Var(0);
        assert!(unify(&v, &Ty::Int, &mut s).is_ok());
        assert_eq!(s.apply(&v), Ty::Int);
    }

    #[test]
    fn unify_concrete_with_var_binds() {
        let mut s = Substitution::new();
        let v = Ty::Var(7);
        assert!(unify(&Ty::String, &v, &mut s).is_ok());
        assert_eq!(s.apply(&v), Ty::String);
    }

    #[test]
    fn unify_generic_recursive() {
        let mut s = Substitution::new();
        let v = Ty::Var(0);
        let lhs = Ty::Generic("List".into(), vec![v.clone()]);
        let rhs = Ty::Generic("List".into(), vec![Ty::Int]);
        assert!(unify(&lhs, &rhs, &mut s).is_ok());
        assert_eq!(s.apply(&v), Ty::Int);
        assert_eq!(s.apply(&lhs), rhs);
    }

    #[test]
    fn unify_generic_name_mismatch() {
        let mut s = Substitution::new();
        let lhs = Ty::Generic("List".into(), vec![Ty::Int]);
        let rhs = Ty::Generic("Option".into(), vec![Ty::Int]);
        assert!(unify(&lhs, &rhs, &mut s).is_err());
    }

    #[test]
    fn unify_generic_arity_mismatch() {
        let mut s = Substitution::new();
        let lhs = Ty::Generic("Result".into(), vec![Ty::Int]);
        let rhs = Ty::Generic("Result".into(), vec![Ty::Int, Ty::String]);
        assert!(unify(&lhs, &rhs, &mut s).is_err());
    }

    #[test]
    fn unify_occurs_check_rejects_infinite_type() {
        // a = List<a> must be rejected.
        let mut s = Substitution::new();
        let v = Ty::Var(0);
        let recursive = Ty::Generic("List".into(), vec![v.clone()]);
        let err = unify(&v, &recursive, &mut s).unwrap_err();
        assert!(matches!(err, UnifyError::OccursCheck { .. }));
    }

    #[test]
    fn unify_var_var_same_id_ok() {
        let mut s = Substitution::new();
        assert!(unify(&Ty::Var(3), &Ty::Var(3), &mut s).is_ok());
    }

    #[test]
    fn unify_var_var_distinct_binds_one_to_other() {
        let mut s = Substitution::new();
        assert!(unify(&Ty::Var(1), &Ty::Var(2), &mut s).is_ok());
        // After unification both variables walk to the same representative.
        assert_eq!(s.walk(&Ty::Var(1)), s.walk(&Ty::Var(2)));
    }

    #[test]
    fn unify_chain_through_substitution() {
        // Var(0) := Var(1), then Var(1) := Int → walking Var(0) yields Int.
        let mut s = Substitution::new();
        assert!(unify(&Ty::Var(0), &Ty::Var(1), &mut s).is_ok());
        assert!(unify(&Ty::Var(1), &Ty::Int, &mut s).is_ok());
        assert_eq!(s.apply(&Ty::Var(0)), Ty::Int);
    }

    #[test]
    fn unify_fn_types() {
        let mut s = Substitution::new();
        let lhs = Ty::Fn(vec![Ty::Var(0)], Box::new(Ty::Var(1)));
        let rhs = Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool));
        assert!(unify(&lhs, &rhs, &mut s).is_ok());
        assert_eq!(s.apply(&Ty::Var(0)), Ty::Int);
        assert_eq!(s.apply(&Ty::Var(1)), Ty::Bool);
    }

    #[test]
    fn unify_fn_arity_mismatch() {
        let mut s = Substitution::new();
        let lhs = Ty::Fn(vec![Ty::Int], Box::new(Ty::Unit));
        let rhs = Ty::Fn(vec![Ty::Int, Ty::Int], Box::new(Ty::Unit));
        assert!(unify(&lhs, &rhs, &mut s).is_err());
    }

    #[test]
    fn unify_error_short_circuits() {
        // Error should not cascade as a mismatch.
        let mut s = Substitution::new();
        assert!(unify(&Ty::Error, &Ty::Int, &mut s).is_ok());
        assert!(unify(&Ty::String, &Ty::Error, &mut s).is_ok());
    }

    #[test]
    fn unify_never_short_circuits() {
        let mut s = Substitution::new();
        assert!(unify(&Ty::Never, &Ty::Int, &mut s).is_ok());
        assert!(unify(&Ty::Int, &Ty::Never, &mut s).is_ok());
    }

    #[test]
    fn substitution_apply_recurses_into_generics() {
        let mut s = Substitution::new();
        s.bind(TyVarId(0), Ty::Int);
        let nested = Ty::Generic(
            "Option".into(),
            vec![Ty::Generic("List".into(), vec![Ty::Var(0)])],
        );
        let applied = s.apply(&nested);
        assert_eq!(
            applied,
            Ty::Generic(
                "Option".into(),
                vec![Ty::Generic("List".into(), vec![Ty::Int])]
            )
        );
    }

    #[test]
    fn types_compatible_uses_unify_for_concrete_match() {
        assert!(types_compatible(&Ty::Int, &Ty::Int));
        assert!(!types_compatible(&Ty::Int, &Ty::Bool));
    }

    #[test]
    fn types_compatible_var_acts_as_wildcard() {
        // Preserved v0.7 semantics: an unresolved var unifies with any type.
        assert!(types_compatible(&Ty::Var(0), &Ty::Int));
        assert!(types_compatible(&Ty::Int, &Ty::Var(0)));
        assert!(types_compatible(
            &Ty::Generic("List".into(), vec![Ty::Var(0)]),
            &Ty::Generic("List".into(), vec![Ty::Int])
        ));
    }
}
