use tower_lsp::lsp_types::{ParameterInformation, ParameterLabel, SignatureInformation};
use tyra_ast::{FnDef, TypeExprKind};

pub(crate) struct ActiveCall<'a> {
    pub callee: &'a str,
    pub active_parameter: u32,
}

/// Scan `text[..offset]` forward, tracking bracket nesting and string/comment
/// state, to find the innermost open function-call paren enclosing the cursor.
/// Returns the function name and the 0-based index of the active argument.
pub(crate) fn find_active_call(text: &str, offset: usize) -> Option<ActiveCall<'_>> {
    let bytes = text.as_bytes();
    let limit = offset.min(bytes.len());

    // Stack entry: paren_pos==usize::MAX for non-call brackets `[]` `{}`.
    struct Frame {
        paren_pos: usize,
        comma_count: u32,
    }

    enum St {
        Normal,
        InString,
        InLineComment,
        InBlockComment,
    }

    let mut st = St::Normal;
    let mut stack: Vec<Frame> = Vec::new();
    let mut j = 0usize;

    while j < limit {
        match st {
            St::InLineComment => {
                if bytes[j] == b'\n' {
                    st = St::Normal;
                }
                j += 1;
            }
            St::InBlockComment => {
                if j + 1 < bytes.len() && bytes[j] == b'*' && bytes[j + 1] == b')' {
                    st = St::Normal;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            St::InString => {
                if bytes[j] == b'\\' {
                    j += 2; // skip escaped char
                } else if bytes[j] == b'"' {
                    st = St::Normal;
                    j += 1;
                } else {
                    j += 1;
                }
            }
            St::Normal => match bytes[j] {
                b'#' => {
                    st = St::InLineComment;
                    j += 1;
                }
                b'(' if j + 1 < bytes.len() && bytes[j + 1] == b'*' => {
                    st = St::InBlockComment;
                    j += 2;
                }
                b'"' => {
                    st = St::InString;
                    j += 1;
                }
                b'(' => {
                    stack.push(Frame {
                        paren_pos: j,
                        comma_count: 0,
                    });
                    j += 1;
                }
                b'[' | b'{' => {
                    stack.push(Frame {
                        paren_pos: usize::MAX,
                        comma_count: 0,
                    });
                    j += 1;
                }
                b')' | b']' | b'}' => {
                    stack.pop();
                    j += 1;
                }
                b',' => {
                    if let Some(frame) = stack.last_mut() {
                        if frame.paren_pos != usize::MAX {
                            frame.comma_count += 1;
                        }
                    }
                    j += 1;
                }
                _ => {
                    j += 1;
                }
            },
        }
    }

    // Find innermost real-paren frame.
    let frame = stack.iter().rev().find(|f| f.paren_pos != usize::MAX)?;
    let paren_pos = frame.paren_pos;
    let active_parameter = frame.comma_count;

    // Extract the identifier immediately before the `(`.
    let mut callee_end = paren_pos;
    // Skip trailing whitespace between identifier and `(`.
    while callee_end > 0 && bytes[callee_end - 1] == b' ' {
        callee_end -= 1;
    }
    let mut callee_start = callee_end;
    while callee_start > 0 {
        let c = bytes[callee_start - 1];
        if c.is_ascii_alphanumeric() || c == b'_' {
            callee_start -= 1;
        } else {
            break;
        }
    }
    if callee_start == callee_end {
        return None;
    }
    let callee = &text[callee_start..callee_end];
    // Reject numeric literals like `1(...)`.
    if callee.as_bytes()[0].is_ascii_digit() {
        return None;
    }

    Some(ActiveCall {
        callee,
        active_parameter,
    })
}

/// Build an LSP `SignatureInformation` from a user-defined function definition.
pub(crate) fn build_signature_for_fn(f: &FnDef) -> SignatureInformation {
    let param_strs: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_expr_name(&p.type_annotation.kind)))
        .collect();

    let ret = f
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", type_expr_name(&t.kind)))
        .unwrap_or_default();

    let label = format!("fn {}({}){}", f.name, param_strs.join(", "), ret);

    let parameters: Vec<ParameterInformation> = param_strs
        .into_iter()
        .map(|s| ParameterInformation {
            label: ParameterLabel::Simple(s),
            documentation: None,
        })
        .collect();

    SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: None,
    }
}

