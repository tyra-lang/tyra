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
        lower(&ast, &sources)
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
            .any(|s| matches!(&s.instr, Instruction::BranchIf { .. }));
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
            .any(|s| matches!(&s.instr, Instruction::Return { value: Some(_) }));
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
            .any(|s| matches!(&s.instr, Instruction::Jump { .. }));
        assert!(has_jump);
    }

    #[test]
    fn implicit_return_unit() {
        let source = "fn f()\n  print(\"hi\")\nend\n";
        let prog = lower_str(source);
        let f = &prog.functions[0];
        assert!(
            f.body
                .last()
                .is_some_and(|s| matches!(&s.instr, Instruction::Return { value: None }))
        );
    }

    #[test]
    fn float_binop_uses_float_variant() {
        let prog = lower_str("let x = 1.0 + 2.0\n");
        let main = &prog.functions[0];
        let has_float_add = main.body.iter().any(|s| {
            matches!(
                s.instr,
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
            .any(|s| matches!(&s.instr, Instruction::BranchIf { .. }));
        assert!(has_branch, "expected BranchIf in match lowering");
        // Should have Alloca + Store + Load for match result
        let has_alloca = f
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Alloca { .. }));
        let has_store = f
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Store { .. }));
        let has_load = f
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Load { .. }));
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
        let has_red_tag = main.body.iter().any(|s| {
            matches!(
                s.instr,
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
            .any(|s| matches!(&s.instr, Instruction::BranchIf { .. }));
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
            .filter(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "print"))
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
            .filter(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "print"))
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
        let source =
            "value Pair\n  first: Int\n  second: Int\nend\nlet p = Pair(first: 10, second: 20)\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_struct_init = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::StructInit { type_name, .. } if type_name == "Pair"));
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
            .filter(|s| matches!(&s.instr, Instruction::FieldGet { .. }))
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
            .filter(|s| matches!(&s.instr, Instruction::StructInit { .. }))
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
            .any(|s| matches!(&s.instr, Instruction::FieldGet { field_index: 1, .. }));
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
        let has_mangled = prog.functions.iter().any(|f| f.name == "Pair__sum");
        assert!(
            has_mangled,
            "expected mangled function Pair__sum, got: {:?}",
            prog.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        // The mangled function should have self as first param
        let pair_sum = prog
            .functions
            .iter()
            .find(|f| f.name == "Pair__sum")
            .unwrap();
        assert_eq!(pair_sum.params[0].0, "self");
        assert_eq!(pair_sum.params[0].1, tyra_types::Ty::Named("Pair".into()));
    }

    #[test]
    fn method_call_resolved_to_mangled_name() {
        let source = "value Pair\n  first: Int\n  second: Int\nend\nimpl Summable for Pair\n  fn sum(self) -> Int\n    self.first + self.second\n  end\nend\nlet p = Pair(first: 10, second: 20)\nlet r = p.sum()\n";
        let prog = lower_str(source);
        let main = &prog.functions.iter().find(|f| f.name == "main").unwrap();
        // Should have a Call to Pair__sum
        let has_call = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "Pair__sum"));
        assert!(has_call, "expected Call to Pair__sum, got: {:?}", main.body);
    }

    #[test]
    fn mut_local_uses_alloca_store_load() {
        let source = "mut x = 10\nx = 20\nprintln(x)\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let has_alloca = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Alloca { dest } if dest == "x"));
        let store_count = main
            .body
            .iter()
            .filter(|s| matches!(&s.instr, Instruction::Store { dest, .. } if dest == "x"))
            .count();
        let has_load = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Load { source, .. } if source == "x"));
        assert!(has_alloca, "expected Alloca for mut x");
        assert!(
            store_count >= 2,
            "expected at least 2 Stores to x (init + reassign), got {store_count}"
        );
        assert!(has_load, "expected Load from x for println");
    }

    #[test]
    fn data_field_mutation() {
        // §8.6: data type field mutation uses FieldSet (GEP+store in-place), not struct rebuild
        let source = "data User\n  id: Int\n  mut name: String\nend\nmut user = User(id: 1, name: \"alice\")\nuser.name = \"bob\"\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        // Should have Alloca for user
        let has_alloca = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Alloca { dest } if dest == "user"));
        assert!(has_alloca, "expected Alloca for mut user");
        // Should have exactly 1 StructInit (constructor only; mutation uses FieldSet)
        let struct_inits = main
            .body
            .iter()
            .filter(|s| matches!(&s.instr, Instruction::StructInit { .. }))
            .count();
        assert!(
            struct_inits == 1,
            "expected exactly 1 StructInit (constructor), got {struct_inits}"
        );
        // Should have exactly 1 FieldSet (in-place mutation via GEP+store)
        let field_sets = main
            .body
            .iter()
            .filter(|s| matches!(&s.instr, Instruction::FieldSet { .. }))
            .count();
        assert!(
            field_sets == 1,
            "expected exactly 1 FieldSet for user.name = \"bob\", got {field_sets}"
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
            .any(|s| matches!(&s.instr, Instruction::StringFormat { .. }));
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
            .filter(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "print"))
            .count();
        assert!(
            print_calls >= 2,
            "expected multiple print calls for print+interp, got {print_calls}"
        );
        // Should NOT have StringFormat (optimization: direct segment printing)
        let has_format = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::StringFormat { .. }));
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
        let has_adt_tag = get_fn
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::AdtTag { .. }));
        assert!(has_adt_tag, "expected AdtTag for ok_or() Option tag check");
        // Should have branching for Some/None paths
        let has_branch = get_fn
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::BranchIf { .. }));
        assert!(
            has_branch,
            "expected BranchIf for ok_or() Some/None dispatch"
        );
        // Should construct Result ADT (AdtInit with Result type)
        let result_inits = get_fn
            .body
            .iter()
            .filter(|i| {
                matches!(&i.instr, Instruction::AdtInit { type_name, .. }
                if type_name.starts_with("Result__"))
            })
            .count();
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
        let has_result_struct = prog
            .struct_defs
            .iter()
            .any(|sd| sd.name == "Result__Int__MyErr");
        assert!(
            has_result_struct,
            "expected Result__Int__MyErr struct def, got: {:?}",
            prog.struct_defs
                .iter()
                .map(|sd| &sd.name)
                .collect::<Vec<_>>()
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
        let working_pos = work_fn
            .body
            .iter()
            .position(|i| matches!(&i.instr, Instruction::Call { func, .. } if func == "print"));
        let cleanup_pos = work_fn
            .body
            .iter()
            .rposition(|i| matches!(&i.instr, Instruction::Call { func, .. } if func == "cleanup"));
        assert!(
            working_pos.is_some() && cleanup_pos.is_some(),
            "expected both print and cleanup calls"
        );
        assert!(
            cleanup_pos.unwrap() > working_pos.unwrap(),
            "deferred cleanup() should come after print(\"working\")"
        );
        // cleanup should be before the Return
        let return_pos = work_fn
            .body
            .iter()
            .rposition(|i| matches!(&i.instr, Instruction::Return { .. }))
            .unwrap();
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
            .filter_map(|s| match &s.instr {
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
        let fail_label_pos = work_fn.body.iter().position(
            |i| matches!(&i.instr, Instruction::Label(l) if l.starts_with("propagate_fail")),
        );
        assert!(fail_label_pos.is_some(), "expected propagate_fail label");
        let after_fail: Vec<_> = work_fn.body[fail_label_pos.unwrap()..].to_vec();
        let cleanup_pos = after_fail
            .iter()
            .position(|i| matches!(&i.instr, Instruction::Call { func, .. } if func == "cleanup"));
        let return_pos = after_fail
            .iter()
            .position(|i| matches!(&i.instr, Instruction::Return { .. }));
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
    fn defer_in_if_branch_uses_activation_flag() {
        // spec §12.3: a defer statement inside an if-branch should only
        // execute at function return when the if-body actually ran. The
        // lowerer pre-allocates a bool flag per defer site, initializes
        // it to false, sets it to true when the defer stmt is reached,
        // and guards emit_deferred behind a runtime check of that flag.
        let source = "\
fn cleanup() -> Unit\n\
  print(\"inner\")\n\
end\n\
fn work(_ flag: Bool) -> Unit\n\
  if flag\n\
    defer cleanup()\n\
    print(\"if-body\")\n\
  end\n\
  print(\"after-if\")\n\
end\n";
        let prog = lower_str(source);
        let work_fn = prog.functions.iter().find(|f| f.name == "work").unwrap();
        // Pre-allocated activation flag alloca at function start.
        let has_flag_alloca = work_fn.body.iter().any(
            |i| matches!(&i.instr, Instruction::Alloca { dest } if dest.starts_with(".defer_active_")),
        );
        assert!(
            has_flag_alloca,
            "expected .defer_active_N alloca at function entry"
        );
        // Initial store of 0 into the flag (false).
        let has_flag_init = work_fn.body.iter().any(|s| {
            matches!(
                &s.instr, Instruction::Store { dest, value: Operand::Const(Constant::Int(0)) }
                if dest.starts_with(".defer_active_"))
        });
        assert!(has_flag_init, "expected store 0 to .defer_active_N");
        // Inside the if body, store 1 into the flag to activate the defer.
        let has_flag_set = work_fn.body.iter().any(|s| {
            matches!(
                &s.instr, Instruction::Store { dest, value: Operand::Const(Constant::Int(1)) }
                if dest.starts_with(".defer_active_"))
        });
        assert!(
            has_flag_set,
            "expected store 1 to .defer_active_N inside if-body"
        );
        // Runtime guard at emit_deferred: the flag load must feed into a
        // BranchIf whose cond is a neq-zero compare against that load.
        // Without this wiring the old broken flat-Vec impl could satisfy
        // the alloca/store structural asserts above while still emitting
        // the deferred call unconditionally.
        let load_pos = work_fn.body.iter().position(|i| {
            matches!(&i.instr, Instruction::Load { source, .. } if source.starts_with(".defer_active_"))
        });
        assert!(load_pos.is_some(), "expected load of .defer_active_N");
        let load_idx = load_pos.unwrap();
        let Instruction::Load { dest: load_tmp, .. } = &work_fn.body[load_idx].instr else {
            unreachable!()
        };
        // Next few instructions should be: Const(0), BinOp Neq, BranchIf(active).
        let cmp = work_fn.body[load_idx + 1..]
            .iter()
            .take(5)
            .find_map(|s| match &s.instr {
                Instruction::BinOp {
                    dest, op, lhs, rhs, ..
                } if matches!(op, MirBinOp::NeqInt)
                    && matches!(lhs, Operand::Var(n) if n == load_tmp)
                    && matches!(rhs, Operand::Var(_)) =>
                {
                    Some(dest.clone())
                }
                _ => None,
            });
        assert!(
            cmp.is_some(),
            "expected NeqInt compare against the flag load"
        );
        let cmp_dest = cmp.unwrap();
        let branched_on_flag = work_fn.body.iter().any(
            |i| matches!(&i.instr, Instruction::BranchIf { cond: Operand::Var(c), .. } if c == &cmp_dest),
        );
        assert!(
            branched_on_flag,
            "expected BranchIf using the flag's compare result as cond"
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
        let has_into_call = outer_fn.body.iter().any(
            |s| matches!(&s.instr, Instruction::Call { func, .. } if func == "InnerErr__into"),
        );
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
        let has_into_call = outer_fn
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Call { func, .. } if func.contains("__into")));
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
        let calls_int = main
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "identity__Int"));
        let calls_str = main.body.iter().any(
            |s| matches!(&s.instr, Instruction::Call { func, .. } if func == "identity__String"),
        );
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
        let wrap_int = prog
            .functions
            .iter()
            .find(|f| f.name == "wrap__Int")
            .unwrap();
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
        let count = prog
            .functions
            .iter()
            .filter(|f| f.name == "id__Int")
            .count();
        assert_eq!(count, 1, "expected exactly 1 id__Int function, got {count}");
    }

    // ---- List<T> tests (§11) ----

    #[test]
    fn list_literal_lowers_to_list_init() {
        // spec §11: [1, 2, 3] produces ListInit with 3 elements
        let source = "let xs = [1, 2, 3]\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_list_init = main.body.iter().any(
            |inst| matches!(&inst.instr, Instruction::ListInit { elements, .. } if elements.len() == 3),
        );
        assert!(has_list_init, "expected ListInit with 3 elements");
    }

    #[test]
    fn list_literal_registers_struct_def() {
        // List<Int> should register a struct_def named "List__Int"
        let source = "let xs = [1, 2, 3]\n";
        let prog = lower_str(source);
        let has_list_def = prog.struct_defs.iter().any(|sd| sd.name == "List__Int");
        assert!(has_list_def, "expected struct def List__Int");
    }

    #[test]
    fn list_index_lowers_to_list_get() {
        // spec §11: xs[0] produces ListGet
        let source = "\
let xs = [10, 20, 30]\n\
let x = xs[1]\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_list_get = main
            .body
            .iter()
            .any(|inst| matches!(&inst.instr, Instruction::ListGet { .. }));
        assert!(has_list_get, "expected ListGet instruction");
    }

    #[test]
    fn list_get_method_lowers_to_list_get_safe() {
        // spec §11: xs.get(0) produces ListGetSafe
        let source = "\
let xs = [10, 20, 30]\n\
let y = xs.get(0)\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_list_get_safe = main
            .body
            .iter()
            .any(|inst| matches!(&inst.instr, Instruction::ListGetSafe { .. }));
        assert!(has_list_get_safe, "expected ListGetSafe instruction");
    }

    #[test]
    fn list_len_method_lowers_to_list_len() {
        // spec §11: xs.len() produces ListLen
        let source = "\
let xs = [1, 2, 3]\n\
let n = xs.len()\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_list_len = main
            .body
            .iter()
            .any(|inst| matches!(&inst.instr, Instruction::ListLen { .. }));
        assert!(has_list_len, "expected ListLen instruction");
    }

    #[test]
    fn list_param_registers_struct_def() {
        // fn f(_ items: List<Int>) should register List__Int
        let source = "\
fn f(_ items: List<Int>) -> Int\n\
  items.len()\n\
end\n";
        let prog = lower_str(source);
        let has_list_def = prog.struct_defs.iter().any(|sd| sd.name == "List__Int");
        assert!(has_list_def, "expected struct def List__Int for param type");
    }

    #[test]
    fn list_get_safe_registers_option_struct_def() {
        // .get() should also register Option__Int
        let source = "\
let xs = [1, 2, 3]\n\
let y = xs.get(0)\n";
        let prog = lower_str(source);
        let has_option_def = prog.struct_defs.iter().any(|sd| sd.name == "Option__Int");
        assert!(
            has_option_def,
            "expected struct def Option__Int from .get()"
        );
    }

    #[test]
    fn nested_list_with_empty_inner_picks_correct_elem_type() {
        // Regression: `let data: List<List<Int>> = [[1,2], []]` previously
        // typed the empty inner literal as List<List<Int>> (the outer
        // hint), tripping an LLVM struct-type mismatch in the outer list's
        // insertvalue. The hint must be peeled one level when recursing
        // into ListLit items.
        let source = "let data: List<List<Int>> = [[1, 2], []]\n";
        let prog = lower_str(source);
        // Both List__Int and List__List__Int should appear, and exactly one
        // ListInit at each level. The inner empty must lower with
        // elem_type Int, not List<Int>.
        let has_inner = prog.struct_defs.iter().any(|sd| sd.name == "List__Int");
        let has_outer = prog
            .struct_defs
            .iter()
            .any(|sd| sd.name == "List__List__Int");
        assert!(has_inner, "expected struct def List__Int");
        assert!(has_outer, "expected struct def List__List__Int");
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        // Find the empty ListInit and assert its elem_type is Int.
        let empty_elem_ty = main.body.iter().find_map(|inst| match &inst.instr {
            Instruction::ListInit {
                elements,
                elem_type,
                ..
            } if elements.is_empty() => Some(elem_type.clone()),
            _ => None,
        });
        assert_eq!(
            empty_elem_ty,
            Some(tyra_types::Ty::Int),
            "empty inner list should pick Int from the peeled List<List<Int>> hint"
        );
    }

    #[test]
    fn for_loop_over_list_generates_loop() {
        // for x in xs should generate BranchIf + ListGet for List iteration
        let source = "\
let xs = [10, 20, 30]\n\
for x in xs\n\
  println(x)\n\
end\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_branch = main
            .body
            .iter()
            .any(|inst| matches!(&inst.instr, Instruction::BranchIf { .. }));
        let has_list_get = main
            .body
            .iter()
            .any(|inst| matches!(&inst.instr, Instruction::ListGet { .. }));
        assert!(has_branch, "expected BranchIf in for-loop");
        assert!(has_list_get, "expected ListGet in for-loop body");
    }

    // ---- Comparison tests (§8.6, §11) ----

    #[test]
    fn string_eq_lowers_to_eq_string() {
        // String == String should use EqString, not EqInt
        let source = "\
let a = \"hello\"\n\
let b = \"world\"\n\
let eq = a == b\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_eq_string = main.body.iter().any(|inst| {
            matches!(
                &inst.instr,
                Instruction::BinOp {
                    op: MirBinOp::EqString,
                    ..
                }
            )
        });
        assert!(has_eq_string, "expected EqString for string equality");
    }

    #[test]
    fn value_type_lt_extracts_field() {
        // §8.6: single-field value type Ord → FieldGet + LtInt
        let source = "\
value UserId\n\
  id: Int\n\
end\n\
let id1 = UserId(id: 1)\n\
let id2 = UserId(id: 2)\n\
let cmp = id1 < id2\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_field_get = main.body.iter().any(
            |inst| matches!(&inst.instr, Instruction::FieldGet { type_name, .. } if type_name == "UserId"),
        );
        let has_lt_int = main.body.iter().any(|inst| {
            matches!(
                &inst.instr,
                Instruction::BinOp {
                    op: MirBinOp::LtInt,
                    ..
                }
            )
        });
        assert!(
            has_field_get,
            "expected FieldGet for UserId field extraction"
        );
        assert!(has_lt_int, "expected LtInt for field comparison");
    }

    #[test]
    fn value_type_eq_compares_all_fields() {
        // §8.6: multi-field value type Eq → FieldGet per field + EqInt + And
        let source = "\
value Pair\n\
  x: Int\n\
  y: Int\n\
end\n\
let a = Pair(x: 1, y: 2)\n\
let b = Pair(x: 1, y: 2)\n\
let eq = a == b\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let field_gets = main.body.iter().filter(|inst| {
            matches!(&inst.instr, Instruction::FieldGet { type_name, .. } if type_name == "Pair")
        }).count();
        let has_and = main.body.iter().any(|inst| {
            matches!(
                &inst.instr,
                Instruction::BinOp {
                    op: MirBinOp::And,
                    ..
                }
            )
        });
        assert!(
            field_gets >= 4,
            "expected at least 4 FieldGets (2 fields x 2 operands)"
        );
        assert!(has_and, "expected And to combine field comparisons");
    }

    #[test]
    fn match_string_literal_generates_comparison() {
        // match on string literal should generate EqString + BranchIf
        let source = "\
let cmd = \"serve\"\n\
match cmd\n\
when \"serve\"\n\
  println(\"ok\")\n\
when _\n\
  println(\"no\")\n\
end\n";
        let prog = lower_str(source);
        let main = prog.functions.iter().find(|f| f.is_main).unwrap();
        let has_eq_string = main.body.iter().any(|inst| {
            matches!(
                &inst.instr,
                Instruction::BinOp {
                    op: MirBinOp::EqString,
                    ..
                }
            )
        });
        assert!(
            has_eq_string,
            "expected EqString for string pattern matching"
        );
    }

    #[test]
    fn nested_adt_match_checks_inner_tag() {
        // Err(NotFound) vs Err(InvalidId) should generate separate inner tag checks
        let source = "\
type E =\n\
  | NotFound\n\
  | InvalidId\n\
fn f() -> Result<Int, E>\n\
  let r = Err(E.InvalidId)\n\
  match r\n\
  when Ok(v)\n\
    Ok(v)\n\
  when Err(NotFound)\n\
    Ok(0)\n\
  when Err(InvalidId)\n\
    Ok(1)\n\
  end\n\
end\n";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        // Should have at least 2 AdtPayload extractions (outer Ok/Err + inner variant checks)
        let payload_count = f
            .body
            .iter()
            .filter(|inst| matches!(&inst.instr, Instruction::AdtPayload { .. }))
            .count();
        assert!(
            payload_count >= 2,
            "expected at least 2 AdtPayload extractions for nested ADT match, got {payload_count}"
        );
    }

    #[test]
    fn while_in_function_has_jump_into_loop_header() {
        // Regression: the while-loop lowerer used to push the loop header
        // label without a preceding Jump, leaving the enclosing basic
        // block (allocas/stores from the function prologue) without a
        // terminator. LLVM verifier rejected the IR. The fix emits an
        // explicit Jump to the header before the Label.
        let source = "\
fn compute(_ n: Int) -> Int\n\
  mut sum = 0\n\
  mut i = 0\n\
  while i < n\n\
    sum = sum + i\n\
    i = i + 1\n\
  end\n\
  sum\n\
end\n";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "compute").unwrap();

        // Find index of the first loop-header Label.
        let header_idx = f
            .body
            .iter()
            .position(
                |i| matches!(&i.instr, Instruction::Label(name) if name.starts_with("while_")),
            )
            .expect("expected a while_* label");

        assert!(
            header_idx > 0,
            "header label cannot be the first instruction"
        );
        match &f.body[header_idx - 1].instr {
            Instruction::Jump { label } => {
                let Instruction::Label(header) = &f.body[header_idx].instr else {
                    unreachable!()
                };
                assert_eq!(
                    label, header,
                    "instruction before while header must jump to the header"
                );
            }
            other => panic!("expected Jump into while header, got {other:?}"),
        }
    }

    /// Regression guard for the M9 follow-up: `mut t = spawn f(); t.await`
    /// must emit an `Await` instruction. Without `task_result_types`
    /// propagation through Stmt::Mut + Ident-Load, `.await` silently
    /// fell through to identity and returned the raw task handle as the
    /// value — silent miscompilation (see lower/expr.rs).
    #[test]
    fn mut_spawn_await_lowers_with_await_instruction() {
        let source = "\
fn double(_ n: Int) -> Int
  n * 2
end
fn run() -> Int
  mut t = spawn double(21)
  t.await
end
";
        let prog = lower_str(source);
        let run = prog.functions.iter().find(|f| f.name == "run").unwrap();
        let has_await = run
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Await { .. }));
        assert!(
            has_await,
            "expected an Instruction::Await in run() body — task_result_types\n\
             propagation regressed? body = {:#?}",
            run.body
        );
    }

    /// M11 fix: `when Ok(xs)` where the Ok inner is a Named data type
    /// must register `var_types[xs]` so that downstream field access
    /// (`xs.field`) resolves to a proper FieldGet. Previously the arm
    /// populated only string_vars/float_vars, leaving Named / Generic
    /// payloads without a type, which produced bogus IR like
    /// `%t = add i64 %xs.field, 0`.
    #[test]
    fn match_ok_named_payload_registers_var_types() {
        let source = "\
data User
  id: Int
  name: String
end
fn fetch() -> Result<User, String>
  Ok(User(id: 1, name: \"alice\"))
end
fn run() -> Int
  match fetch()
  when Ok(u)
    u.id
  when Err(_)
    0
  end
end
";
        let prog = lower_str(source);
        let run = prog.functions.iter().find(|f| f.name == "run").unwrap();
        // The critical assertion: a FieldGet is emitted for u.id. Before
        // the fix, lowering fell through to a bogus Copy `{obj}.{field}`.
        let has_field_get = run.body.iter().any(|s| {
            matches!(
                &s.instr,
                Instruction::FieldGet { type_name, field_index: 0, .. }
                    if type_name == "User"
            )
        });
        assert!(
            has_field_get,
            "expected FieldGet for u.id after match-Ok-Named binding;\n\
             body = {:#?}",
            run.body
        );
    }

    /// Matching a Generic inner (List<Int> here) must also register the
    /// pattern-bound variable in generic_var_types so list operations
    /// like `xs[0]` / iteration see the proper element type.
    #[test]
    fn match_ok_generic_payload_registers_generic_var_types() {
        let source = "\
fn items() -> Result<List<Int>, String>
  Ok([1, 2, 3])
end
fn run() -> Int
  match items()
  when Ok(xs)
    xs[0]
  when Err(_)
    0
  end
end
";
        let prog = lower_str(source);
        let run = prog.functions.iter().find(|f| f.name == "run").unwrap();
        // ListGet is the proper lowering; without generic_var_types,
        // Index would fall back to a plain i64 default.
        let has_list_get = run
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::ListGet { .. }));
        assert!(
            has_list_get,
            "expected ListGet for xs[0] after match-Ok-List<Int> binding;\n\
             body = {:#?}",
            run.body
        );
    }

    /// `let t = match ... when 0 -> "" when _ -> "X" end` must register
    /// `t` as a String binding so subsequent interpolation `"#{t}..."`
    /// formats with `%s` rather than `%ld`. Before the fix,
    /// `is_string_expr` did not recurse into Match arms; the resulting
    /// printf used `%ld` and printed the raw string pointer as an int.
    #[test]
    fn match_of_string_arms_propagates_string_var() {
        let source = "\
fn f() -> String
  let t = match 0
  when 0
    \"\"
  when _
    \"X\"
  end
  \"#{t}IV\"
end
";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        let format_ref = f
            .body
            .iter()
            .find_map(|s| match &s.instr {
                Instruction::StringFormat { format_ref, .. } => Some(*format_ref),
                _ => None,
            })
            .expect("expected a StringFormat instruction");
        let format_str = &prog.string_constants[format_ref];
        assert!(
            format_str.contains("%s"),
            "expected %s for the match-of-strings binding `t`, got {format_str:?}"
        );
        assert!(
            !format_str.contains("%ld"),
            "expected no %ld; the only interp slot is the String binding `t`. got {format_str:?}"
        );
    }

    /// M11 phase 2 safety gate: passing a non-function value to
    /// `app.get` / `app.post` produces UB at request dispatch, because
    /// the stdlib types the handler slot as `String` (ptr) and the
    /// runtime casts that pointer to a function signature. The lowering
    /// must reject anything other than a known-function Ident.
    #[test]
    #[should_panic(expected = "http.server get() handler must be a top-level function name")]
    fn app_get_rejects_string_literal_handler() {
        // `import http.server` activates the safety gate. The stdlib
        // module body is not loaded by lower_str, but the import record
        // is enough to set imported_modules[\"server\"].
        let source = "\
import http.server
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  app.get(\"/foo\", \"not-a-function\")
  0
end
";
        let _ = lower_str(source);
    }

    #[test]
    #[should_panic(expected = "http.server get() handler must be a top-level function name")]
    fn app_get_rejects_shadowed_function_name() {
        // HIGH-severity regression guard from the phase 2 review: a
        // `let my_handler = "..."` that shadows a top-level fn of the
        // same name would previously pass the gate because
        // fn_return_types.contains_key(\"my_handler\") is true. The gate
        // now also checks var_types / mut_vars / pattern_vars.
        let source = "\
import http.server
data Request
  path: String
end
data Response
  status: Int
  body: String
end
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn my_handler(_ req: Request) -> Response
  Response(status: 200, body: \"ok\")
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  let my_handler = \"shadow\"
  app.get(\"/foo\", my_handler)
  0
end
";
        let _ = lower_str(source);
    }

    #[test]
    fn app_get_accepts_function_identifier_handler() {
        // Negative control: a bare function name resolves fine.
        let source = "\
import http.server
data Request
  path: String
end
data Response
  status: Int
  body: String
end
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn my_handler(_ req: Request) -> Response
  Response(status: 200, body: \"ok\")
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  app.get(\"/foo\", my_handler)
  0
end
";
        // Must not panic.
        let _ = lower_str(source);
    }

    /// Layout cross-check — Tyra side. The runtime's `stdlib_http_server.rs`
    /// pins `TyraRequest` / `TyraResponse` Rust layouts with `offset_of!`;
    /// this test is the authoritative Tyra counterpart. Read the real
    /// `stdlib/http/server.tyra` file, scan for the `data Request` and
    /// `data Response` blocks, and assert declaration order. A maintainer
    /// reordering stdlib fields (e.g. `body, path, method`) trips this
    /// test before the runtime silently reads swapped pointers.
    #[test]
    fn http_server_tyra_struct_field_order() {
        // Read the real stdlib/http/server.tyra file and scan its
        // `data Request` / `data Response` blocks for field order. A
        // maintainer editing the stdlib file to reorder fields must
        // update this test, which forces synchronous review of the
        // runtime-side TyraRequest/TyraResponse layout.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let stdlib_path =
            std::path::Path::new(manifest_dir).join("../../../stdlib/http/server.tyra");
        let src = std::fs::read_to_string(&stdlib_path).unwrap_or_else(|e| {
            panic!(
                "cannot read {}: {e} — layout test requires the real stdlib file",
                stdlib_path.display()
            )
        });

        let req_fields = extract_data_fields(&src, "Request");
        assert_eq!(
            req_fields,
            vec!["method", "path", "body"],
            "stdlib/http/server.tyra `data Request` field order must match \
             runtime TyraRequest {{ method, path, body }}"
        );
        let resp_fields = extract_data_fields(&src, "Response");
        assert_eq!(
            resp_fields,
            vec!["status", "body"],
            "stdlib/http/server.tyra `data Response` field order must match \
             runtime TyraResponse {{ status, body }}"
        );
    }

    /// Minimal scanner that pulls field names in declaration order out
    /// of a `data TypeName ... end` block. Assumes a well-formed stdlib
    /// file (no nested blocks, no multi-line field decls).
    ///
    /// Defenses:
    /// - Prefix false-match: walks `data ` occurrences and requires the
    ///   following word to equal `type_name` exactly, so a future
    ///   `data RequestBody` would not match a search for `Request`.
    /// - Comment-line skip: if a `#` appears anywhere in the line
    ///   before `abs`, the match is inside a comment (whether leading
    ///   or trailing). Skipped.
    fn extract_data_fields(src: &str, type_name: &str) -> Vec<String> {
        let mut search_pos = 0;
        let start = loop {
            let remaining = &src[search_pos..];
            let rel = remaining
                .find("data ")
                .unwrap_or_else(|| panic!("data block `{type_name}` not found"));
            let abs = search_pos + rel;
            let after = &src[abs + 5..]; // skip "data "
            let word_end = after
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            let word = &after[..word_end];
            // Reject matches inside a comment — detected by a `#`
            // appearing on the same line before `abs` (covers both
            // leading-comment and trailing-comment cases).
            let line_start = src[..abs].rfind('\n').map_or(0, |i| i + 1);
            let line_prefix = &src[line_start..abs];
            let in_comment = line_prefix.contains('#');
            if word == type_name && !in_comment {
                break abs;
            }
            search_pos = abs + 5 + word_end;
        };
        let rest = &src[start + 5 + type_name.len()..];
        let end_rel = rest
            .find("\nend")
            .unwrap_or_else(|| panic!("missing `end` after data {type_name}"));
        let block = &rest[..end_rel];
        block
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    return None;
                }
                trimmed.split_once(':').map(|(n, _)| n.trim().to_string())
            })
            .collect()
    }

    /// Aliased `import http.server as srv` must still activate the gate;
    /// the canonical path `"http.server"` is recorded alongside the
    /// alias in imported_modules for exactly this reason.
    #[test]
    #[should_panic(expected = "http.server get() handler must be a top-level function name")]
    fn app_get_gate_fires_under_aliased_import() {
        let source = "\
import http.server as srv
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  app.get(\"/foo\", \"not-a-fn\")
  0
end
";
        let _ = lower_str(source);
    }

    /// Function parameter shadow: `fn setup(_ my_handler: String)` must
    /// not let a param name of the same shape as a top-level fn slip
    /// through the gate. Params are registered in `local_binding_names`
    /// at `lower_fn` entry for exactly this reason.
    #[test]
    #[should_panic(expected = "http.server get() handler must be a top-level function name")]
    fn app_get_rejects_function_param_shadow() {
        let source = "\
import http.server
data Request
  path: String
end
data Response
  status: Int
  body: String
end
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn my_handler(_ req: Request) -> Response
  Response(status: 200, body: \"ok\")
end
fn register(_ app: AppServer, _ my_handler: String) -> Unit
  app.get(\"/foo\", my_handler)
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  register(app, \"shadow-value\")
  0
end
";
        let _ = lower_str(source);
    }

    /// For-loop induction variable with Int element type: a shadow must
    /// still be rejected even though `Ty::Int` doesn't populate any
    /// type-keyed tracking map (we maintain `local_binding_names` as the
    /// authoritative shadow set).
    #[test]
    #[should_panic(expected = "http.server get() handler must be a top-level function name")]
    fn app_get_rejects_for_loop_int_shadow() {
        let source = "\
import http.server
data Request
  path: String
end
data Response
  status: Int
  body: String
end
data AppServer
  _handle: Int
end
impl AppServerOps for AppServer
  fn get(self, _ path: String, _ handler: String) -> Unit
    ()
  end
end
fn my_handler(_ req: Request) -> Response
  Response(status: 200, body: \"ok\")
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  for my_handler in [1, 2, 3]
    app.get(\"/foo\", my_handler)
  end
  0
end
";
        let _ = lower_str(source);
    }

    #[test]
    fn app_get_gate_skipped_when_http_server_not_imported() {
        // User defines their own unrelated AppServer with get(String, String)
        // signature. Should compile fine — the gate must not fire because
        // http.server is not imported.
        let source = "\
data AppServer
  _handle: Int
end
impl UserOps for AppServer
  fn get(self, _ key: String, _ val: String) -> Unit
    ()
  end
end
fn main() -> Int
  let app = AppServer(_handle: 0)
  app.get(\"key\", \"val\")
  0
end
";
        // Must not panic — no http.server import, so the gate is dormant.
        let _ = lower_str(source);
    }

    /// Regression guard for Assign-over-mut-task-handle: `mut t = spawn f();
    /// t = spawn g(); t.await` should still unbox via Await. Without
    /// propagation in ExprKind::Assign, the second spawn's tracking would
    /// be lost.
    #[test]
    fn mut_spawn_reassign_await_still_unboxes() {
        let source = "\
fn double(_ n: Int) -> Int
  n * 2
end
fn run() -> Int
  mut t = spawn double(1)
  t = spawn double(21)
  t.await
end
";
        let prog = lower_str(source);
        let run = prog.functions.iter().find(|f| f.name == "run").unwrap();
        let has_await = run
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::Await { .. }));
        assert!(
            has_await,
            "expected Await after reassign; body = {:#?}",
            run.body
        );
    }

    /// Regression: `when Some(literal)` should emit a payload value comparison
    /// after the tag check. Previously the payload Store was emitted with empty
    /// destination (E0500), and the literal value was never compared.
    #[test]
    fn some_literal_pattern_emits_payload_comparison() {
        let source = "\
fn check(_ opt: Option<Int>) -> Int
  match opt
  when Some(42)
    1
  when _
    0
  end
end
";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "check").unwrap();
        // Must emit at least one AdtPayload (extract payload for comparison)
        // and at least one Const { value: Int(42) } for the literal check.
        let has_payload = f
            .body
            .iter()
            .any(|s| matches!(&s.instr, Instruction::AdtPayload { .. }));
        let has_lit_42 = f.body.iter().any(|s| {
            matches!(
                s.instr,
                Instruction::Const {
                    value: crate::ir::Constant::Int(42),
                    ..
                }
            )
        });
        assert!(
            has_payload,
            "expected AdtPayload in Some(42) arm; body = {:#?}",
            f.body
        );
        assert!(
            has_lit_42,
            "expected Const(42) for literal comparison; body = {:#?}",
            f.body
        );
    }

    /// Regression: `when Err(_) -> fallback` must store `fallback` into the
    /// match result slot. Previously `rename_pattern_bindings` renamed `_` to
    /// `___pN`, defeating the `field_name != "_"` guard, causing a spurious
    /// payload Store that `block_ends_with_assignment` then misidentified as a
    /// user assignment, skipping the arm-result Store entirely.
    #[test]
    fn match_err_wildcard_arm_stores_result() {
        let source = "\
fn get(_ n: Int) -> Result<Int, Int>
  if n > 0
    Err(1)
  else
    Ok(n)
  end
end
fn f() -> Int
  let fallback = 99
  let r = get(500)
  match r
  when Ok(v)
    v
  when Err(_)
    fallback
  end
end
";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        // After Fix A the wildcard `_` is not renamed; no Store { dest: "___pN" }
        // should appear (dest starts with "___p" followed by digits).
        let spurious_wildcard_store = f.body.iter().any(|i| {
            if let Instruction::Store { dest, .. } = &i.instr {
                dest == "_"
                    || (dest.starts_with("___p") && dest[4..].chars().all(|c| c.is_ascii_digit()))
            } else {
                false
            }
        });
        assert!(
            !spurious_wildcard_store,
            "wildcard `_` payload must not be stored; body = {:#?}",
            f.body
        );
        // After Fix B the arm-result Store for `fallback` must be emitted.
        let fallback_stored = f.body.iter().any(
            |i| matches!(&i.instr, Instruction::Store { value: Operand::Var(v), .. } if v == "fallback"),
        );
        assert!(
            fallback_stored,
            "`fallback` must be stored into the match result slot; body = {:#?}",
            f.body
        );
    }

    /// Regression (Fix B — general case): `when Err(e) -> fallback` where the
    /// arm body is a bare ident unrelated to the payload must also store
    /// `fallback`. Before Fix B, `Store { dest: "e__pN" }` was the last
    /// instruction in the arm range and was misidentified as a user assignment.
    #[test]
    fn match_named_payload_arm_stores_unrelated_result() {
        let source = "\
fn get(_ n: Int) -> Result<Int, Int>
  if n > 0
    Err(1)
  else
    Ok(n)
  end
end
fn f() -> Int
  let fallback = 99
  let r = get(500)
  match r
  when Ok(v)
    v
  when Err(e)
    fallback
  end
end
";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        // `fallback` must be stored into the match result slot.
        let fallback_stored = f.body.iter().any(
            |i| matches!(&i.instr, Instruction::Store { value: Operand::Var(v), .. } if v == "fallback"),
        );
        assert!(
            fallback_stored,
            "`fallback` must be stored into match result (named payload arm); body = {:#?}",
            f.body
        );
    }

    // ── Source-location tests (ADR 0014 / Phase 1) ──────────────────────

    /// A top-level `panic()` call must carry the line of the panic() expression,
    /// not line 0 or some surrounding statement's line.
    #[test]
    fn panic_call_loc_is_its_own_line() {
        // Line 1: let x = 0
        // Line 2: panic("boom")
        let source = "let x = 0\npanic(\"boom\")\n";
        let prog = lower_str(source);
        let main = &prog.functions[0];
        let panic_stmt = main
            .body
            .iter()
            .find(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "panic"));
        let stmt = panic_stmt.expect("expected a Call to panic in MIR");
        assert_eq!(
            stmt.loc.line, 2,
            "panic() on line 2 should carry line=2, got loc={:?}",
            stmt.loc
        );
    }

    /// `panic()` nested inside an if-arm must carry the panic line, not the
    /// if-expression line.  This is the key regression: before Phase 1 the
    /// parent statement's loc (the `if`) was used for all instructions in the
    /// arm, so the wrong line was reported on stderr.
    #[test]
    fn panic_inside_if_arm_loc_is_panic_line() {
        // Line 1: fn f
        // Line 2:   if true
        // Line 3:     panic("inner")
        // Line 4:   else
        // Line 5:     0
        // Line 6:   end
        // Line 7: end
        let source = "fn f() -> Int\n  if true\n    panic(\"inner\")\n  else\n    0\n  end\nend\n";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        let panic_stmt = f
            .body
            .iter()
            .find(|s| matches!(&s.instr, Instruction::Call { func, .. } if func == "panic"));
        let stmt = panic_stmt.expect("expected a Call to panic in MIR body of f");
        assert_eq!(
            stmt.loc.line, 3,
            "panic() on line 3 (inside if-arm) should carry line=3, got loc={:?}",
            stmt.loc
        );
    }

    /// Compiler-synthesised control-flow instructions (Label, Jump, BranchIf,
    /// result-slot Alloca) must carry a dummy SourceLoc so they never contribute
    /// spurious line information to DWARF or coverage.
    #[test]
    fn synthetic_cfg_instructions_have_dummy_loc() {
        // A simple if/else forces Label + BranchIf + Jump + Alloca into the MIR.
        let source = "fn f(_ x: Int) -> Int\n  if x > 0\n    1\n  else\n    0\n  end\nend\n";
        let prog = lower_str(source);
        let f = prog.functions.iter().find(|f| f.name == "f").unwrap();
        for stmt in &f.body {
            let is_synthetic = matches!(
                &stmt.instr,
                Instruction::Label(_)
                    | Instruction::Jump { .. }
                    | Instruction::BranchIf { .. }
                    | Instruction::Alloca { .. }
            );
            if is_synthetic {
                assert!(
                    stmt.loc.is_dummy(),
                    "synthetic {:?} should have dummy loc, got {:?}",
                    stmt.instr,
                    stmt.loc
                );
            }
        }
    }
}
