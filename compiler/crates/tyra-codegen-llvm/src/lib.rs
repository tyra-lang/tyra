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
            struct_defs: vec![],
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
            struct_defs: vec![],
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
            struct_defs: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@.str.0"));
        assert!(ir.contains("@.str.1"));
        assert!(ir.contains("hello"));
        assert!(ir.contains("world"));
    }

    #[test]
    fn struct_type_declaration() {
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![tyra_mir::Instruction::Return { value: None }],
                is_main: true,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Point".into(),
                fields: vec![
                    ("x".into(), tyra_types::Ty::Float),
                    ("y".into(), tyra_types::Ty::Float),
                ],
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.Point = type { double, double }"));
    }

    #[test]
    fn struct_init_and_field_get() {
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::Int(10),
                    },
                    tyra_mir::Instruction::Const {
                        dest: "_t1".into(),
                        value: tyra_mir::Constant::Int(20),
                    },
                    tyra_mir::Instruction::StructInit {
                        dest: "_t2".into(),
                        type_name: "Pair".into(),
                        fields: vec![
                            tyra_mir::Operand::Var("_t0".into()),
                            tyra_mir::Operand::Var("_t1".into()),
                        ],
                    },
                    tyra_mir::Instruction::FieldGet {
                        dest: "_t3".into(),
                        obj: tyra_mir::Operand::Var("_t2".into()),
                        type_name: "Pair".into(),
                        field_index: 0,
                    },
                    tyra_mir::Instruction::FieldGet {
                        dest: "_t4".into(),
                        obj: tyra_mir::Operand::Var("_t2".into()),
                        type_name: "Pair".into(),
                        field_index: 1,
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![
                    ("first".into(), tyra_types::Ty::Int),
                    ("second".into(), tyra_types::Ty::Int),
                ],
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.Pair = type { i64, i64 }"));
        assert!(ir.contains("insertvalue %struct.Pair undef, i64 %_t0, 0"));
        assert!(ir.contains("insertvalue %struct.Pair %_t2.s0, i64 %_t1, 1"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 0"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 1"));
    }
}