/// Look up a hardcoded signature for a small set of prelude functions.
pub(crate) fn prelude_signature(name: &str) -> Option<SignatureInformation> {
    static TABLE: &[(&str, &str, &[&str])] = &[
        (
            "print",
            "fn print(value: String) -> Unit",
            &["value: String"],
        ),
        (
            "println",
            "fn println(value: String) -> Unit",
            &["value: String"],
        ),
        (
            "eprint",
            "fn eprint(value: String) -> Unit",
            &["value: String"],
        ),
        (
            "eprintln",
            "fn eprintln(value: String) -> Unit",
            &["value: String"],
        ),
        (
            "panic",
            "fn panic(message: String) -> Never",
            &["message: String"],
        ),
    ];

    let (_, label, params) = TABLE.iter().find(|(n, _, _)| *n == name)?;

    Some(SignatureInformation {
        label: label.to_string(),
        documentation: None,
        parameters: Some(
            params
                .iter()
                .map(|p| ParameterInformation {
                    label: ParameterLabel::Simple(p.to_string()),
                    documentation: None,
                })
                .collect(),
        ),
        active_parameter: None,
    })
}

fn type_expr_name(kind: &TypeExprKind) -> String {
    match kind {
        TypeExprKind::Named(n) => n.clone(),
        TypeExprKind::Generic(n, args) => {
            if args.is_empty() {
                n.clone()
            } else {
                let arg_names: Vec<String> = args.iter().map(|a| type_expr_name(&a.kind)).collect();
                format!("{}<{}>", n, arg_names.join(", "))
            }
        }
        TypeExprKind::Fn(_, _) => "fn".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(text: &str, cursor: usize) -> Option<(String, u32)> {
        find_active_call(text, cursor).map(|c| (c.callee.to_string(), c.active_parameter))
    }

    #[test]
    fn find_active_call_basic() {
        // foo(a, |b)  — cursor at 7, before 'b'
        let text = "foo(a, b)";
        assert_eq!(call(text, 7), Some(("foo".to_string(), 1)));
    }

    #[test]
    fn find_active_call_nested() {
        // foo(bar(1, 2), |x)  — cursor before 'x'
        let text = "foo(bar(1, 2), x)";
        let cursor = text.find(", x").unwrap() + 2; // position of 'x'
        assert_eq!(call(text, cursor), Some(("foo".to_string(), 1)));
    }

    #[test]
    fn find_active_call_string_with_paren() {
        // foo("(hi,", |x)  — the `(` and `,` inside the string are ignored
        let text = "foo(\"(hi,\", x)";
        // cursor before 'x' = position 12
        let cursor = text.rfind('x').unwrap();
        assert_eq!(call(text, cursor), Some(("foo".to_string(), 1)));
    }

    #[test]
    fn find_active_call_trailing_comma() {
        // foo(a,|)  — cursor right after the comma (before `)`)
        let text = "foo(a,)";
        assert_eq!(call(text, 6), Some(("foo".to_string(), 1)));
    }

    #[test]
    fn find_active_call_outside_paren() {
        // foo|  — cursor after identifier, no enclosing call
        let text = "foo";
        assert_eq!(call(text, 3), None);
    }

    #[test]
    fn build_signature_for_fn_renders_label() {
        use tyra_diagnostics::SourceMap;

        let src = "fn add(x: Int, y: Int) -> Int\n  x + y\nend\n";
        let mut sources = SourceMap::new();
        let mut report = tyra_diagnostics::Report::new();
        let id = sources.add("t.tyra".into(), src.into());
        let ast = tyra_parser::parse(id, &sources, &mut report);
        let f = ast
            .items
            .into_iter()
            .find_map(|it| {
                if let tyra_ast::Item::FnDef(f) = it {
                    Some(f)
                } else {
                    None
                }
            })
            .expect("no FnDef in AST");

        let sig = build_signature_for_fn(&f);
        assert_eq!(sig.label, "fn add(x: Int, y: Int) -> Int");

        let params = sig.parameters.expect("params should be present");
        assert_eq!(params.len(), 2);
        assert!(
            matches!(&params[0].label, ParameterLabel::Simple(s) if s == "x: Int"),
            "unexpected first param label: {:?}",
            params[0].label
        );
        assert!(
            matches!(&params[1].label, ParameterLabel::Simple(s) if s == "y: Int"),
            "unexpected second param label: {:?}",
            params[1].label
        );
    }
}
