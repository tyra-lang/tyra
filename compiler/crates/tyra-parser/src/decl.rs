// Declaration parsing: fn, value, data, type, trait, impl, import.
// spec reference: §8 (types), §9 (functions), §13 (modules)

use tyra_ast::*;
use tyra_diagnostics::Report;
use tyra_lexer::TokenKind;

use crate::stmt::parse_body;
use crate::token_stream::TokenStream;
use crate::type_expr::{parse_type, parse_type_params};

/// Parse a function definition (§9.1, §9.3, §14.2).
/// Parsed function header (name, type params, self, params, return type).
struct FnHeader {
    start: Span,
    name: String,
    type_params: Vec<TypeParam>,
    self_param: Option<SelfParam>,
    params: Vec<Param>,
    return_type: Option<TypeExpr>,
}

/// Parse the common parts of a function: name, type params, self, params, return type.
/// Consumes `fn`, name, `(params...)`, and optional `-> ReturnType`.
fn parse_fn_header(ts: &mut TokenStream, report: &mut Report) -> FnHeader {
    let start = ts.advance().span; // consume 'fn'
    let name = ts.expect_ident(report).unwrap_or_default();
    let type_params = parse_type_params(ts, report);

    ts.expect(&TokenKind::LParen, report);

    // Parse self parameter if present (§8.7)
    let self_param = if matches!(ts.peek(), TokenKind::Ident(s) if s == "self") {
        let sp = ts.advance().span;
        if ts.check(&TokenKind::Comma) {
            ts.advance();
        }
        Some(SelfParam { span: sp })
    } else {
        None
    };

    let params = parse_params(ts, report);
    ts.expect(&TokenKind::RParen, report);

    let return_type = if ts.check(&TokenKind::Arrow) {
        ts.advance();
        Some(parse_type(ts, report))
    } else {
        None
    };

    FnHeader {
        start,
        name,
        type_params,
        self_param,
        params,
        return_type,
    }
}

