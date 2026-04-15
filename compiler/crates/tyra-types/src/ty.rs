// Internal type representation for the Tyra type checker.
// spec reference: §7.2 (primitives), §8 (type system), §9.4 (function types)

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

/// Check if two types are compatible (assignable).
/// Never is compatible with everything (bottom type).
/// Error is compatible with everything (to suppress cascading errors).
pub fn types_compatible(expected: &Ty, actual: &Ty) -> bool {
    if actual.is_never() || actual.is_error() || expected.is_error() {
        return true;
    }
    expected == actual
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
}
