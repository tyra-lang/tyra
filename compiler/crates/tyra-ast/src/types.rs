// AST node types for the Tyra language.
//
// Design:
// - Every node carries a Span for diagnostics
// - Nodes are plain data (no methods beyond construction)
// - Box is used for recursive types to keep enum sizes small
// - String is used for identifiers (interning can be added later)
//
// spec reference: §6-§14

use tyra_diagnostics::Span;

// ============================================================================
// Top-level: Source file
// ============================================================================

/// A complete source file — the root of the AST.
/// Contains a mix of declarations and executable statements (§6.1).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceFile {
    pub items: Vec<Item>,
    pub span: Span,
}

/// A top-level item: either a declaration or an executable statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Function definition (§9.1): `fn name(...) -> T ... end`
    FnDef(FnDef),
    /// Value type definition (§8.6): `value Name ... end`
    ValueDef(ValueDef),
    /// Data type definition (§8.6): `data Name ... end`
    DataDef(DataDef),
    /// Type alias or ADT (§8.5): `type Name = ...`
    TypeDef(TypeDef),
    /// Trait definition (§8.7): `trait Name ... end`
    TraitDef(TraitDef),
    /// Trait implementation (§8.7): `impl Trait for Type ... end`
    ImplDef(ImplDef),
    /// Import (§13.2): `import a.b.c`
    Import(ImportDecl),
    /// Executable statement at top level (§6.1)
    Stmt(Stmt),
}

// ============================================================================
// Declarations
// ============================================================================

/// Function definition (§9.1, §9.3, §9.4, §14.2).
#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    /// `self` receiver for trait methods (§8.7). None for free functions.
    /// For `value` types, `self` is passed by value; for `data`, by reference.
    /// This distinction is a type-checker concern, not an AST concern.
    pub self_param: Option<SelfParam>,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Stmt>,
    pub is_async: bool,
    pub is_export: bool,
    pub span: Span,
}

/// The `self` parameter in a trait method (§8.7).
#[derive(Debug, Clone, PartialEq)]
pub struct SelfParam {
    pub span: Span,
}

/// A function parameter with optional external label (§9.3).
/// `_ x: Int` -> label=None, name="x"
/// `name: String` -> label=Some("name"), name="name"
/// `to target: Point` -> label=Some("to"), name="target"
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub label: Option<String>,
    pub name: String,
    pub type_annotation: TypeExpr,
    pub span: Span,
}

/// A type parameter with optional constraints (§8.4).
/// `T` -> name="T", constraints=[]
/// `T: Eq` -> name="T", constraints=["Eq"]
/// `T: Eq + Hash` -> name="T", constraints=["Eq", "Hash"]
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParam {
    pub name: String,
    pub constraints: Vec<TypeExpr>,
    pub span: Span,
}

/// `value Name ... end` (§8.6)
#[derive(Debug, Clone, PartialEq)]
pub struct ValueDef {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<FieldDef>,
    pub is_export: bool,
    pub span: Span,
}

/// `data Name ... end` (§8.6)
#[derive(Debug, Clone, PartialEq)]
pub struct DataDef {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<FieldDef>,
    pub is_export: bool,
    pub span: Span,
}

/// A field in a value or data type.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub type_annotation: TypeExpr,
    pub is_mut: bool,
    pub span: Span,
}

/// `type Name = ...` — type alias or ADT (§8.5).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub kind: TypeDefKind,
    pub is_export: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDefKind {
    /// Type alias: `type UserId = Int`
    Alias(TypeExpr),
    /// ADT: `type Payment = | Card(last4: String) | Cash`
    Adt(Vec<Variant>),
}

/// An ADT variant (§8.5): `| Card(last4: String)` or `| Cash`
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub span: Span,
}

/// `trait Name ... end` (§8.7)
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDef {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub methods: Vec<FnDef>,
    pub is_export: bool,
    pub span: Span,
}

/// `impl Trait for Type ... end` (§8.7)
#[derive(Debug, Clone, PartialEq)]
pub struct ImplDef {
    pub trait_name: String,
    pub trait_type_args: Vec<TypeExpr>,
    pub target_type: TypeExpr,
    pub methods: Vec<FnDef>,
    pub span: Span,
}

/// `import a.b.c` or `import a.b.c as alias` (§13.2)
#[derive(Debug, Clone, PartialEq)]
pub struct ImportDecl {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub span: Span,
}

