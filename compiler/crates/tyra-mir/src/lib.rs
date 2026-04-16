// tyra-mir: Mid-level IR for the Tyra language.
// Desugars AST into a flat instruction sequence for codegen.
//
// Current scope (Milestone 1a): basic expressions, function calls, if/while.
// Full match lowering, closures, and async are deferred.

pub mod ir;
pub mod lower;
mod monomorphize;

pub use ir::*;
pub use lower::lower;

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::{Report, SourceMap};

    fn lower_str(source: &str) -> Program {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), source.into());
        let mut report = Report::new();
        let ast = tyra_parser::parse(id, &sources, &mut report);
        assert!(
            !report.has_errors(),
            "parse errors: {:?}",
            report.diagnostics()
        );
        lower(&ast)
    }

    #[test]
    fn hello_world_lowers() {
        let prog = lower_str("print(\"hello, tyra\")\n");
        assert_eq!(prog.functions.len(), 1);
        assert!(prog.functions[0].is_main);
        assert_eq!(prog.functions[0].name, "main");
        assert_eq!(prog.string_constants, vec!["hello, tyra"]);
    }

    #[test]
    fn fn_def_lowers() {
        let source = "fn add(_ x: Int, _ y: Int) -> Int\n  x + y\nend\n";
        let prog = lower_str(source);
        assert_eq!(prog.functions.len(), 1);
        assert_eq!(prog.functions[0].name, "add");
        assert_eq!(prog.functions[0].params.len(), 2);
        assert!(!prog.functions[0].is_main);
    }

    #[test]
    fn fn_and_top_level() {
        let source = "fn greet()\n  print(\"hi\")\nend\ngreet()\n";
        let prog = lower_str(source);
        assert_eq!(prog.functions.len(), 2); // greet + implicit main
        assert!(prog.functions.iter().any(|f| f.name == "greet"));
        assert!(prog.functions.iter().any(|f| f.name == "main" && f.is_main));
    }

    #[test]
    fn let_binding_lowers() {
        let prog = lower_str("let x = 42\nprint(\"done\")\n");
        let main = &prog.functions[0];
        // Should have: Const(42), Copy(x), Const(string), Call(print), Return
        assert!(main.body.len() >= 4);
    }

    #[test]
    fn if_else_lowers() {
        let source = "if true\n  print(\"yes\")\nelse\n  print(\"no\")\nend\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Should have: BranchIf, Label(then), Call, Jump, Label(else), Call, Jump, Label(end)
        let has_branch = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::BranchIf { .. }));
        assert!(has_branch);
    }

    #[test]
    fn string_dedup() {
        let prog = lower_str("print(\"hello\")\nprint(\"hello\")\n");
        // Same string should be deduplicated
        assert_eq!(prog.string_constants.len(), 1);
        assert_eq!(prog.string_constants[0], "hello");
    }

    #[test]
    fn return_in_fn() {
        let source = "fn f() -> Int\n  return 42\nend\n";
        let prog = lower_str(source);
        let f = &prog.functions[0];
        let has_return = f
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Return { value: Some(_) }));
        assert!(has_return);
    }

    #[test]
    fn while_lowers() {
        let source = "while true\n  print(\"loop\")\nend\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_jump = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Jump { .. }));
        assert!(has_jump);
    }

    #[test]
    fn implicit_return_unit() {
        let source = "fn f()\n  print(\"hi\")\nend\n";
        let prog = lower_str(source);
        let f = &prog.functions[0];
        assert!(matches!(
            f.body.last(),
            Some(Instruction::Return { value: None })
        ));
    }

    #[test]
    fn float_binop_uses_float_variant() {
        let prog = lower_str("let x = 1.0 + 2.0\n");
        let main = &prog.functions[0];
        let has_float_add = main.body.iter().any(|i| {
            matches!(
                i,
                Instruction::BinOp {
                    op: MirBinOp::AddFloat,
                    ..
                }
            )
        });
        assert!(has_float_add, "expected AddFloat, got: {:?}", main.body);
    }

    #[test]
    fn match_int_pattern_lowers() {
        let source = "fn f(_ n: Int) -> Int\n  match n\n  when 0\n    10\n  when 1\n    20\n  when _\n    30\n  end\nend\n";
        let prog = lower_str(source);
        let f = &prog.functions[0];
        // Should have BranchIf for Int literal patterns
        let has_branch = f
            .body
            .iter()
            .any(|i| matches!(i, Instruction::BranchIf { .. }));
        assert!(has_branch, "expected BranchIf in match lowering");
        // Should have Alloca + Store + Load for match result
        let has_alloca = f
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Alloca { .. }));
        let has_store = f
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Store { .. }));
        let has_load = f.body.iter().any(|i| matches!(i, Instruction::Load { .. }));
        assert!(has_alloca, "expected Alloca for match result");
        assert!(has_store, "expected Store for match result");
        assert!(has_load, "expected Load for match result");
    }

    #[test]
    fn adt_constructor_lowers_to_tag() {
        let source = "type Color =\n  | Red\n  | Green\n  | Blue\nlet c = Color.Red\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Color.Red should be Const(Int(0))
        let has_red_tag = main.body.iter().any(|i| {
            matches!(
                i,
                Instruction::Const {
                    value: Constant::Int(0),
                    ..
                }
            )
        });
        assert!(has_red_tag, "expected Color.Red = 0, got: {:?}", main.body);
    }

    #[test]
    fn constructor_pattern_dispatch() {
        let source = "type Color =\n  | Red\n  | Green\nfn f(_ c: Int) -> Int\n  match c\n  when Red\n    10\n  when Green\n    20\n  when _\n    0\n  end\nend\n";
        let prog = lower_str(source);
        let f = &prog.functions[0];
        let has_branch = f
            .body
            .iter()
            .any(|i| matches!(i, Instruction::BranchIf { .. }));
        assert!(has_branch, "expected BranchIf for constructor pattern");
    }

    #[test]
    fn string_interp_in_print() {
        let source = r#"let x = 42
print("value = #{x}")
"#;
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Should have multiple Call("print") instructions for each segment
        let print_calls = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::Call { func, .. } if func == "print"))
            .count();
        assert!(
            print_calls >= 2,
            "expected at least 2 print calls for interpolation, got {print_calls}"
        );
    }

    #[test]
    fn println_interp_adds_newline() {
        let source = r#"let x = 1
println("n=#{x}")
"#;
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Should have 3 print calls: literal "n=", expr x, newline "\n"
        let print_calls = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::Call { func, .. } if func == "print"))
            .count();
        assert!(
            print_calls >= 3,
            "expected at least 3 print calls for println interp, got {print_calls}"
        );
        // Newline should be interned
        assert!(prog.string_constants.contains(&"\n".to_string()));
    }

    #[test]
    fn full_program_lowers() {
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
print("done")
"#;
        let prog = lower_str(source);
        assert_eq!(prog.functions.len(), 2); // fib + main
        assert!(prog.functions.iter().any(|f| f.name == "fib"));
        assert!(prog.functions.iter().any(|f| f.name == "main" && f.is_main));
    }

    #[test]
    fn multi_field_value_type_constructor() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nlet p = Pair(first: 10, second: 20)\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_struct_init = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::StructInit { type_name, .. } if type_name == "Pair"));
        assert!(
            has_struct_init,
            "expected StructInit for Pair, got: {:?}",
            main.body
        );
        // Should have struct_defs for Pair
        assert_eq!(prog.struct_defs.len(), 1);
        assert_eq!(prog.struct_defs[0].name, "Pair");
        assert_eq!(prog.struct_defs[0].fields.len(), 2);
    }

    #[test]
    fn multi_field_value_type_field_access() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nlet p = Pair(first: 10, second: 20)\nlet a = p.first\nlet b = p.second\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let field_gets: Vec<_> = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::FieldGet { .. }))
            .collect();
        assert_eq!(
            field_gets.len(),
            2,
            "expected 2 FieldGet instructions, got: {:?}",
            field_gets
        );
    }

    #[test]
    fn multi_field_value_copy() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nlet p = Pair(first: 10, second: 20)\nlet p2 = p.copy(first: 99)\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // copy(first: 99) should: FieldGet second from p, then StructInit with 99 and extracted second
        let struct_inits: Vec<_> = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::StructInit { .. }))
            .collect();
        assert_eq!(
            struct_inits.len(),
            2,
            "expected 2 StructInit (constructor + copy), got: {:?}",
            struct_inits
        );
        // Should have a FieldGet for the non-overridden field (second)
        let has_field_get = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::FieldGet { field_index: 1, .. }));
        assert!(
            has_field_get,
            "expected FieldGet for second field in copy()"
        );
    }

    #[test]
    fn impl_method_lowered_as_mangled_function() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nimpl Summable for Pair\n  fn sum(self) -> Int\n    self.first + self.second\n  end\nend\n";
        let prog = lower_str(source);
        // Should have a function named Pair__sum
        let has_mangled = prog
            .functions
            .iter()
            .any(|f| f.name == "Pair__sum");
        assert!(
            has_mangled,
            "expected mangled function Pair__sum, got: {:?}",
            prog.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        // The mangled function should have self as first param
        let pair_sum = prog.functions.iter().find(|f| f.name == "Pair__sum").unwrap();
        assert_eq!(pair_sum.params[0].0, "self");
        assert_eq!(pair_sum.params[0].1, tyra_types::Ty::Named("Pair".into()));
    }

    #[test]
    fn method_call_resolved_to_mangled_name() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nimpl Summable for Pair\n  fn sum(self) -> Int\n    self.first + self.second\n  end\nend\nlet p = Pair(first: 10, second: 20)\nlet r = p.sum()\n";
        let prog = lower_str(source);
        let main = &prog.functions.iter().find(|f| f.name == "main").unwrap();
        // Should have a Call to Pair__sum
        let has_call = main.body.iter().any(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "Pair__sum")
        });
        assert!(
            has_call,
            "expected Call to Pair__sum, got: {:?}",
            main.body
        );
    }

    #[test]
    fn mut_local_uses_alloca_store_load() {
        let source = "mut x = 10\nx = 20\nprintln(x)\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_alloca = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Alloca { dest } if dest == "x"));
        let store_count = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::Store { dest, .. } if dest == "x"))
            .count();
        let has_load = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Load { source, .. } if source == "x"));
        assert!(has_alloca, "expected Alloca for mut x");
        assert!(store_count >= 2, "expected at least 2 Stores to x (init + reassign), got {store_count}");
        assert!(has_load, "expected Load from x for println");
    }

    #[test]
    fn data_field_mutation() {
        let source = "data User\n  id: Int\n  mut name: String\nend\nmut user = User(id: 1, name: \"alice\")\nuser.name = \"bob\"\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Should have Alloca for user
        let has_alloca = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::Alloca { dest } if dest == "user"));
        assert!(has_alloca, "expected Alloca for mut user");
        // Should have at least 2 StructInit (constructor + field mutation rebuild)
        let struct_inits = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::StructInit { .. }))
            .count();
        assert!(
            struct_inits >= 2,
            "expected >= 2 StructInit (init + field mutation), got {struct_inits}"
        );
    }

    #[test]
    fn string_interp_emits_string_format() {
        let source = "let name = \"world\"\nlet s = \"hello #{name}\"\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_format = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::StringFormat { .. }));
        assert!(
            has_format,
            "expected StringFormat instruction for standalone string interpolation"
        );
    }

    #[test]
    fn string_interp_in_print_uses_segments() {
        let source = "let x = 42\nprint(\"val=#{x}\")\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // print with StringInterp should use segment approach (multiple print calls)
        let print_calls = main
            .body
            .iter()
            .filter(|i| matches!(i, Instruction::Call { func, .. } if func == "print"))
            .count();
        assert!(
            print_calls >= 2,
            "expected multiple print calls for print+interp, got {print_calls}"
        );
        // Should NOT have StringFormat (optimization: direct segment printing)
        let has_format = main
            .body
            .iter()
            .any(|i| matches!(i, Instruction::StringFormat { .. }));
        assert!(
            !has_format,
            "print+interp should NOT use StringFormat (segment optimization)"
        );
    }

    #[test]
    fn ok_or_converts_option_to_result() {
        // spec §12.2: Option<T>.ok_or(err) → Result<T, E>
        let source = "\
type LookupError = | NotFound\n\
fn find(_ id: Int) -> Option<String>\n\
  if id == 1\n\
    Some(\"alice\")\n\
  else\n\
    None\n\
  end\n\
end\n\
fn get(_ id: Int) -> Result<String, LookupError>\n\
  let name = find(id).ok_or(LookupError.NotFound)?\n\
  Ok(name)\n\
end\n";
        let prog = lower_str(source);
        let get_fn = prog.functions.iter().find(|f| f.name == "get").unwrap();
        // Should have AdtTag (for ok_or tag check)
        let has_adt_tag = get_fn.body.iter().any(|i| {
            matches!(i, Instruction::AdtTag { .. })
        });
        assert!(has_adt_tag, "expected AdtTag for ok_or() Option tag check");
        // Should have branching for Some/None paths
        let has_branch = get_fn.body.iter().any(|i| {
            matches!(i, Instruction::BranchIf { .. })
        });
        assert!(has_branch, "expected BranchIf for ok_or() Some/None dispatch");
        // Should construct Result ADT (AdtInit with Result type)
        let result_inits = get_fn.body.iter().filter(|i| {
            matches!(i, Instruction::AdtInit { type_name, .. }
                if type_name.starts_with("Result__"))
        }).count();
        assert!(
            result_inits >= 2,
            "expected at least 2 Result AdtInit (Ok + Err paths), got {result_inits}"
        );
    }

    #[test]
    fn ok_or_result_type_registered() {
        // ok_or should register the Result<T, E> ADT struct def
        let source = "\
type MyErr = | Fail\n\
fn find(_ id: Int) -> Option<Int>\n\
  Some(42)\n\
end\n\
fn get() -> Result<Int, MyErr>\n\
  let x = find(1).ok_or(MyErr.Fail)?\n\
  Ok(x)\n\
end\n";
        let prog = lower_str(source);
        // The program should have a Result__Int__MyErr struct def
        let has_result_struct = prog.struct_defs.iter().any(|sd| {
            sd.name == "Result__Int__MyErr"
        });
        assert!(
            has_result_struct,
            "expected Result__Int__MyErr struct def, got: {:?}",
            prog.struct_defs.iter().map(|sd| &sd.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn defer_emits_before_implicit_return() {
        // spec §12.3: defer expressions execute LIFO before function return
        let source = "\
fn cleanup() -> Unit\n\
  print(\"done\")\n\
end\n\
fn work() -> Unit\n\
  defer cleanup()\n\
  print(\"working\")\n\
end\n";
        let prog = lower_str(source);
        let work_fn = prog.functions.iter().find(|f| f.name == "work").unwrap();
        // Find positions of print("working") and cleanup() calls
        let working_pos = work_fn.body.iter().position(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "print")
        });
        let cleanup_pos = work_fn.body.iter().rposition(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "cleanup")
        });
        assert!(
            working_pos.is_some() && cleanup_pos.is_some(),
            "expected both print and cleanup calls"
        );
        assert!(
            cleanup_pos.unwrap() > working_pos.unwrap(),
            "deferred cleanup() should come after print(\"working\")"
        );
        // cleanup should be before the Return
        let return_pos = work_fn.body.iter().rposition(|i| {
            matches!(i, Instruction::Return { .. })
        }).unwrap();
        assert!(
            cleanup_pos.unwrap() < return_pos,
            "deferred cleanup() should come before Return"
        );
    }

    #[test]
    fn defer_lifo_order() {
        // spec §12.3: multiple defers execute in reverse (LIFO) order
        let source = "\
fn first() -> Unit\n\
  print(\"1\")\n\
end\n\
fn second() -> Unit\n\
  print(\"2\")\n\
end\n\
fn work() -> Unit\n\
  defer first()\n\
  defer second()\n\
  print(\"work\")\n\
end\n";
        let prog = lower_str(source);
        let work_fn = prog.functions.iter().find(|f| f.name == "work").unwrap();
        // Collect all Call instructions after the print("work") call
        let calls: Vec<&str> = work_fn
            .body
            .iter()
            .filter_map(|i| match i {
                Instruction::Call { func, .. } if func != "print" => Some(func.as_str()),
                _ => None,
            })
            .collect();
        // LIFO: second() should be emitted before first()
        let second_pos = calls.iter().position(|&f| f == "second");
        let first_pos = calls.iter().position(|&f| f == "first");
        assert!(
            second_pos.is_some() && first_pos.is_some(),
            "expected both first and second calls, got: {:?}",
            calls
        );
        assert!(
            second_pos.unwrap() < first_pos.unwrap(),
            "LIFO: second() should come before first() in deferred execution"
        );
    }

    #[test]
    fn defer_emits_before_early_return() {
        // spec §12.3: defer must execute before ? operator early return
        let source = "\
fn cleanup() -> Unit\n\
  print(\"cleanup\")\n\
end\n\
fn inner() -> Option<Int>\n\
  None\n\
end\n\
fn work() -> Option<Int>\n\
  defer cleanup()\n\
  let x = inner()?\n\
  Some(x)\n\
end\n";
        let prog = lower_str(source);
        let work_fn = prog.functions.iter().find(|f| f.name == "work").unwrap();
        // In the ? failure path, cleanup should be called before Return
        // Find the propagate_fail label and check that cleanup comes before Return
        let fail_label_pos = work_fn.body.iter().position(|i| {
            matches!(i, Instruction::Label(l) if l.starts_with("propagate_fail"))
        });
        assert!(fail_label_pos.is_some(), "expected propagate_fail label");
        let after_fail: Vec<_> = work_fn.body[fail_label_pos.unwrap()..].to_vec();
        let cleanup_pos = after_fail.iter().position(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "cleanup")
        });
        let return_pos = after_fail.iter().position(|i| {
            matches!(i, Instruction::Return { .. })
        });
        assert!(
            cleanup_pos.is_some() && return_pos.is_some(),
            "expected cleanup and return after propagate_fail label"
        );
        assert!(
            cleanup_pos.unwrap() < return_pos.unwrap(),
            "deferred cleanup() should come before early Return in ? path"
        );
    }

    #[test]
    fn propagate_result_with_into_conversion() {
        // spec §12.2: ? on Result<T, E> in fn returning Result<U, F>
        // where E != F calls E__into(err) to convert.
        let source = "\
type InnerErr = | Bad\n\
type OuterErr = | Wrapped(msg: String)\n\
impl Into<OuterErr> for InnerErr\n\
  fn into(self) -> OuterErr\n\
    OuterErr.Wrapped(msg: \"converted\")\n\
  end\n\
end\n\
fn inner() -> Result<Int, InnerErr>\n\
  Err(InnerErr.Bad)\n\
end\n\
fn outer() -> Result<Int, OuterErr>\n\
  let x = inner()?\n\
  Ok(x)\n\
end\n";
        let prog = lower_str(source);
        let outer_fn = prog.functions.iter().find(|f| f.name == "outer").unwrap();
        // The ? operator failure path should call InnerErr__into
        let has_into_call = outer_fn.body.iter().any(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "InnerErr__into")
        });
        assert!(
            has_into_call,
            "expected Call to InnerErr__into in ? failure path, got: {:?}",
            outer_fn.body
        );
    }

    #[test]
    fn propagate_result_same_error_no_conversion() {
        // When inner and outer error types match, no Into conversion needed.
        let source = "\
type MyErr = | Fail\n\
fn inner() -> Result<Int, MyErr>\n\
  Err(MyErr.Fail)\n\
end\n\
fn outer() -> Result<Int, MyErr>\n\
  let x = inner()?\n\
  Ok(x)\n\
end\n";
        let prog = lower_str(source);
        let outer_fn = prog.functions.iter().find(|f| f.name == "outer").unwrap();
        // No Into conversion call when error types are the same
        let has_into_call = outer_fn.body.iter().any(|i| {
            matches!(i, Instruction::Call { func, .. } if func.contains("__into"))
        });
        assert!(
            !has_into_call,
            "should NOT call __into when error types match"
        );
    }

    #[test]
    fn turbofish_monomorphizes_generic_function() {
        // spec §8.4: turbofish call generates monomorphized function
        let source = "\
fn identity<T>(_ x: T) -> T\n\
  x\n\
end\n\
let a = identity::<Int>(42)\n\
let b = identity::<String>(\"hello\")\n";
        let prog = lower_str(source);
        // Should generate identity__Int and identity__String functions
        let has_int = prog.functions.iter().any(|f| f.name == "identity__Int");
        let has_str = prog.functions.iter().any(|f| f.name == "identity__String");
        assert!(
            has_int,
            "expected monomorphized function identity__Int, got: {:?}",
            prog.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert!(
            has_str,
            "expected monomorphized function identity__String, got: {:?}",
            prog.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        // Main should call both monomorphized functions
        let main = prog.functions.iter().find(|f| f.name == "main").unwrap();
        let calls_int = main.body.iter().any(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "identity__Int")
        });
        let calls_str = main.body.iter().any(|i| {
            matches!(i, Instruction::Call { func, .. } if func == "identity__String")
        });
        assert!(calls_int, "expected call to identity__Int");
        assert!(calls_str, "expected call to identity__String");
    }

    #[test]
    fn turbofish_monomorphized_params_have_concrete_types() {
        // The monomorphized function should have concrete parameter types
        let source = "\
fn wrap<T>(_ x: T) -> T\n\
  x\n\
end\n\
let a = wrap::<Int>(42)\n";
        let prog = lower_str(source);
        let wrap_int = prog.functions.iter().find(|f| f.name == "wrap__Int").unwrap();
        // Parameter should be Int, not a type variable
        assert_eq!(wrap_int.params.len(), 1);
        assert_eq!(wrap_int.params[0].1, tyra_types::Ty::Int);
        // Return type should be Int
        assert_eq!(wrap_int.return_type, tyra_types::Ty::Int);
    }

    #[test]
    fn turbofish_dedup_same_instantiation() {
        // Calling same turbofish twice should only generate one function
        let source = "\
fn id<T>(_ x: T) -> T\n\
  x\n\
end\n\
let a = id::<Int>(1)\n\
let b = id::<Int>(2)\n";
        let prog = lower_str(source);
        let count = prog.functions.iter().filter(|f| f.name == "id__Int").count();
        assert_eq!(count, 1, "expected exactly 1 id__Int function, got {count}");
    }
}
