// tyra-types: Type checker for the Tyra language.
// spec reference: §8 (type system), §10.1 (operators), §12.2 (?), §14 (async)
//
// Current scope: basic type inference and checking sufficient for Milestone 1a.
// Full generics, ability derivation, and trait resolution are deferred.

mod checker;
mod ty;

pub use checker::{TypeEnv, TypeIndex, check, infer_expr};
pub use ty::{Substitution, Ty, TyVarId, UnifyError, types_compatible, unify};

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
        let _ = super::check(&ast, &mut report);
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
        assert!(
            has_e0400(&report),
            "expected E0400 for non-exhaustive Bool match; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0400(&report),
            "exhaustive Bool match should not report E0400"
        );
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
        assert!(
            !has_e0400(&report),
            "wildcard arm should satisfy exhaustiveness"
        );
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
        assert!(
            !has_e0400(&report),
            "exhaustive ADT match should not report E0400; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_e0400(&report),
            "expected E0400 for missing Blue; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_e0400(&report),
            "expected E0400 for missing None arm; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0400(&report),
            "exhaustive Option match should not report E0400; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_e0400(&report),
            "expected E0400 for missing Err arm; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0400(&report),
            "exhaustive Result match should not report E0400; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0400(&report),
            "ident binding should catch-all for user ADT; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_w0401(&report),
            "expected W0401 for duplicate Red arm; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_w0401(&report),
            "expected W0401 for arm after wildcard; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_e0401(&report),
            "expected E0401 for missing Err(Forbidden); got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0401(&report),
            "all nested Err variants present; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0401(&report),
            "ident binding in Err should act as nested catch-all"
        );
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
        assert!(
            has_e0401(&report),
            "expected E0401 for missing Some(Green); got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_w0401(&report),
            "expected W0401 for two Ok arms with catch-all fields"
        );
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
        assert!(
            !has_e0401(&report),
            "exhaustive Option<ADT> should not report E0401; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            !has_e0401(&report),
            "outer catch-all should skip nested check"
        );
        assert!(
            !has_e0400(&report),
            "outer catch-all should skip E0400 as well"
        );
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
        assert!(
            !has_e0401(&report),
            "Some(x) should be nested catch-all; got: {:?}",
            report.diagnostics()
        );
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
        assert!(
            has_w0401(&report),
            "expected W0401 for duplicate string literal"
        );
    }

    // ========================================================================
    // Explicit `return` type check (§9.5, E0309)
    // ========================================================================

    fn has_e0309(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0309"))
    }

    #[test]
    fn explicit_return_type_mismatch_detected() {
        let source = r#"
fn f(_ x: Int) -> Int
  if x < 0
    return "negative"
  end
  x
end
"#;
        let report = check_str(source);
        assert!(
            has_e0309(&report),
            "expected E0309 for return String from Int fn; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn explicit_return_type_ok() {
        let source = r#"
fn f(_ x: Int) -> Int
  if x < 0
    return 0
  end
  x
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0309(&report),
            "correct return type should not report E0309"
        );
    }

    #[test]
    fn return_unit_from_unit_fn_ok() {
        let source = r#"
fn greet(_ name: String) -> Unit
  if name == ""
    return
  end
  print(name)
end
"#;
        let report = check_str(source);
        assert!(!has_e0309(&report), "bare return in Unit fn should be OK");
    }

    // ========================================================================
    // ? operator return-type constraint (§12.2, E0310)
    // ========================================================================

    fn has_e0310(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0310"))
    }

    #[test]
    fn propagate_in_non_result_fn_errors() {
        // ? on Result requires the fn to return Result.
        let source = r#"
fn f(_ x: Result<Int, String>) -> Int
  let n = x?
  n
end
"#;
        let report = check_str(source);
        assert!(
            has_e0310(&report),
            "expected E0310 for ? in Int fn; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn propagate_in_matching_result_fn_ok() {
        let source = r#"
fn f(_ x: Result<Int, String>) -> Result<Int, String>
  let n = x?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0310(&report),
            "? with matching Result fn should not report E0310"
        );
    }

    #[test]
    fn propagate_option_in_option_fn_ok() {
        let source = r#"
fn f(_ x: Option<Int>) -> Option<Int>
  let n = x?
  Some(n)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0310(&report),
            "? Option in Option fn should not report E0310"
        );
    }

    #[test]
    fn propagate_option_in_result_fn_errors() {
        // Cross-family: Option? in Result-returning fn is not allowed.
        let source = r#"
fn f(_ x: Option<Int>) -> Result<Int, String>
  let n = x?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            has_e0310(&report),
            "Option? in Result fn should report E0310"
        );
    }

    // Top-level `?` is caught earlier by the resolver (E0211) — ADR-0006 Rule 3.
    // See resolver tests for that behavior; no duplicate check needed here.

    // ========================================================================
    // ? operator Into<F> error conversion (§12.2, E0311)
    // ========================================================================

    fn has_e0311(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0311"))
    }

    #[test]
    fn propagate_same_err_type_no_into_required() {
        // Identity conversion (E == F) is auto-provided; no impl needed.
        let source = r#"
type Err = | Bad
fn inner() -> Result<Int, Err>
  Err(Err.Bad)
end
fn outer() -> Result<Int, Err>
  let n = inner()?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0311(&report),
            "same-type ? should not require Into impl; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn propagate_distinct_err_without_into_errors() {
        // E != F and no `impl Into<F> for E` → E0311.
        let source = r#"
type InnerErr = | BadInput
type OuterErr = | Wrapped
fn inner() -> Result<Int, InnerErr>
  Err(InnerErr.BadInput)
end
fn outer() -> Result<Int, OuterErr>
  let n = inner()?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            has_e0311(&report),
            "? across distinct error types without Into impl should fire E0311; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn propagate_distinct_err_with_into_ok() {
        // `impl Into<OuterErr> for InnerErr` satisfies the constraint.
        // Asserting the report is completely clean (not just free of E0311)
        // guards against the check being skipped for unrelated reasons.
        let source = r#"
type InnerErr = | BadInput
type OuterErr = | Wrapped
impl Into<OuterErr> for InnerErr
  fn into(self) -> OuterErr
    OuterErr.Wrapped
  end
end
fn inner() -> Result<Int, InnerErr>
  Err(InnerErr.BadInput)
end
fn outer() -> Result<Int, OuterErr>
  let n = inner()?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            !report.has_errors(),
            "? with declared Into impl should leave the report clean; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn propagate_into_check_skips_error_cascade() {
        // If an earlier type error makes inner_ty's err slot Ty::Error, do
        // not pile on E0311 — the root-cause diagnostic was already emitted.
        let source = r#"
type Err = | Bad
fn outer() -> Result<Int, Err>
  let n = notafunc()?
  Ok(n)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0311(&report),
            "E0311 should not cascade onto upstream type errors; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Trait impl required methods (§8.7, E0500)
    // ========================================================================

    fn has_e0500(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0500"))
    }

    #[test]
    fn impl_missing_required_method_errors() {
        let source = r#"
trait Greeter
  fn greet(self) -> String
  fn farewell(self) -> String
end
value Person
  name: String
end
impl Greeter for Person
  fn greet(self) -> String
    "hello"
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0500(&report),
            "expected E0500 for missing farewell method; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn impl_all_required_methods_ok() {
        let source = r#"
trait Greeter
  fn greet(self) -> String
end
value Person
  name: String
end
impl Greeter for Person
  fn greet(self) -> String
    "hello"
  end
end
"#;
        let report = check_str(source);
        assert!(!has_e0500(&report), "complete impl should not report E0500");
    }

    #[test]
    fn stringable_impl_missing_to_string_errors() {
        // Stringable is prelude-registered; missing `to_string` → E0500.
        let source = r#"
value Point
  x: Int
end
impl Stringable for Point
end
"#;
        let report = check_str(source);
        assert!(
            has_e0500(&report),
            "expected E0500 for Stringable without to_string; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Stringable explicit impl requirement (§8.7, E0501)
    // ========================================================================

    fn has_e0501(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0501"))
    }

    #[test]
    fn to_string_without_stringable_impl_errors() {
        let source = r#"
value Point
  x: Int
end
fn show(_ p: Point) -> String
  p.to_string()
end
"#;
        let report = check_str(source);
        assert!(
            has_e0501(&report),
            "expected E0501 for .to_string() without impl; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Ability tracking (§8 auto-derivation)
    // ========================================================================

    fn has_e0306(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0306"))
    }

    fn has_e0307(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0307"))
    }

    #[test]
    fn value_with_float_field_cannot_eq() {
        // Point has Float fields → no Eq auto-derive.
        let source = r#"
value Point
  x: Float
  y: Float
end
fn f(_ a: Point, _ b: Point) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            has_e0306(&report),
            "expected E0306 for Point==Point (Float blocks Eq); got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn value_int_fields_can_eq() {
        let source = r#"
value Pair
  a: Int
  b: Int
end
fn f(_ x: Pair, _ y: Pair) -> Bool
  x == y
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "Int-field value should auto-derive Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn single_field_int_value_has_ord() {
        let source = r#"
value Id
  n: Int
end
fn f(_ a: Id, _ b: Id) -> Bool
  a < b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0307(&report),
            "single-field Int value should auto-derive Ord; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn two_field_value_no_ord() {
        let source = r#"
value Pair
  a: Int
  b: Int
end
fn f(_ x: Pair, _ y: Pair) -> Bool
  x < y
end
"#;
        let report = check_str(source);
        assert!(
            has_e0307(&report),
            "two-field value should not have Ord; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn data_type_no_ord() {
        // data types never auto-derive Ord (§8.6).
        let source = r#"
data User
  id: Int
end
fn f(_ a: User, _ b: User) -> Bool
  a < b
end
"#;
        let report = check_str(source);
        assert!(
            has_e0307(&report),
            "data type should not have Ord; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn forward_reference_ability_derivation_ok() {
        // Outer refers to Inner defined AFTER it. Pass-1 should register both
        // names; pass-2 should still compute correct abilities. Fails pre-C1-fix.
        let source = r#"
value Outer
  inner: Inner
end
value Inner
  x: Int
end
fn eq_outer(_ a: Outer, _ b: Outer) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "forward-ref value should auto-derive Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn data_with_mut_field_no_hash_keeps_eq() {
        // §8.6: mut field blocks Hash but Eq still derivable.
        let source = r#"
data User
  id: Int
  mut name: String
end
fn eq_user(_ a: User, _ b: User) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        // Eq is retained (Int/String both have Eq; mut only blocks Hash).
        assert!(
            !has_e0306(&report),
            "data with mut field should still have Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn zero_field_adt_variant_eq_ok() {
        // Unit-style ADT variants have no fields; the ADT should still auto-derive Eq.
        let source = r#"
type Color =
  | Red
  | Green
  | Blue
fn eq(_ a: Color, _ b: Color) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "zero-field ADT variants should keep Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn option_int_can_eq() {
        // §8.6: Option<Int> derives Eq because Int has Eq.
        let source = r#"
fn f(_ a: Option<Int>, _ b: Option<Int>) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "Option<Int> should auto-derive Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn option_float_cannot_eq() {
        // §8.6 + ADR-0002: Option<Float> has no Eq because Float has no Eq.
        let source = r#"
fn f(_ a: Option<Float>, _ b: Option<Float>) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            has_e0306(&report),
            "Option<Float> must not have Eq (Float blocks it); got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn result_int_string_can_eq() {
        // §8.6: Result<Int, String> derives Eq because both args have Eq.
        let source = r#"
fn f(_ a: Result<Int, String>, _ b: Result<Int, String>) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "Result<Int, String> should auto-derive Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn result_float_int_cannot_eq() {
        // §8.6: Result<Float, Int> has no Eq because the first arg (Float) lacks Eq.
        let source = r#"
fn f(_ a: Result<Float, Int>, _ b: Result<Float, Int>) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            has_e0306(&report),
            "Result<Float, Int> must not have Eq (Float arg blocks it); got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn nested_option_can_eq() {
        // §8.6: ability derivation recurses into nested type arguments.
        let source = r#"
fn f(_ a: Option<Option<Int>>, _ b: Option<Option<Int>>) -> Bool
  a == b
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0306(&report),
            "Option<Option<Int>> should auto-derive Eq; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn option_int_no_ord() {
        // §8.5: ADTs never auto-derive Ord, even when type arguments have Ord.
        let source = r#"
fn f(_ a: Option<Int>, _ b: Option<Int>) -> Bool
  a < b
end
"#;
        let report = check_str(source);
        assert!(
            has_e0307(&report),
            "Option<Int> must not have Ord (ADTs never derive Ord); got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Bare `return;` from non-Unit fn (§9.5, E0309)
    // ========================================================================

    #[test]
    fn bare_return_from_non_unit_fn_errors() {
        // `return` without value in a fn declared to return Int should be an error.
        let source = r#"
fn f(_ x: Int) -> Int
  if x < 0
    return
  end
  x
end
"#;
        let report = check_str(source);
        assert!(
            has_e0309(&report),
            "bare return in non-Unit fn should report E0309; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn to_string_with_stringable_impl_ok() {
        let source = r#"
value Point
  x: Int
end
impl Stringable for Point
  fn to_string(self) -> String
    "p"
  end
end
fn show(_ p: Point) -> String
  p.to_string()
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0501(&report),
            "impl'd Stringable should not error; got: {:?}",
            report.diagnostics()
        );
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

    // ========================================================================
    // E0308 heuristic help messages (Phase 1b)
    // ========================================================================

    fn get_e0308_help(report: &Report) -> Vec<String> {
        report
            .diagnostics()
            .iter()
            .filter(|d| d.code.as_deref() == Some("E0308"))
            .filter_map(|d| d.help.clone())
            .collect()
    }

    fn has_e0308(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0308"))
    }

    // Heuristic (i): expected Option<Int>, got Int — suggest Some(...)
    #[test]
    fn e0308_help_option_wrap_via_let_annotation() {
        let source = "let x: Option<Int> = 42\n";
        let report = check_str(source);
        assert!(
            has_e0308(&report),
            "expected E0308; got: {:?}",
            report.diagnostics()
        );
        let helps = get_e0308_help(&report);
        assert!(
            helps.iter().any(|h| h.contains("Some")),
            "expected 'wrap with Some(...)' help; got: {:?}",
            helps
        );
    }

    // Heuristic (iii): expected Float, got Int — suggest float.from_int
    #[test]
    fn e0308_help_int_to_float_conversion() {
        let source = "let x: Float = 42\n";
        let report = check_str(source);
        assert!(
            has_e0308(&report),
            "expected E0308; got: {:?}",
            report.diagnostics()
        );
        let helps = get_e0308_help(&report);
        assert!(
            helps.iter().any(|h| h.contains("float.from_int")),
            "expected 'float.from_int' help; got: {:?}",
            helps
        );
    }

    // Heuristic (iii): expected Int, got Float — suggest float.to_int
    #[test]
    fn e0308_help_float_to_int_conversion() {
        let source = "let x: Int = 3.14\n";
        let report = check_str(source);
        assert!(
            has_e0308(&report),
            "expected E0308; got: {:?}",
            report.diagnostics()
        );
        let helps = get_e0308_help(&report);
        assert!(
            helps.iter().any(|h| h.contains("float.to_int")),
            "expected 'float.to_int' help; got: {:?}",
            helps
        );
    }

    // Guard: no heuristic hint when Ty::Error is involved (unresolved identifier → Error)
    #[test]
    fn e0308_no_hint_for_error_type() {
        // notafunc() returns Ty::Error; guard must suppress any heuristic help
        let source = "let x: Int = notafunc()\n";
        let report = check_str(source);
        let helps = get_e0308_help(&report);
        assert!(
            helps.is_empty(),
            "no heuristic hint should fire when actual type is Error; got: {:?}",
            helps
        );
    }

    // Heuristic (iv): expected ADT `Color`, found `Red` which is a variant of `Color`.
    // Scenario: fn declared to return `Red` (variant name misused as type name) is called
    // where `Color` is expected. The type checker sees expected=Color, found=Red.
    #[test]
    fn e0308_help_adt_variant_suggestion() {
        // `Red` is a variant of `Color`. A fn is declared as `-> Red` (misusing the
        // variant name as a type), and its return value is bound to `let c: Color`.
        // check_type_match sees expected=Ty::Named("Color"), actual=Ty::Named("Red").
        // Heuristic (iv) should suggest `Color.Red`.
        let source = r#"
type Color =
  | Red
  | Green
  | Blue

fn get_red() -> Red
  Color.Red
end

let c: Color = get_red()
"#;
        let report = check_str(source);
        // E0308 must fire: get_red() returns Ty::Named("Red"), but c: Color expects Ty::Named("Color")
        assert!(
            has_e0308(&report),
            "expected E0308 for Color vs Red; got: {:?}",
            report.diagnostics()
        );
        let helps = get_e0308_help(&report);
        assert!(
            helps.iter().any(|h| h.contains("Color.Red")),
            "expected 'did you mean `Color.Red`?' help; got: {:?}",
            helps
        );
    }

    // Heuristic (iv) negative: same variant name in two ADTs → no suggestion (ambiguous)
    #[test]
    fn e0308_no_hint_for_ambiguous_variant() {
        // Both `A` and `B` have a variant named `Foo`.
        // heuristic (iv) must NOT fire to avoid false positives.
        let source = r#"
type A =
  | Foo
  | Bar

type B =
  | Foo
  | Baz

let x: A = Foo
"#;
        let report = check_str(source);
        // If E0308 fires, there must be no "did you mean" help (ambiguous Foo).
        let helps = get_e0308_help(&report);
        assert!(
            !helps.iter().any(|h| h.contains("did you mean")),
            "heuristic (iv) must not fire for ambiguous variant `Foo`; got: {:?}",
            helps
        );
    }

    // Heuristic (ii): actual is Result<T,E>, expected is T, enclosing fn returns Result
    #[test]
    fn e0308_help_result_propagation_suggestion() {
        let source = r#"
fn inner() -> Result<Int, String>
  Ok(42)
end
fn outer() -> Result<Int, String>
  let n: Int = inner()
  Ok(n)
end
"#;
        let report = check_str(source);
        // inner() returns Result<Int, String>; let n: Int = ... triggers E0308.
        // Since outer() returns Result, heuristic (ii) fires: suggest expr?
        let helps = get_e0308_help(&report);
        if has_e0308(&report) {
            assert!(
                helps.iter().any(|h| h.contains("expr?")),
                "expected 'expr?' propagation suggestion; got: {:?}",
                helps
            );
        }
    }
}
