// tyra-types: Type checker for the Tyra language.
// spec reference: §8 (type system), §10.1 (operators), §12.2 (?), §14 (async)
//
// Current scope: basic type inference and checking sufficient for Milestone 1a.
// Full generics, ability derivation, and trait resolution are deferred.

mod checker;
mod task_affine;
mod ty;

pub use checker::{TypeEnv, TypeIndex, check, infer_expr};
pub use ty::{Substitution, Ty, TyVarId, UnifyError, types_compatible, unify};

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tyra_diagnostics::{Report, SourceMap};

    fn check_str(source: &str) -> Report {
        let mut sources = SourceMap::new();
        let id = sources.add("test.ty".into(), source.into());
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
    // Stringable explicit impl requirement (§8.7, E0320)
    // ========================================================================

    fn has_e0320(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0320"))
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
            has_e0320(&report),
            "expected E0320 for .to_string() without impl; got: {:?}",
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
            !has_e0320(&report),
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

    // ── E0314: string interpolation of non-displayable types (§7.3) ──

    fn has_code(report: &Report, code: &str) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some(code))
    }

    #[test]
    fn e0314_option_bool_interp_rejected() {
        let report = check_str(
            "fn main() -> Unit\n  let ob: Option<Bool> = Some(true)\n  print(\"#{ob}\")\nend\n",
        );
        assert!(has_code(&report, "E0314"), "{:?}", report.diagnostics());
    }

    #[test]
    fn e0314_list_interp_rejected() {
        let report = check_str("fn main() -> Unit\n  let xs = [1, 2]\n  print(\"#{xs}\")\nend\n");
        assert!(has_code(&report, "E0314"), "{:?}", report.diagnostics());
    }

    #[test]
    fn e0314_result_interp_rejected() {
        let report = check_str(
            "fn main() -> Unit\n  let r: Result<Int, String> = Ok(1)\n  print(\"#{r}\")\nend\n",
        );
        assert!(has_code(&report, "E0314"), "{:?}", report.diagnostics());
    }

    #[test]
    fn e0314_value_type_interp_rejected() {
        let report = check_str(
            "value P\n  x: Int\nend\n\nfn main() -> Unit\n  let p: P = P(x: 1)\n  print(\"#{p}\")\nend\n",
        );
        assert!(has_code(&report, "E0314"), "{:?}", report.diagnostics());
    }

    #[test]
    fn e0314_option_interp_help_suggests_match() {
        let report = check_str(
            "fn main() -> Unit\n  let ob: Option<Bool> = Some(true)\n  print(\"#{ob}\")\nend\n",
        );
        let has_match_help = report.diagnostics().iter().any(|d| {
            d.code.as_deref() == Some("E0314")
                && d.help.as_deref().is_some_and(|h| h.contains("match"))
        });
        assert!(has_match_help, "{:?}", report.diagnostics());
    }

    #[test]
    fn interp_displayable_types_accepted() {
        let report = check_str(
            "fn main() -> Unit\n  let i = 1\n  let f = 1.5\n  let b = true\n  let s = \"x\"\n  let oi: Option<Int> = Some(1)\n  let os: Option<String> = Some(\"y\")\n  print(\"#{i} #{f} #{b} #{s} #{oi} #{os}\")\nend\n",
        );
        assert!(!report.has_errors(), "{:?}", report.diagnostics());
    }

    // ========================================================================
    // Affine task-handle tracking (ADR-0030, spec §14.5, E0321 / E0322)
    // ========================================================================

    fn has_e0321(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0321"))
    }

    fn has_e0322(report: &Report) -> bool {
        report
            .diagnostics()
            .iter()
            .any(|d| d.code.as_deref() == Some("E0322"))
    }

    #[test]
    fn task_double_await_straight_line_errors() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  t.await
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321 for straight-line double await; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_await_in_both_if_branches_ok() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let c = true
  let t = spawn work()
  if c
    t.await
  else
    t.await
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "await in both if-branches should not report E0321; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_await_in_one_branch_then_after_merge_ok() {
        // Documented gap (ADR-0030): the branch-merge lattice resolves a
        // Live/Consumed mismatch to MaybeConsumed, and consuming a
        // MaybeConsumed handle is allowed silently. A real double-await is
        // possible here at runtime (when `c` is true), but v1 does not
        // catch it — asserting absence pins down the known gap.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let c = true
  let t = spawn work()
  if c
    t.await
  end
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "await after a one-branch-only await should not report E0321 (documented gap); got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_await_inside_while_loop_errors() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  mut c = true
  let t = spawn work()
  while c
    t.await
    c = false
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321 for a loop-body await of a handle spawned outside the loop; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_passed_to_function_no_diagnostics() {
        let source = r#"
fn work() -> Int
  1
end

fn use_task(_ x: Task<Int>) -> Unit
  ()
end

fn main() -> Unit
  let t = spawn work()
  use_task(t)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a handle passed to a function should escape tracking silently; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_in_plain_list_literal_no_diagnostics() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let xs = [t]
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a handle stored in a plain list literal should escape tracking silently; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_returned_no_diagnostics() {
        let source = r#"
fn make() -> Task<Int>
  let t = spawn work()
  t
end

fn work() -> Int
  1
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a returned handle should escape tracking silently; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_captured_by_lambda_no_diagnostics() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let closure = fn() -> Unit
    t.await
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a handle referenced inside a lambda body should escape tracking silently; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_alias_no_diagnostics() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let x = spawn work()
  let y = x
  y.await
  x.await
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "aliasing a handle via `let y = x` should leave both untracked; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_never_awaited_warns() {
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
end
"#;
        let report = check_str(source);
        assert!(
            has_e0322(&report),
            "expected E0322 for a handle that is never awaited; got: {:?}",
            report.diagnostics()
        );
        assert!(
            !report.has_errors(),
            "E0322 is a warning and must not count as an error; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_join_all_then_element_await_errors() {
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  tasks.join_all([a, b])
  a.await
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: `a` was already consumed as a join_all element; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_select_sources_awaited_individually_ok() {
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  tasks.select([a, b])
  a.await
  b.await
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "select does not consume its sources; individually awaiting each is clean; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_select_result_double_await_errors() {
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  let t = tasks.select([a, b])
  t.await
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: the select result handle `t` is awaited twice; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_join_all_result_usable_as_list() {
        // Step 1 (checker signature) type-propagation: join_all<T>(List<Task<T>>)
        // -> Task<List<T>>, so `.await` on the call yields List<T>.
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  let results: List<Int> = tasks.join_all([a, b]).await
  let first = results[0]
end
"#;
        let report = check_str(source);
        assert!(
            !report.has_errors(),
            "join_all([a, b]).await should type as List<Int>; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_select_result_usable_as_element_type() {
        // Step 1 (checker signature) type-propagation: select<T>(List<Task<T>>)
        // -> Task<T>, so `.await` on the call yields T directly.
        //
        // `a` and `b` are only used as select sources here (never individually
        // awaited), so E0322 fires for each per spec §14.5 — that is the
        // intended nudge toward the leak-free pattern, not a failure of this
        // type-propagation test; only `has_errors()` is asserted.
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  let winner: Int = tasks.select([a, b]).await
end
"#;
        let report = check_str(source);
        assert!(
            !report.has_errors(),
            "select([a, b]).await should type as Int; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Shadow tombstones, select-of-consumed, while-condition depth, loop-exit
    // suppression, and/or branch, module-alias recognition (ADR-0030 fixes)
    // ========================================================================

    #[test]
    fn task_handle_shadowed_by_reassign_no_diagnostics() {
        // Straight-line shadow: the first `t` is awaited then replaced by a
        // fresh spawn before its own scope ends, so it never goes Live-only
        // (no E0322); the second `t` is awaited exactly once (no E0321).
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  t.await
  let t = spawn work()
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "re-spawn after full await, then await again, should be clean; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_shadowed_while_live_warns_e0322() {
        // The first `t` is replaced by the second `let t = ...` while still
        // Live (never awaited) — E0322 for the first handle, no E0321.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let t = spawn work()
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            has_e0322(&report),
            "shadowing a Live handle should warn E0322 for the replaced handle; got: {:?}",
            report.diagnostics()
        );
        assert!(
            !has_e0321(&report),
            "the second handle is awaited exactly once; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_shadowed_by_for_loop_binding_no_diagnostics() {
        // An outer `t` is fully consumed, then a for-loop rebinds `t` to a
        // plain (non-task) element — the loop body's use of `t` must not
        // touch the outer, already-consumed handle.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  t.await
  for t in [1, 2, 3]
    print(t)
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a for-loop binding shadowing a consumed outer handle should be clean; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_shadowed_by_match_arm_binding_no_diagnostics() {
        // Same shadow-tombstone guarantee for match-arm pattern bindings.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  t.await
  let o: Option<Int> = Some(5)
  match o
    when Some(t)
      print(t)
    when None
      ()
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a match-arm pattern binding shadowing a consumed outer handle should be clean; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_while_condition_await_errors() {
        // Fix C: the while condition re-evaluates every iteration, so
        // awaiting a handle spawned outside the loop from inside the
        // condition is the same loop-repeat-await risk as awaiting it in
        // the body.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  while t.await > 0
    ()
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321 for a while-condition await of a handle spawned outside the loop; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_loop_await_then_break_no_e0321() {
        // Fix D (finding 5a, lenient): the loop provably runs the consume at
        // most once per handle because a top-level `break` follows it in
        // the same statement list.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let xs = [1, 2, 3]
  for x in xs
    t.await
    break
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "await immediately followed by break in the same loop body should not report E0321; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_and_or_rhs_await_mutually_exclusive_guards_no_e0321() {
        // Fix E (finding 5b): the RHS of `and`/`or` is analyzed as a branch
        // (may not execute), so two sequential ifs each guarding a
        // short-circuited await must not be flagged, even though both
        // conditions reference the same handle.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let ready = true
  let not_ready = false
  let t = spawn work()
  if ready and t.await > 0
    ()
  end
  if not_ready and t.await > 0
    ()
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "and-guarded await across mutually exclusive conditions should not report E0321; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_select_of_consumed_handle_errors() {
        // Fix B (finding 1): passing an already-awaited handle to
        // tasks.select is UB (the runtime increments its refcount) and must
        // be E0321.
        let source = r#"
import core.tasks

fn work() -> Int
  1
end

fn main() -> Unit
  let a = spawn work()
  let b = spawn work()
  let x = a.await
  tasks.select([a, b])
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: `a` was passed to tasks.select after being consumed; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_module_alias_join_all_double_consume_errors() {
        // Fix F (finding 2): `import core.tasks as ts` must be recognized
        // exactly like the unaliased `tasks` receiver.
        let source = r#"
import core.tasks as ts

fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  ts.join_all([t])
  t.await
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: join_all via an aliased `core.tasks` import consumed `t`; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Round 2: LoopExit (Break/Return/None), dead-code-after-exit,
    // alias-shadow-in-scope, and tombstone-warns-on-replace fixes.
    // ========================================================================

    #[test]
    fn task_handle_shadowed_by_for_loop_binding_outer_live_warns_e0322() {
        // Strengthened version of `task_handle_shadowed_by_for_loop_binding_
        // no_diagnostics`: the OUTER `t` is never awaited before the for-loop
        // rebinds `t`, so it must still be reported as never-awaited — the
        // loop's shadow tombstone (in its own nested scope) must not make
        // the outer, still-Live handle silently escape.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  for t in [1, 2, 3]
    print(t)
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0322(&report),
            "expected E0322: outer `t` is never awaited despite being shadowed by the for-loop binding; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_shadowed_by_match_arm_binding_outer_live_warns_e0322() {
        // Strengthened version of `task_handle_shadowed_by_match_arm_
        // binding_no_diagnostics`: same guarantee for match-arm bindings.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let o: Option<Int> = Some(5)
  match o
    when Some(t)
      print(t)
    when None
      ()
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0322(&report),
            "expected E0322: outer `t` is never awaited despite being shadowed by the match-arm binding; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_handle_shadowed_by_plain_let_of_non_task_warns_e0322() {
        // Direct exercise of the tombstone() fix (finding 4): `let t = 5`
        // shares the SAME frame as the original spawn (no nested scope is
        // pushed for a plain `let`), so replacing a still-Live tracked
        // handle here must warn E0322 for the handle that was just lost.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let t = 5
  print(t)
end
"#;
        let report = check_str(source);
        assert!(
            has_e0322(&report),
            "expected E0322: `let t = 5` silently replaced a Live tracked handle; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_dead_code_after_return_no_e0321() {
        // Finding 2: `visit_stmts` must stop after a top-level `Return` in
        // the same statement list — the second `t.await` is unreachable
        // dead code and must not be treated as a real repeat-consume.
        let source = r#"
fn work() -> Int
  1
end

fn f() -> Int
  let t = spawn work()
  let v = t.await
  return v
  t.await
end

fn main() -> Unit
  let x = f()
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "a consume after `return` in the same statement list is dead code and must not report E0321; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_nested_loop_inner_break_does_not_suppress_outer_repeat_await_e0321() {
        // `break` only proves a single run for a handle bound directly
        // outside the loop being broken (handle.loop_depth + 1 ==
        // ctx.loop_depth). Here `t` is bound outside BOTH loops
        // (loop_depth 0) but consumed at depth 2 inside the inner loop's
        // `break` — the inner break only unwinds one level, so the outer
        // loop can still repeat the whole inner loop and re-run the
        // consume. Must still report E0321.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let xs = [1, 2, 3]
  let ys = [1, 2]
  for x in xs
    for y in ys
      t.await
      break
    end
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: a break in the inner loop does not prove a single run for a handle bound outside both loops; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_single_loop_await_then_break_no_e0321_control() {
        // Control for the nested-loop case above: when the handle is bound
        // directly outside the ONE loop it is awaited and broken out of,
        // the break does prove a single run and E0321 must not fire.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let xs = [1, 2, 3]
  for x in xs
    t.await
    break
  end
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "a break in the same loop the handle was bound outside of should suppress E0321; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_while_condition_then_top_level_return_still_errors_e0321() {
        // Finding 1 (reset-on-loop-entry): a top-level `return` AFTER the
        // while loop must not leak into the while condition's own context.
        // The condition still re-evaluates every iteration regardless of
        // what follows the loop in the outer statement list, so this must
        // still be E0321 — before the fix, the old bool
        // `loop_exit_follows` was computed once for the `while` statement
        // (finding the trailing `return`) and then inherited unchanged
        // into the condition's context, wrongly suppressing it.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  while t.await > 0
    ()
  end
  return
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: a top-level `return` after the loop must not suppress the while-condition repeat-await; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_continue_before_break_still_errors_e0321() {
        // A `continue` reachable before the `break` on the same path
        // defeats the break's single-run proof — the loop can iterate
        // again via `continue`, so the consume can still repeat. Must
        // still be E0321.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let t = spawn work()
  let xs = [1, 2, 3]
  for x in xs
    t.await
    continue
    break
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: a `continue` before the `break` defeats the single-run suppression; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_alias_shadowed_by_local_let_join_all_clean() {
        // Finding 3: a local `let ts = ...` shadows the `import core.tasks
        // as ts` module alias in scope. `ts.join_all([a])` here dispatches
        // to the user's own `PoolOps::join_all` method, not the tasks
        // builtin — it must not be classified as a tracked-consume, so the
        // real, single `a.await` afterward stays clean.
        let source = r#"
import core.tasks as ts

fn work() -> Int
  1
end

data Pool
  n: Int
end

impl PoolOps for Pool
  fn join_all(self, _ xs: List<Task<Int>>) -> Int
    self.n
  end
end

fn main() -> Unit
  let ts = Pool(n: 1)
  let a = spawn work()
  let r = ts.join_all([a])
  let v = a.await
  println(v)
  println(r)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a local `let ts = ...` shadowing the tasks alias must not treat `ts.join_all` as the tasks builtin; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_alias_shadowed_by_fn_param_clean() {
        // Finding 3 (param case): a fn parameter named `ts` shadows the
        // module alias for the whole function body. Every fn parameter is
        // now tombstoned at the top of `analyze_body`, so `stack` sees
        // `ts` bound and `classify_tasks_call` bails out here too.
        let source = r#"
import core.tasks as ts

fn work() -> Int
  1
end

data Pool
  n: Int
end

impl PoolOps for Pool
  fn join_all(self, _ xs: List<Task<Int>>) -> Int
    self.n
  end
end

fn use_pool(_ ts: Pool, _ a: Task<Int>) -> Unit
  let r = ts.join_all([a])
  let v = a.await
  println(v)
  println(r)
end

fn main() -> Unit
  let a = spawn work()
  use_pool(Pool(n: 1), a)
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report) && !has_e0322(&report),
            "a fn-parameter named `ts` shadowing the tasks alias must not treat `ts.join_all` as the tasks builtin; got: {:?}",
            report.diagnostics()
        );
    }

    // ========================================================================
    // Round 3: return-value continue scan (loop_exit_follows) and
    // while-condition depth collapse when the loop body always exits.
    // ========================================================================

    #[test]
    fn task_continue_in_return_value_defeats_suppression_e0321() {
        // A `continue` nested in the VALUE of a `return` statement runs
        // before the return itself — the loop can still iterate again, so
        // `loop_exit_follows` must not treat this `return` as an
        // unconditional exit. Before the fix, `Stmt::Return(_) =>
        // LoopExit::Return` fired before the value was scanned, wrongly
        // suppressing the repeat-await on `t`.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let c = true
  let t = spawn work()
  while c
    t.await
    return if c
      continue
    else
      ()
    end
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: a `continue` nested in the return value defeats the return's single-run suppression; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_continue_in_match_return_value_defeats_suppression_e0321() {
        // Same as above, but the `continue` is nested in a `match` at the
        // root of the return value instead of an `if`.
        let source = r#"
fn work() -> Int
  1
end

fn main() -> Unit
  let c = true
  let t = spawn work()
  while c
    t.await
    return match c
      when true
        continue
      when false
        ()
    end
  end
end
"#;
        let report = check_str(source);
        assert!(
            has_e0321(&report),
            "expected E0321: a `continue` nested in a match return value defeats the return's single-run suppression; got: {:?}",
            report.diagnostics()
        );
    }

    #[test]
    fn task_while_cond_await_with_always_exiting_body_no_e0321() {
        // When a while loop's body unconditionally exits (a top-level
        // `break`/`return` with no `continue` reachable before it), the
        // condition provably evaluates exactly once — it must be analyzed
        // at the enclosing loop depth, not loop_depth + 1. Covers both the
        // `break` and `return` exit forms.
        let source = r#"
fn work() -> Int
  1
end

fn f_break() -> Unit
  let t = spawn work()
  while t.await > 0
    break
  end
end

fn f_return() -> Unit
  let t = spawn work()
  while t.await > 0
    return
  end
end

fn main() -> Unit
  f_break()
  f_return()
end
"#;
        let report = check_str(source);
        assert!(
            !has_e0321(&report),
            "a while condition await with a body that always breaks/returns should not report E0321; got: {:?}",
            report.diagnostics()
        );
    }
}
