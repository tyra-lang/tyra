// tyra-mir: Mid-level IR for the Tyra language.
// Desugars AST into a flat instruction sequence for codegen.
//
// Current scope (Milestone 1a): basic expressions, function calls, if/while.
// Full match lowering, closures, and async are deferred.

pub mod ir;
pub mod lower;

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
}
