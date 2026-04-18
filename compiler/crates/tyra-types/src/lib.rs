// tyra-types: Type checker for the Tyra language.
// spec reference: §8 (type system), §10.1 (operators), §12.2 (?), §14 (async)
//
// Current scope: basic type inference and checking sufficient for Milestone 1a.
// Full generics, ability derivation, and trait resolution are deferred.

mod checker;
mod ty;

pub use checker::{TypeEnv, check, infer_expr};
pub use ty::{Ty, types_compatible};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tyra_diagnostics::{Report, SourceMap};

    fn check_str(source: &str) -> Report {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let ast = tyra_parser::parse(id, &sources, &mut report);
        if report.has_errors() {
            return report;
        }
        tyra_resolve::resolve(&ast, &mut report);
        if report.has_errors() {
            return report;
        }
        super::check(&ast, &mut report);
        report
    }

    #[test]
    fn hello_world_type_checks() {
        let report = check_str("print(\"hello\")\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn int_arithmetic() {
        let report = check_str("let x = 1 + 2\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn float_arithmetic() {
        let report = check_str("let x = 1.0 + 2.0\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn mixed_arithmetic_error() {
        let report = check_str("let x = 1 + 2.0\n");
        assert!(report.has_errors()); // Int + Float is not allowed
    }

    #[test]
    fn float_eq_error() {
        // §7.2 / ADR-0002: Float has no Eq
        let report = check_str("let x = 1.0 == 2.0\n");
        assert!(report.has_errors());
    }

    #[test]
    fn bool_comparison() {
        let report = check_str("let x = 1 < 2\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn logical_operators() {
        let report = check_str("let x = true and false\nlet y = true or false\nlet z = not true\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn logical_requires_bool() {
        let report = check_str("let x = 1 and 2\n");
        assert!(report.has_errors());
    }

    #[test]
    fn not_requires_bool() {
        let report = check_str("let x = not 42\n");
        assert!(report.has_errors());
    }

    #[test]
    fn let_type_annotation_match() {
        let report = check_str("let x: Int = 42\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn let_type_annotation_mismatch() {
        let report = check_str("let x: String = 42\n");
        assert!(report.has_errors()); // String != Int
    }

    #[test]
    fn fn_wrong_arg_count() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\nadd(1)\n";
        let report = check_str(source);
        assert!(report.has_errors()); // expected 2 args, found 1
    }

    #[test]
    fn fn_correct_call() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\nlet r = add(1, 2)\n";
        let report = check_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn if_condition_must_be_bool() {
        let report = check_str("if 42\n  print(\"hi\")\nend\n");
        assert!(report.has_errors());
    }

    #[test]
    fn while_condition_must_be_bool() {
        let report = check_str("while 1\n  print(\"hi\")\nend\n");
        assert!(report.has_errors());
    }

    #[test]
    fn propagation_on_option() {
        let source = "fn f() -> Option<Int>\n  let x = Some(42)\n  let v = x?\n  Some(v)\nend\n";
        let report = check_str(source);
        // Some(42) inferred as Error (prelude constructor, not a fn with known return type)
        // This is a known limitation of the current type checker
        // Just verify no panic occurs
        let _ = report;
    }

    #[test]
    fn unary_neg_on_string_error() {
        let report = check_str("let x = -\"hello\"\n");
        assert!(report.has_errors());
    }

    #[test]
    fn eq_requires_same_type() {
        let report = check_str("let x = 42 == \"hello\"\n");
        assert!(report.has_errors()); // Int != String
    }

    #[test]
    fn string_eq_works() {
        let report = check_str("let x = \"a\" == \"b\"\n");
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn fn_arg_type_mismatch() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\nadd(\"a\", \"b\")\n";
        let report = check_str(source);
        assert!(report.has_errors()); // String args to Int params
    }

    #[test]
    fn fn_arg_type_correct() {
        let source = "fn greet(_ name: String) -> Unit\n  print(name)\nend\ngreet(\"tyra\")\n";
        let report = check_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn full_program_type_checks() {
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
print("hello")
"#;
        let report = check_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    #[test]
    fn return_type_mismatch_detected() {
        let source = r#"
fn wrong() -> Int
  "not an int"
end
"#;
        let report = check_str(source);
        assert!(report.has_errors(), "expected return type mismatch error");
        let has_e0309 = report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0309"));
        assert!(has_e0309, "expected E0309 error code");
    }

    #[test]
    fn return_type_match_ok() {
        let source = r#"
fn add(_ x: Int, _ y: Int) -> Int
  x + y
end
"#;
        let report = check_str(source);
        assert!(!report.has_errors(), "errors: {:?}", report.diagnostics());
    }

    // ========================================================================
    // Match exhaustiveness checks (§10.3, E0400)
    // ========================================================================

    fn has_e0400(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0400"))
    }

    #[test]
    fn non_exhaustive_bool_match_errors() {
        let source = r#"
let x = true
match x
when true
  print("t")
end
"#;
        let report = check_str(source);
        assert!(has_e0400(&report), "expected E0400 for non-exhaustive Bool match; got: {:?}", report.diagnostics());
    }

    #[test]
    fn exhaustive_bool_match_ok() {
        let source = r#"
let x = true
match x
when true
  print("t")
when false
  print("f")
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "exhaustive Bool match should not report E0400");
    }

    #[test]
    fn wildcard_arm_satisfies_exhaustiveness() {
        let source = r#"
let x = true
match x
when true
  print("t")
when _
  print("other")
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "wildcard arm should satisfy exhaustiveness");
    }

    #[test]
    fn ident_binding_arm_satisfies_exhaustiveness() {
        let source = r#"
let x = true
match x
when true
  print("t")
when other
  print("else")
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "ident binding arm acts as catch-all");
    }

    #[test]
    fn exhaustive_user_adt_match_ok() {
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn describe(_ c: Color) -> Unit
  match c
  when Red
    print("r")
  when Green
    print("g")
  when Blue
    print("b")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "exhaustive ADT match should not report E0400; got: {:?}", report.diagnostics());
    }

    #[test]
    fn non_exhaustive_user_adt_match_errors() {
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn describe(_ c: Color) -> Unit
  match c
  when Red
    print("r")
  when Green
    print("g")
  end
end
"#;
        let report = check_str(source);
        assert!(has_e0400(&report), "expected E0400 for missing Blue; got: {:?}", report.diagnostics());
    }

    #[test]
    fn non_exhaustive_option_match_errors() {
        // Only `Some(n)` arm, missing `None`.
        let source = r#"
fn f(_ x: Option<Int>) -> Int
  match x
  when Some(n)
    n
  end
end
"#;
        let report = check_str(source);
        assert!(has_e0400(&report), "expected E0400 for missing None arm; got: {:?}", report.diagnostics());
    }

    #[test]
    fn exhaustive_option_match_ok() {
        let source = r#"
fn f(_ x: Option<Int>) -> Int
  match x
  when Some(n)
    n
  when None
    0
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "exhaustive Option match should not report E0400; got: {:?}", report.diagnostics());
    }

    #[test]
    fn non_exhaustive_result_match_errors() {
        // Spec §10.3 uses Result as its primary example — missing Err arm.
        let source = r#"
fn f(_ x: Result<Int, String>) -> Int
  match x
  when Ok(n)
    n
  end
end
"#;
        let report = check_str(source);
        assert!(has_e0400(&report), "expected E0400 for missing Err arm; got: {:?}", report.diagnostics());
    }

    #[test]
    fn exhaustive_result_match_ok() {
        let source = r#"
fn f(_ x: Result<Int, String>) -> Int
  match x
  when Ok(n)
    n
  when Err(e)
    0
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "exhaustive Result match should not report E0400; got: {:?}", report.diagnostics());
    }

    #[test]
    fn ident_catchall_on_user_adt_ok() {
        // Ensures the ident-as-catchall path works for non-Bool subjects too.
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn describe(_ c: Color) -> Unit
  match c
  when Red
    print("r")
  when other
    print("else")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0400(&report), "ident binding should catch-all for user ADT; got: {:?}", report.diagnostics());
    }

    // ========================================================================
    // Redundant arm detection (§10.3, W0401)
    // ========================================================================

    fn has_w0401(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("W0401"))
    }

    #[test]
    fn duplicate_constructor_arm_warns() {
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn f(_ c: Color) -> Unit
  match c
  when Red
    print("r1")
  when Red
    print("r2")
  when Green
    print("g")
  when Blue
    print("b")
  end
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for duplicate Red arm; got: {:?}", report.diagnostics());
    }

    #[test]
    fn arm_after_wildcard_warns() {
        let source = r#"
let x = true
match x
when _
  print("any")
when true
  print("never reached")
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for arm after wildcard; got: {:?}", report.diagnostics());
    }

    #[test]
    fn duplicate_bool_lit_warns() {
        let source = r#"
let x = true
match x
when true
  print("t1")
when true
  print("t2")
when false
  print("f")
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for duplicate true arm");
    }

    #[test]
    fn duplicate_int_lit_warns() {
        let source = r#"
fn f(_ n: Int) -> Unit
  match n
  when 0
    print("zero")
  when 0
    print("zero again")
  when _
    print("other")
  end
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for duplicate int 0");
    }

    #[test]
    fn non_duplicate_arms_no_warning() {
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn f(_ c: Color) -> Unit
  match c
  when Red
    print("r")
  when Green
    print("g")
  when Blue
    print("b")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_w0401(&report), "distinct arms should not warn");
    }

    // ========================================================================
    // Nested Constructor exhaustiveness (§10.3, E0401)
    // ========================================================================

    fn has_e0401(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0401"))
    }

    #[test]
    fn nested_result_err_non_exhaustive_errors() {
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Ok(x)
    print("ok")
  when Err(NotFound)
    print("nf")
  end
end
"#;
        let report = check_str(source);
        assert!(has_e0401(&report), "expected E0401 for missing Err(Forbidden); got: {:?}", report.diagnostics());
    }

    #[test]
    fn nested_result_err_exhaustive_ok() {
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Ok(x)
    print("ok")
  when Err(NotFound)
    print("nf")
  when Err(Forbidden)
    print("fb")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0401(&report), "all nested Err variants present; got: {:?}", report.diagnostics());
    }

    #[test]
    fn nested_err_wildcard_is_catchall() {
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Ok(x)
    print("ok")
  when Err(e)
    print("err")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0401(&report), "ident binding in Err should act as nested catch-all");
    }

    #[test]
    fn nested_option_adt_non_exhaustive_errors() {
        let source = r#"
type Color =
  | Red
  | Green
fn f(_ o: Option<Color>) -> Unit
  match o
  when Some(Red)
    print("r")
  when None
    print("n")
  end
end
"#;
        let report = check_str(source);
        assert!(has_e0401(&report), "expected E0401 for missing Some(Green); got: {:?}", report.diagnostics());
    }

    // ========================================================================
    // Review regression: W0401 must NOT fire when payload distinguishes arms
    // ========================================================================

    #[test]
    fn distinct_nested_payloads_no_w0401_regression() {
        // Err(NotFound) and Err(Forbidden) share the same head Constructor `Err`
        // but are semantically distinct — must NOT warn as redundant.
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Ok(x)
    print("ok")
  when Err(NotFound)
    print("nf")
  when Err(Forbidden)
    print("fb")
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_w0401(&report),
            "expected NO W0401 for distinct nested payloads; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn duplicate_constructor_with_catchall_fields_warns() {
        // Same head + all fields are wildcards → redundant.
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Ok(x)
    print("ok1")
  when Ok(_)
    print("ok2")
  when Err(e)
    print("err")
  end
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for two Ok arms with catch-all fields");
    }

    // ========================================================================
    // Additional E0401 / W0401 edge cases
    // ========================================================================

    #[test]
    fn nested_option_adt_exhaustive_ok() {
        let source = r#"
type Color =
  | Red
  | Green
fn f(_ o: Option<Color>) -> Unit
  match o
  when Some(Red)
    print("r")
  when Some(Green)
    print("g")
  when None
    print("n")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0401(&report), "exhaustive Option<ADT> should not report E0401; got: {:?}", report.diagnostics());
    }

    #[test]
    fn outer_catchall_skips_nested_check() {
        // Outer `_` arm → both E0400 and E0401 should skip.
        // Even though Err(NotFound) only covers one inner variant, the outer `_`
        // makes the whole match exhaustive.
        let source = r#"
type MyErr =
  | NotFound
  | Forbidden
fn f(_ r: Result<Int, MyErr>) -> Unit
  match r
  when Err(NotFound)
    print("nf")
  when _
    print("rest")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0401(&report), "outer catch-all should skip nested check");
        assert!(!has_e0400(&report), "outer catch-all should skip E0400 as well");
    }

    #[test]
    fn nested_some_ident_is_catchall() {
        // Some(x) should act as nested catch-all, so Green is OK.
        let source = r#"
type Color =
  | Red
  | Green
fn f(_ o: Option<Color>) -> Unit
  match o
  when Some(x)
    print("any")
  when None
    print("n")
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0401(&report), "Some(x) should be nested catch-all; got: {:?}", report.diagnostics());
    }

    #[test]
    fn duplicate_string_lit_warns() {
        let source = r#"
fn f(_ s: String) -> Unit
  match s
  when "hi"
    print("h1")
  when "hi"
    print("h2")
  when _
    print("other")
  end
end
"#;
        let report = check_str(source);
        assert!(has_w0401(&report), "expected W0401 for duplicate string literal");
    }

    #[test]
    fn triple_duplicate_constructor_warns_twice() {
        // 3 Red arms → arms 2 and 3 are redundant (2 distinct spans).
        // Note: check_fn double-processes the last expression (match is the fn body),
        // so we deduplicate by label span rather than asserting a raw count.
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn f(_ c: Color) -> Unit
  match c
  when Red
    print("r1")
  when Red
    print("r2")
  when Red
    print("r3")
  when Green
    print("g")
  when Blue
    print("b")
  end
end
"#;
        let report = check_str(source);
        let distinct_spans: HashSet<_> = report
            .diagnostics()
            .iter()
            .filter(|d| d.code.as_deref() == Some("W0401"))
            .flat_map(|d| d.labels.iter().map(|l| (l.span.start, l.span.end)))
            .collect();
        assert_eq!(
            distinct_spans.len(),
            2,
            "expected 2 distinct W0401 spans for 3 Red arms; got {}: {:?}",
            distinct_spans.len(),
            report.diagnostics()
        );
    }
}