pub fn parse_fn_def(
    ts: &mut TokenStream,
    report: &mut Report,
    is_async: bool,
    is_export: bool,
) -> FnDef {
    let header = parse_fn_header(ts, report);

    ts.expect_newline_or_eof(report);
    let body = parse_body(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();

    FnDef {
        name: header.name,
        type_params: header.type_params,
        self_param: header.self_param,
        params: header.params,
        return_type: header.return_type,
        body,
        is_async,
        is_export,
        span: header.start.merge(end),
    }
}

/// Parse a lambda expression: `fn(params) -> T body end` (§9.4)
pub fn parse_lambda(ts: &mut TokenStream, report: &mut Report) -> LambdaExpr {
    let start = ts.advance().span; // consume 'fn'
    ts.expect(&TokenKind::LParen, report);
    let params = parse_params(ts, report);
    ts.expect(&TokenKind::RParen, report);

    let return_type = if ts.check(&TokenKind::Arrow) {
        ts.advance();
        Some(parse_type(ts, report))
    } else {
        None
    };

    ts.expect_newline_or_eof(report);
    let body = parse_body(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();

    LambdaExpr {
        params,
        return_type,
        body,
        span: start.merge(end),
    }
}

/// Parse `value Name ... end` (§8.6)
pub fn parse_value_def(ts: &mut TokenStream, report: &mut Report, is_export: bool) -> ValueDef {
    let start = ts.advance().span; // consume 'value'
    let name = ts.expect_ident(report).unwrap_or_default();
    let type_params = parse_type_params(ts, report);
    ts.expect_newline_or_eof(report);
    let fields = parse_field_defs(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    ValueDef {
        name,
        type_params,
        fields,
        is_export,
        span: start.merge(end),
    }
}

/// Parse `data Name ... end` (§8.6)
pub fn parse_data_def(ts: &mut TokenStream, report: &mut Report, is_export: bool) -> DataDef {
    let start = ts.advance().span; // consume 'data'
    let name = ts.expect_ident(report).unwrap_or_default();
    let type_params = parse_type_params(ts, report);
    ts.expect_newline_or_eof(report);
    let fields = parse_field_defs(ts, report);
    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    DataDef {
        name,
        type_params,
        fields,
        is_export,
        span: start.merge(end),
    }
}

/// Parse `type Name = Alias | type Name = | Variant(...)` (§8.5)
pub fn parse_type_def(ts: &mut TokenStream, report: &mut Report, is_export: bool) -> TypeDef {
    let start = ts.advance().span; // consume 'type'
    let name = ts.expect_ident(report).unwrap_or_default();
    let type_params = parse_type_params(ts, report);
    ts.expect(&TokenKind::Eq, report);

    let kind = if ts.check(&TokenKind::Pipe) || is_pipe_after_newline(ts) {
        // ADT: `| Variant1 | Variant2`
        ts.skip_newlines();
        let mut variants = Vec::new();
        while ts.eat(&TokenKind::Pipe) {
            variants.push(parse_variant(ts, report));
            ts.skip_newlines();
        }
        TypeDefKind::Adt(variants)
    } else {
        // Type alias
        let aliased = parse_type(ts, report);
        TypeDefKind::Alias(aliased)
    };

    let end = ts.peek_span();
    TypeDef {
        name,
        type_params,
        kind,
        is_export,
        span: start.merge(end),
    }
}

/// Parse `trait Name ... end` (§8.7)
pub fn parse_trait_def(ts: &mut TokenStream, report: &mut Report, is_export: bool) -> TraitDef {
    let start = ts.advance().span; // consume 'trait'
    let name = ts.expect_ident(report).unwrap_or_default();
    let type_params = parse_type_params(ts, report);
    ts.expect_newline_or_eof(report);
    ts.skip_newlines();

    let mut methods = Vec::new();
    while ts.check(&TokenKind::Fn) {
        methods.push(parse_fn_signature(ts, report));
        ts.skip_newlines();
    }

    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    TraitDef {
        name,
        type_params,
        methods,
        is_export,
        span: start.merge(end),
    }
}

/// Parse a function signature without body (for trait method declarations).
/// `fn name(self, params...) -> ReturnType`
fn parse_fn_signature(ts: &mut TokenStream, report: &mut Report) -> FnDef {
    let header = parse_fn_header(ts, report);

    // Signature only — no body, no end keyword
    ts.expect_newline_or_eof(report);
    let end = ts.peek_span();

    FnDef {
        name: header.name,
        type_params: header.type_params,
        self_param: header.self_param,
        params: header.params,
        return_type: header.return_type,
        body: vec![],
        is_async: false,
        is_export: false,
        span: header.start.merge(end),
    }
}

/// Parse `impl Trait<Args> for Type ... end` (§8.7)
pub fn parse_impl_def(ts: &mut TokenStream, report: &mut Report) -> ImplDef {
    let start = ts.advance().span; // consume 'impl'
    let trait_name = ts.expect_ident(report).unwrap_or_default();

    let trait_type_args = if ts.check(&TokenKind::Lt) {
        ts.advance();
        let args = parse_comma_separated_types(ts, report);
        ts.expect(&TokenKind::Gt, report);
        args
    } else {
        vec![]
    };

    ts.expect(&TokenKind::For, report);
    let target_type = parse_type(ts, report);
    ts.expect_newline_or_eof(report);
    ts.skip_newlines();

    let mut methods = Vec::new();
    while ts.check(&TokenKind::Fn) {
        methods.push(parse_fn_def(ts, report, false, false));
        ts.skip_newlines();
    }

    ts.expect(&TokenKind::End, report);
    let end = ts.peek_span();
    ImplDef {
        trait_name,
        trait_type_args,
        target_type,
        methods,
        span: start.merge(end),
    }
}

/// Parse `import a.b.c [as alias]` (§13.2)
pub fn parse_import(ts: &mut TokenStream, report: &mut Report) -> ImportDecl {
    let start = ts.advance().span; // consume 'import'
    let mut path = vec![ts.expect_ident(report).unwrap_or_default()];
    while ts.eat(&TokenKind::Dot) {
        path.push(ts.expect_ident(report).unwrap_or_default());
    }
    let alias = if matches!(ts.peek(), TokenKind::Ident(s) if s == "as") {
        ts.advance(); // consume 'as'
        Some(ts.expect_ident(report).unwrap_or_default())
    } else {
        None
    };
    let end = ts.peek_span();
    ts.expect_newline_or_eof(report);
    ImportDecl {
        path,
        alias,
        span: start.merge(end),
    }
}

/// Parse a `test "name" [panics] ... end` block (ADR 0013).
/// `test` and `panics` are contextual keywords — the caller has already
/// confirmed that the current token is `Ident("test")` followed by a string.
pub fn parse_test_def(ts: &mut TokenStream, report: &mut Report) -> TestDef {
    let start_span = ts.peek_span();
    ts.advance(); // consume Ident("test")

    // Consume the string name literal
    let name = match ts.peek().clone() {
        TokenKind::String(s) => {
            ts.advance();
            s
        }
        _ => {
            let tok = ts.peek_token().clone();
            report.add(
                tyra_diagnostics::Diagnostic::error("expected string literal after `test`")
                    .with_code("E0100")
                    .with_label(tyra_diagnostics::Label::new(tok.span, "expected string")),
            );
            String::new()
        }
    };

    // Optional `panics` modifier (contextual keyword — stays as Ident)
    let expects_panic = if matches!(ts.peek(), TokenKind::Ident(kw) if kw == "panics") {
        ts.advance();
        true
    } else {
        false
    };

    ts.expect_newline_or_eof(report);
    let body = parse_body(ts, report);
    let end_span = ts.peek_span();
    ts.expect(&TokenKind::End, report);

    TestDef {
        name,
        expects_panic,
        body,
        span: start_span.merge(end_span),
    }
}

// -- Internal helpers --

/// Parse function parameters: `_ x: Int, name: String, to target: Point`
fn parse_params(ts: &mut TokenStream, report: &mut Report) -> Vec<Param> {
    let mut params = Vec::new();
    while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
        params.push(parse_param(ts, report));
        if !ts.eat(&TokenKind::Comma) {
            break;
        }
    }
    params
}

/// Parse a single parameter.
fn parse_param(ts: &mut TokenStream, report: &mut Report) -> Param {
    let start = ts.peek_span();

    // `_ name: Type` — positional (label = None).
    // Accept contextual keywords (`value`, `data`, `type`, `trait`, `impl`)
    // as parameter names. They're reserved at the top level for
    // declarations but inside `(...)` we're unambiguously in parameter
    // position.
    if matches!(ts.peek(), TokenKind::Ident(s) if s == "_") {
        ts.advance();
        let name = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
        ts.expect(&TokenKind::Colon, report);
        let type_annotation = parse_type(ts, report);
        let end = type_annotation.span;
        return Param {
            label: None,
            name,
            type_annotation,
            span: start.merge(end),
        };
    }

    // First identifier
    let first = ts.expect_ident_or_field_keyword(report).unwrap_or_default();

    // Check if this is `label name: Type` (two idents before colon)
    if let TokenKind::Ident(_) = ts.peek() {
        let label = first;
        let name = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
        ts.expect(&TokenKind::Colon, report);
        let type_annotation = parse_type(ts, report);
        let end = type_annotation.span;
        return Param {
            label: Some(label),
            name,
            type_annotation,
            span: start.merge(end),
        };
    }

    // `name: Type` — label same as name
    ts.expect(&TokenKind::Colon, report);
    let type_annotation = parse_type(ts, report);
    let end = type_annotation.span;
    Param {
        label: Some(first.clone()),
        name: first,
        type_annotation,
        span: start.merge(end),
    }
}

/// Parse field definitions for value/data: `name: Type` or `mut name: Type`
fn parse_field_defs(ts: &mut TokenStream, report: &mut Report) -> Vec<FieldDef> {
    let mut fields = Vec::new();
    ts.skip_newlines();
    while !matches!(ts.peek(), TokenKind::End | TokenKind::Eof) {
        let start = ts.peek_span();
        let is_mut = ts.eat(&TokenKind::Mut);
        let name = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
        ts.expect(&TokenKind::Colon, report);
        let type_annotation = parse_type(ts, report);
        let end = type_annotation.span;
        fields.push(FieldDef {
            name,
            type_annotation,
            is_mut,
            span: start.merge(end),
        });
        ts.expect_newline_or_eof(report);
        ts.skip_newlines();
    }
    fields
}

/// Parse an ADT variant: `VariantName` or `VariantName(field: Type, ...)`
fn parse_variant(ts: &mut TokenStream, report: &mut Report) -> Variant {
    let start = ts.peek_span();
    let name = ts.expect_ident(report).unwrap_or_default();

    let fields = if ts.check(&TokenKind::LParen) {
        ts.advance();
        let mut fields = Vec::new();
        while !ts.check(&TokenKind::RParen) && !ts.at_eof() {
            let fstart = ts.peek_span();
            let fname = ts.expect_ident_or_field_keyword(report).unwrap_or_default();
            ts.expect(&TokenKind::Colon, report);
            let ftype = parse_type(ts, report);
            let fend = ftype.span;
            fields.push(FieldDef {
                name: fname,
                type_annotation: ftype,
                is_mut: false,
                span: fstart.merge(fend),
            });
            if !ts.eat(&TokenKind::Comma) {
                break;
            }
        }
        ts.expect(&TokenKind::RParen, report);
        fields
    } else {
        vec![]
    };

    let end = ts.peek_span();
    Variant {
        name,
        fields,
        span: start.merge(end),
    }
}

fn parse_comma_separated_types(ts: &mut TokenStream, report: &mut Report) -> Vec<TypeExpr> {
    let mut types = Vec::new();
    loop {
        types.push(parse_type(ts, report));
        if !ts.eat(&TokenKind::Comma) {
            break;
        }
    }
    types
}

/// Check if the next non-newline token is `|` (for ADT detection).
/// Handles `type Payment = \n  | Card(...) | Cash`.
fn is_pipe_after_newline(ts: &TokenStream) -> bool {
    matches!(ts.peek_past_newlines(), TokenKind::Pipe)
}
