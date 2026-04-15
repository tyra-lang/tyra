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
}