// ============================================================================
// Statements
// ============================================================================

/// A statement in a function body or top-level executable context.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `let x = expr` or `let x: T = expr`
    Let(LetStmt),
    /// `mut x = expr` or `mut x: T = expr`
    Mut(MutStmt),
    /// `return` or `return expr`
    Return(ReturnStmt),
    /// `defer expr`
    Defer(DeferStmt),
    /// Expression used as a statement (value discarded)
    Expr(ExprStmt),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    pub name: String,
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MutStmt {
    pub name: String,
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeferStmt {
    pub expr: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprStmt {
    pub expr: Expr,
    pub span: Span,
}

// ============================================================================
// Expressions
// ============================================================================

/// An expression node. Box is used for recursive variants.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    // -- Literals --
    /// Integer literal: `42`
    IntLit(i64),
    /// Float literal: `3.14`
    FloatLit(f64),
    /// String literal: `"hello"` (after escape processing, before interpolation).
    /// Raw strings `r"..."` (§7.3) are also represented as `StringLit` — the lexer
    /// handles the distinction. If round-trip formatting fidelity is needed in the
    /// future, a separate `RawStringLit` variant can be added.
    StringLit(String),
    /// String with interpolation segments: `"hello, #{name}!"`
    StringInterp(Vec<StringPart>),
    /// Boolean literal: `true` / `false`
    BoolLit(bool),
    /// Unit literal: `()`
    UnitLit,
    /// List literal: `[1, 2, 3]` (§11)
    ListLit(Vec<Expr>),
    /// Map literal: `{k: v, ...}` (§11)
    MapLit(Vec<(Expr, Expr)>),

    // -- Identifiers and paths --
    /// Simple identifier: `x`, `User`
    Ident(String),
    /// Qualified path: `Color.Red`, `server.Response` (§8.5)
    FieldAccess(Box<Expr>, String),

    // -- Operations --
    /// Binary operation: `a + b`, `x == y`, `p and q`
    BinaryOp(Box<Expr>, BinOp, Box<Expr>),
    /// Unary operation: `-x`, `not p`
    UnaryOp(UnaryOp, Box<Expr>),
    /// Assignment: `user.name = "new"` (§8.6 field update)
    Assign(Box<Expr>, Box<Expr>),

    // -- Calls --
    /// Function call: `add(1, 2)`, `create_user(name: "mika")` (§9.2)
    Call(Box<Expr>, Vec<Arg>),
    /// Turbofish call: `parse::<Int>(text)` (§8.4)
    TurbofishCall(Box<Expr>, Vec<TypeExpr>, Vec<Arg>),
    /// Index: `items[0]` (§11)
    Index(Box<Expr>, Box<Expr>),

    // -- Postfix --
    /// Propagation: `expr?` (§12.2)
    Propagate(Box<Expr>),
    /// Await: `expr.await` (§14.3)
    Await(Box<Expr>),

    // -- Control flow (expression position) --
    /// `if cond ... else ... end` (§10.2)
    If(Box<IfExpr>),
    /// `match expr when ... end` (§10.3)
    Match(Box<MatchExpr>),

    // -- Control flow (statement position, but syntactically expressions) --
    /// `for item in iter ... end` (§10.5)
    For(Box<ForExpr>),
    /// `while cond ... end` (§10.4)
    While(Box<WhileExpr>),

    // -- Functions --
    /// Anonymous function: `fn(x: Int) -> Int ... end` (§9.4)
    Lambda(Box<LambdaExpr>),

    // -- Spawn --
    /// `spawn f(args)` (§14.4)
    Spawn(Box<Expr>),
}

/// A segment of a string with interpolation.
#[derive(Debug, Clone, PartialEq)]
pub enum StringPart {
    /// Literal text segment
    Lit(String),
    /// Interpolated expression: `#{expr}`
    Expr(Expr),
}

/// A function call argument, optionally labeled (§9.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    pub label: Option<String>,
    pub value: Expr,
    pub span: Span,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    // Arithmetic
    Add, // +
    Sub, // -
    Mul, // *
    Div, // /
    // Comparison
    Eq,    // ==
    NotEq, // !=
    Lt,    // <
    LtEq,  // <=
    Gt,    // >
    GtEq,  // >=
    RefEq, // ===
    // Logical (§10.1)
    And, // and
    Or,  // or
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `not x` (§10.1)
    Not,
}

// ============================================================================
// Control flow expressions
// ============================================================================

