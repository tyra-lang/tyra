// tyra-parser: Parse tokens into AST.
// spec reference: §6-§14

pub mod decl;
pub mod expr;
pub mod pattern;
pub mod stmt;
pub mod token_stream;
pub mod type_expr;

use tyra_ast::*;
use tyra_diagnostics::{Report, SourceId, SourceMap};
use tyra_lexer::TokenKind;

use token_stream::TokenStream;

/// Parse a source file into an AST.
pub fn parse(source_id: SourceId, sources: &SourceMap, report: &mut Report) -> SourceFile {
    let tokens = tyra_lexer::tokenize(source_id, sources, report);
    let mut ts = TokenStream::new(tokens);
    let start = ts.peek_span();

    let mut items = Vec::new();
    ts.skip_newlines();

    while !ts.at_eof() {
        let before = ts.position();
        let item = parse_item(&mut ts, report);
        items.push(item);
        // Safety: ensure we always make progress to prevent infinite loops
        if ts.position() == before {
            ts.advance();
        }
        ts.skip_newlines();
    }

    let end = ts.peek_span();
    SourceFile {
        items,
        span: start.merge(end),
    }
}

/// Parse a single top-level item: declaration or executable statement.
fn parse_item(ts: &mut TokenStream, report: &mut Report) -> Item {
    let start_span = ts.peek_span();

    // Check for `export` modifier
    let is_export = ts.eat(&TokenKind::Export);

    // Check for `async` modifier (only before `fn`)
    let is_async = ts.eat(&TokenKind::Async);

    // Validate: `async` is only valid before `fn` (§14.2)
    if is_async && !matches!(ts.peek(), TokenKind::Fn) {
        report.add(
            tyra_diagnostics::Diagnostic::error("`async` can only be applied to `fn`")
                .with_code("E0108")
                .with_label(tyra_diagnostics::Label::new(
                    start_span,
                    "`async` used here",
                )),
        );
    }

    match ts.peek() {
        TokenKind::Fn => {
            let func = decl::parse_fn_def(ts, report, is_async, is_export);
            ts.skip_newlines();
            Item::FnDef(func)
        }
        TokenKind::Value => {
            let val = decl::parse_value_def(ts, report, is_export);
            ts.skip_newlines();
            Item::ValueDef(val)
        }
        TokenKind::Data => {
            let data = decl::parse_data_def(ts, report, is_export);
            ts.skip_newlines();
            Item::DataDef(data)
        }
        TokenKind::Type => {
            let typedef = decl::parse_type_def(ts, report, is_export);
            ts.skip_newlines();
            Item::TypeDef(typedef)
        }
        TokenKind::Trait => {
            let traitdef = decl::parse_trait_def(ts, report, is_export);
            ts.skip_newlines();
            Item::TraitDef(traitdef)
        }
        TokenKind::Impl => {
            // Validate: `export` is not valid on `impl` (§13.3)
            if is_export {
                report.add(
                    tyra_diagnostics::Diagnostic::error("`export` cannot be applied to `impl`")
                        .with_code("E0109")
                        .with_label(tyra_diagnostics::Label::new(
                            start_span,
                            "`export` used here",
                        )),
                );
            }
            let impldef = decl::parse_impl_def(ts, report);
            ts.skip_newlines();
            Item::ImplDef(impldef)
        }
        TokenKind::Import => {
            if is_export {
                report.add(
                    tyra_diagnostics::Diagnostic::error("`export` cannot be applied to `import`")
                        .with_code("E0109")
                        .with_label(tyra_diagnostics::Label::new(
                            start_span,
                            "`export` used here",
                        )),
                );
            }
            let import = decl::parse_import(ts, report);
            ts.skip_newlines();
            Item::Import(import)
        }
        _ => {
            // Executable statement (§6.1)
            if is_export {
                report.add(
                    tyra_diagnostics::Diagnostic::error(
                        "`export` cannot be applied to a statement",
                    )
                    .with_code("E0109")
                    .with_label(tyra_diagnostics::Label::new(
                        start_span,
                        "`export` used here",
                    )),
                );
            }
            let s = stmt::parse_stmt(ts, report);
            ts.skip_newlines();
            Item::Stmt(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_ast::*;

    fn parse_str(source: &str) -> (SourceFile, Report) {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let ast = parse(id, &sources, &mut report);
        (ast, report)
    }

    #[test]
    fn parse_hello_world() {
        let (ast, report) = parse_str("print(\"hello, tyra\")\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        assert_eq!(ast.items.len(), 1);
        assert!(matches!(ast.items[0], Item::Stmt(_)));
    }

    #[test]
    fn parse_fn_def() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        assert_eq!(ast.items.len(), 1);
        if let Item::FnDef(f) = &ast.items[0] {
            assert_eq!(f.name, "add");
            assert_eq!(f.params.len(), 2);
            assert!(f.params[0].label.is_none()); // positional (_)
            assert_eq!(f.params[0].name, "x");
        } else {
            panic!("expected FnDef");
        }
    }

    #[test]
    fn parse_let_binding() {
        let (ast, report) = parse_str("let x = 42\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Let(s)) = &ast.items[0] {
            assert_eq!(s.name, "x");
            assert!(matches!(s.value.kind, ExprKind::IntLit(42)));
        } else {
            panic!("expected Let");
        }
    }

    #[test]
    fn parse_mut_binding_with_type() {
        let (ast, report) = parse_str("mut count: Int = 0\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Mut(s)) = &ast.items[0] {
            assert_eq!(s.name, "count");
            assert!(s.type_annotation.is_some());
        } else {
            panic!("expected Mut");
        }
    }

    #[test]
    fn parse_if_else() {
        let source = "if true\n  1\nelse\n  2\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(s)) = &ast.items[0] {
            assert!(matches!(s.expr.kind, ExprKind::If(_)));
        } else {
            panic!("expected If expr");
        }
    }

    #[test]
    fn parse_else_if_chain() {
        let source = "if x > 0\n  1\nelse if x < 0\n  2\nelse\n  0\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::If(if_expr) = &expr.kind {
                assert!(matches!(if_expr.else_body, Some(ElseBranch::ElseIf(_))));
            } else {
                panic!("expected If");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_match() {
        let source = "match x\nwhen 0\n  1\nwhen _\n  2\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::Match(m) = &expr.kind {
                assert_eq!(m.arms.len(), 2);
                assert!(matches!(m.arms[0].pattern.kind, PatternKind::IntLit(0)));
                assert!(matches!(m.arms[1].pattern.kind, PatternKind::Wildcard));
            } else {
                panic!("expected Match");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_constructor_pattern() {
        let source = "match r\nwhen Ok(v)\n  v\nwhen Err(e)\n  0\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::Match(m) = &expr.kind {
                assert!(matches!(
                    m.arms[0].pattern.kind,
                    PatternKind::Constructor(ref name, _) if name == "Ok"
                ));
            } else {
                panic!("expected Match");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_value_def() {
        let source = "value Point\n  x: Float\n  y: Float\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::ValueDef(v) = &ast.items[0] {
            assert_eq!(v.name, "Point");
            assert_eq!(v.fields.len(), 2);
            assert!(!v.fields[0].is_mut);
        } else {
            panic!("expected ValueDef");
        }
    }

    #[test]
    fn parse_data_def_with_mut() {
        let source = "data User\n  id: Int\n  mut name: String\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::DataDef(d) = &ast.items[0] {
            assert_eq!(d.name, "User");
            assert_eq!(d.fields.len(), 2);
            assert!(!d.fields[0].is_mut);
            assert!(d.fields[1].is_mut);
        } else {
            panic!("expected DataDef");
        }
    }

    #[test]
    fn parse_type_alias() {
        let source = "type UserId = Int\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::TypeDef(t) = &ast.items[0] {
            assert_eq!(t.name, "UserId");
            assert!(matches!(t.kind, TypeDefKind::Alias(_)));
        } else {
            panic!("expected TypeDef");
        }
    }

    #[test]
    fn parse_adt() {
        let source = "type Color =\n  | Red\n  | Green\n  | Blue\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::TypeDef(t) = &ast.items[0] {
            if let TypeDefKind::Adt(variants) = &t.kind {
                assert_eq!(variants.len(), 3);
                assert_eq!(variants[0].name, "Red");
            } else {
                panic!("expected ADT");
            }
        } else {
            panic!("expected TypeDef");
        }
    }

    #[test]
    fn parse_adt_with_fields() {
        let source = "type Payment =\n  | Card(last4: String)\n  | Cash\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::TypeDef(t) = &ast.items[0] {
            if let TypeDefKind::Adt(variants) = &t.kind {
                assert_eq!(variants[0].name, "Card");
                assert_eq!(variants[0].fields.len(), 1);
                assert_eq!(variants[1].name, "Cash");
                assert!(variants[1].fields.is_empty());
            } else {
                panic!("expected ADT");
            }
        } else {
            panic!("expected TypeDef");
        }
    }

    #[test]
    fn parse_import() {
        let source = "import http.server\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Import(i) = &ast.items[0] {
            assert_eq!(i.path, vec!["http", "server"]);
            assert!(i.alias.is_none());
        } else {
            panic!("expected Import");
        }
    }

    #[test]
    fn parse_import_as() {
        let source = "import app.user_repo as repo\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Import(i) = &ast.items[0] {
            assert_eq!(i.alias.as_deref(), Some("repo"));
        } else {
            panic!("expected Import");
        }
    }

    #[test]
    fn parse_turbofish() {
        let (ast, report) = parse_str("parse::<Int>(text)\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            assert!(matches!(expr.kind, ExprKind::TurbofishCall(_, _, _)));
        } else {
            panic!("expected TurbofishCall");
        }
    }

    #[test]
    fn parse_binary_ops_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3)
        let (ast, report) = parse_str("1 + 2 * 3\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::BinaryOp(left, BinOp::Add, right) = &expr.kind {
                assert!(matches!(left.kind, ExprKind::IntLit(1)));
                assert!(matches!(right.kind, ExprKind::BinaryOp(_, BinOp::Mul, _)));
            } else {
                panic!("expected Add at top level");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_method_call_chain() {
        let (ast, report) = parse_str("a.b.c()\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            // Should be Call(FieldAccess(FieldAccess(a, b), c), [])
            assert!(matches!(expr.kind, ExprKind::Call(_, _)));
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_propagation() {
        let (ast, report) = parse_str("f()?\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            assert!(matches!(expr.kind, ExprKind::Propagate(_)));
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_for_loop() {
        let source = "for item in items\n  print(item)\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::For(f) = &expr.kind {
                assert_eq!(f.binding, "item");
            } else {
                panic!("expected For");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_list_literal() {
        let (ast, report) = parse_str("[1, 2, 3]\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::Stmt(Stmt::Expr(ExprStmt { expr, .. })) = &ast.items[0] {
            if let ExprKind::ListLit(items) = &expr.kind {
                assert_eq!(items.len(), 3);
            } else {
                panic!("expected ListLit");
            }
        } else {
            panic!("expected Expr");
        }
    }

    #[test]
    fn parse_full_program() {
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

print("hello")
"#;
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        assert_eq!(ast.items.len(), 2);
        assert!(matches!(ast.items[0], Item::FnDef(_)));
        assert!(matches!(ast.items[1], Item::Stmt(_)));
    }

    #[test]
    fn parse_trait_def_signature_only() {
        let source = "trait Stringable\n  fn to_string(self) -> String\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        if let Item::TraitDef(t) = &ast.items[0] {
            assert_eq!(t.name, "Stringable");
            assert_eq!(t.methods.len(), 1);
            assert_eq!(t.methods[0].name, "to_string");
            assert!(t.methods[0].self_param.is_some());
            assert!(t.methods[0].body.is_empty());
        } else {
            panic!("expected TraitDef");
        }
    }

    #[test]
    fn parse_trait_followed_by_impl() {
        let source = "trait Summable\n  fn sum(self) -> Int\nend\nimpl Summable for Pair\n  fn sum(self) -> Int\n    self.first + self.second\n  end\nend\n";
        let (ast, report) = parse_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
        assert_eq!(ast.items.len(), 2);
        assert!(matches!(ast.items[0], Item::TraitDef(_)));
        assert!(matches!(ast.items[1], Item::ImplDef(_)));
    }

    #[test]
    fn keyword_as_field_name_in_adt_variant() {
        // `value` keyword should be accepted as a field name in ADT variants
        let (ast, report) = parse_str("type Err =\n  | Bad(value: String)\n");
        assert!(
            !report.has_errors(),
            "keyword 'value' should be accepted as field name: {:?}",
            report.diagnostics()
        );
        assert!(matches!(ast.items[0], Item::TypeDef(_)));
    }

    #[test]
    fn keyword_as_field_name_in_value_def() {
        // `type` keyword should be accepted as a field name in value types
        let (ast, report) = parse_str("value Config\n  type: String\nend\n");
        assert!(
            !report.has_errors(),
            "keyword 'type' should be accepted as field name: {:?}",
            report.diagnostics()
        );
        assert!(matches!(ast.items[0], Item::ValueDef(_)));
    }

    #[test]
    fn malformed_input_does_not_panic_on_eof_overrun() {
        // Rust-flavored source fed to the Tyra parser used to panic in
        // token_stream with index-out-of-bounds at peek/advance after
        // the cursor overran the trailing Eof token. The parser must
        // instead emit a diagnostic and return. Regression for the
        // ai-gen benchmark (bench/ai-gen) where the model frequently
        // produces garbage-by-our-standards syntax.
        let garbled = "fn main() {\n    let mut input = String::new();\n}\n";
        let (_ast, report) = parse_str(garbled);
        assert!(
            report.has_errors(),
            "malformed input should produce diagnostics, not parse clean"
        );
    }

    #[test]
    fn unclosed_bracket_does_not_panic() {
        // Unmatched opening bracket leaves bracket_depth > 0, which in
        // the old token_stream code caused peek_skip_newlines to walk
        // past Eof and index-out-of-bounds panic.
        let (_ast, report) = parse_str("let xs = [1, 2, 3\n");
        assert!(report.has_errors());
    }
}
