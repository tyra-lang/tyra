// tyra-codegen-llvm: Generate LLVM IR text from MIR.
//
// Milestone 1a approach: generate LLVM IR as text (.ll file),
// then compile with clang. No LLVM library dependency needed.
// Can be upgraded to inkwell for direct LLVM API access later.

mod codegen;

pub use codegen::emit_llvm_ir;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world_ir() {
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::StringRef(0),
                    },
                    tyra_mir::Instruction::Call {
                        dest: Some("_t1".into()),
                        func: "print".into(),
                        args: vec![tyra_mir::Operand::Var("_t0".into())],
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec!["hello, tyra".into()],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@.str.0"));
        assert!(ir.contains("hello, tyra"));
        assert!(ir.contains("define i32 @main()"));
        assert!(ir.contains("call"));
        assert!(ir.contains("ret i32 0"));
    }

    #[test]
    fn fn_with_int_params() {
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "add".into(),
                params: vec![
                    ("x".into(), tyra_types::Ty::Int),
                    ("y".into(), tyra_types::Ty::Int),
                ],
                return_type: tyra_types::Ty::Int,
                body: vec![
                    tyra_mir::Instruction::BinOp {
                        dest: "_t0".into(),
                        op: tyra_mir::MirBinOp::AddInt,
                        lhs: tyra_mir::Operand::Var("x".into()),
                        rhs: tyra_mir::Operand::Var("y".into()),
                    },
                    tyra_mir::Instruction::Return {
                        value: Some(tyra_mir::Operand::Var("_t0".into())),
                    },
                ],
                is_main: false,
            }],
            string_constants: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("define i64 @add(i64 %x, i64 %y)"));
        assert!(ir.contains("add i64"));
        assert!(ir.contains("ret i64"));
    }

    #[test]
    fn multiple_string_constants() {
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![tyra_mir::Instruction::Return { value: None }],
                is_main: true,
            }],
            string_constants: vec!["hello".into(), "world".into()],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@.str.0"));
        assert!(ir.contains("@.str.1"));
        assert!(ir.contains("hello"));
        assert!(ir.contains("world"));
    }
}