/// `if cond body [else if ... else ...] end` (§10.2)
#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    pub condition: Expr,
    pub then_body: Vec<Stmt>,
    /// `else if` chains and final `else` are represented as nested IfExpr
    /// in the else_body. A final `else` has a single-element Vec of Stmts.
    pub else_body: Option<ElseBranch>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElseBranch {
    /// `else ... end`
    Else(Vec<Stmt>),
    /// `else if ...` (chained, no separate end)
    ElseIf(Box<IfExpr>),
}

/// `match expr when ... end` (§10.3)
#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    pub subject: Expr,
    pub arms: Vec<MatchArm>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `for item in iter body end` (§10.5)
#[derive(Debug, Clone, PartialEq)]
pub struct ForExpr {
    pub binding: String,
    pub iter: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// `while cond body end` (§10.4)
#[derive(Debug, Clone, PartialEq)]
pub struct WhileExpr {
    pub condition: Expr,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// Anonymous function expression (§9.4)
#[derive(Debug, Clone, PartialEq)]
pub struct LambdaExpr {
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

// ============================================================================
// Patterns (§10.3)
// ============================================================================

/// A pattern in a `match` arm.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// Wildcard: `_`
    Wildcard,
    /// Identifier binding: `value`, `name`
    Ident(String),
    /// Integer literal: `0`, `1`
    IntLit(i64),
    /// Float literal (rare in patterns but syntactically possible)
    FloatLit(f64),
    /// String literal: `"hello"`
    StringLit(String),
    /// Boolean literal: `true`, `false`
    BoolLit(bool),
    /// Constructor pattern: `Ok(value)`, `Card(last4: last4)` (§8.5).
    /// The name is unqualified in match patterns.
    /// Nested patterns like `Err(Json(inner: MissingKey(key: k)))` are
    /// handled by nesting Constructor patterns within PatternField.
    Constructor(String, Vec<PatternField>),
}

/// A field in a constructor pattern: `last4: last4` or shorthand `last4`.
/// The spec (§8.5) says `when Card(last4)` is shorthand for `when Card(last4: last4)`.
/// The parser is responsible for desugaring this shorthand; the AST always contains
/// the explicit `field_name: pattern` form.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternField {
    pub field_name: String,
    pub pattern: Pattern,
    pub span: Span,
}

// ============================================================================
// Type expressions (§8)
// ============================================================================

/// A type annotation in source code.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    /// Simple named type: `Int`, `String`, `User`
    Named(String),
    /// Generic type application: `List<Int>`, `Result<T, E>`, `Map<K, V>`
    Generic(String, Vec<TypeExpr>),
    /// Function type: `fn(Int, Int) -> Bool` (§9.4)
    Fn(Vec<TypeExpr>, Box<TypeExpr>),
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    fn test_span() -> Span {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), "".into());
        Span::new(id, 0, 0)
    }

    #[test]
    fn build_simple_fn_def() {
        let span = test_span();
        let func = FnDef {
            name: "add".into(),
            type_params: vec![],
            self_param: None,
            params: vec![
                Param {
                    label: None,
                    name: "x".into(),
                    type_annotation: TypeExpr {
                        kind: TypeExprKind::Named("Int".into()),
                        span,
                    },
                    span,
                },
                Param {
                    label: None,
                    name: "y".into(),
                    type_annotation: TypeExpr {
                        kind: TypeExprKind::Named("Int".into()),
                        span,
                    },
                    span,
                },
            ],
            return_type: Some(TypeExpr {
                kind: TypeExprKind::Named("Int".into()),
                span,
            }),
            body: vec![Stmt::Expr(ExprStmt {
                expr: Expr {
                    kind: ExprKind::BinaryOp(
                        Box::new(Expr {
                            kind: ExprKind::Ident("x".into()),
                            span,
                        }),
                        BinOp::Add,
                        Box::new(Expr {
                            kind: ExprKind::Ident("y".into()),
                            span,
                        }),
                    ),
                    span,
                },
                span,
            })],
            is_async: false,
            is_export: false,
            span,
        };
        assert_eq!(func.name, "add");
        assert_eq!(func.params.len(), 2);
        assert!(!func.is_async);
    }

    #[test]
    fn build_adt() {
        let span = test_span();
        let adt = TypeDef {
            name: "Payment".into(),
            type_params: vec![],
            kind: TypeDefKind::Adt(vec![
                Variant {
                    name: "Card".into(),
                    fields: vec![FieldDef {
                        name: "last4".into(),
                        type_annotation: TypeExpr {
                            kind: TypeExprKind::Named("String".into()),
                            span,
                        },
                        is_mut: false,
                        span,
                    }],
                    span,
                },
                Variant {
                    name: "Cash".into(),
                    fields: vec![],
                    span,
                },
            ]),
            is_export: false,
            span,
        };
        assert_eq!(adt.name, "Payment");
        if let TypeDefKind::Adt(variants) = &adt.kind {
            assert_eq!(variants.len(), 2);
            assert_eq!(variants[0].name, "Card");
            assert_eq!(variants[0].fields.len(), 1);
            assert_eq!(variants[1].name, "Cash");
            assert!(variants[1].fields.is_empty());
        } else {
            panic!("expected ADT");
        }
    }

    #[test]
    fn build_match_expr() {
        let span = test_span();
        let m = MatchExpr {
            subject: Expr {
                kind: ExprKind::Ident("result".into()),
                span,
            },
            arms: vec![
                MatchArm {
                    pattern: Pattern {
                        kind: PatternKind::Constructor(
                            "Ok".into(),
                            vec![PatternField {
                                field_name: "value".into(),
                                pattern: Pattern {
                                    kind: PatternKind::Ident("v".into()),
                                    span,
                                },
                                span,
                            }],
                        ),
                        span,
                    },
                    body: vec![],
                    span,
                },
                MatchArm {
                    pattern: Pattern {
                        kind: PatternKind::Constructor("Err".into(), vec![]),
                        span,
                    },
                    body: vec![],
                    span,
                },
            ],
            span,
        };
        assert_eq!(m.arms.len(), 2);
    }

    #[test]
    fn build_if_else_if() {
        let span = test_span();
        let if_expr = IfExpr {
            condition: Expr {
                kind: ExprKind::BoolLit(true),
                span,
            },
            then_body: vec![],
            else_body: Some(ElseBranch::ElseIf(Box::new(IfExpr {
                condition: Expr {
                    kind: ExprKind::BoolLit(false),
                    span,
                },
                then_body: vec![],
                else_body: Some(ElseBranch::Else(vec![])),
                span,
            }))),
            span,
        };
        assert!(matches!(if_expr.else_body, Some(ElseBranch::ElseIf(_))));
    }

    #[test]
    fn build_value_def() {
        let span = test_span();
        let v = ValueDef {
            name: "Point".into(),
            type_params: vec![],
            fields: vec![
                FieldDef {
                    name: "x".into(),
                    type_annotation: TypeExpr {
                        kind: TypeExprKind::Named("Float".into()),
                        span,
                    },
                    is_mut: false,
                    span,
                },
                FieldDef {
                    name: "y".into(),
                    type_annotation: TypeExpr {
                        kind: TypeExprKind::Named("Float".into()),
                        span,
                    },
                    is_mut: false,
                    span,
                },
            ],
            is_export: false,
            span,
        };
        assert_eq!(v.name, "Point");
        assert_eq!(v.fields.len(), 2);
        assert!(!v.fields[0].is_mut);
    }

    #[test]
    fn build_generic_type() {
        let span = test_span();
        let t = TypeExpr {
            kind: TypeExprKind::Generic(
                "Result".into(),
                vec![
                    TypeExpr {
                        kind: TypeExprKind::Named("User".into()),
                        span,
                    },
                    TypeExpr {
                        kind: TypeExprKind::Named("AppError".into()),
                        span,
                    },
                ],
            ),
            span,
        };
        if let TypeExprKind::Generic(name, args) = &t.kind {
            assert_eq!(name, "Result");
            assert_eq!(args.len(), 2);
        } else {
            panic!("expected Generic");
        }
    }

    #[test]
    fn build_import() {
        let span = test_span();
        let import = ImportDecl {
            path: vec!["http".into(), "server".into()],
            alias: None,
            span,
        };
        assert_eq!(import.path, vec!["http", "server"]);

        let aliased = ImportDecl {
            path: vec!["app".into(), "user_repo".into()],
            alias: Some("repo".into()),
            span,
        };
        assert_eq!(aliased.alias.as_deref(), Some("repo"));
    }

    #[test]
    fn pattern_wildcard() {
        let span = test_span();
        let p = Pattern {
            kind: PatternKind::Wildcard,
            span,
        };
        assert!(matches!(p.kind, PatternKind::Wildcard));
    }
}
