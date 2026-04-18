// tyra-codegen-llvm: Generate LLVM IR text from MIR.
//
// Milestone 1a approach: generate LLVM IR as text (.ll file),
// then compile with clang. No LLVM library dependency needed.
// Can be upgraded to inkwell for direct LLVM API access later.

mod builtins;
mod codegen;
mod list_codegen;

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
        assert!(ir.contains("define i32 @main(i32 %argc, ptr %argv)"));
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
                is_data: false,
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
                is_data: false,
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.Pair = type { i64, i64 }"));
        assert!(ir.contains("insertvalue %struct.Pair undef, i64 %_t0, 0"));
        assert!(ir.contains("insertvalue %struct.Pair %_t2.s0, i64 %_t1, 1"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 0"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 1"));
    }

    #[test]
    fn list_init_emits_malloc_and_stores() {
        // §11: ListInit should emit malloc, GEP+store per element, insertvalue
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
                    tyra_mir::Instruction::ListInit {
                        dest: "_t2".into(),
                        elem_type: tyra_types::Ty::Int,
                        elements: vec![
                            tyra_mir::Operand::Var("_t0".into()),
                            tyra_mir::Operand::Var("_t1".into()),
                        ],
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "List__Int".into(),
                fields: vec![
                    ("data".into(), tyra_types::Ty::String), // ptr
                    ("len".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.List__Int = type { ptr, i64 }"));
        assert!(ir.contains("@malloc(i64 16)")); // 2 elements * 8 bytes
        assert!(ir.contains("getelementptr i64"));
        assert!(ir.contains("insertvalue %struct.List__Int"));
    }

    #[test]
    fn list_get_emits_bounds_check() {
        // §11: ListGet should emit bounds check with icmp + abort branch
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
                    tyra_mir::Instruction::ListInit {
                        dest: "_t1".into(),
                        elem_type: tyra_types::Ty::Int,
                        elements: vec![tyra_mir::Operand::Var("_t0".into())],
                    },
                    tyra_mir::Instruction::Const {
                        dest: "_t2".into(),
                        value: tyra_mir::Constant::Int(0),
                    },
                    tyra_mir::Instruction::ListGet {
                        dest: "_t3".into(),
                        list: tyra_mir::Operand::Var("_t1".into()),
                        index: tyra_mir::Operand::Var("_t2".into()),
                        elem_type: tyra_types::Ty::Int,
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "List__Int".into(),
                fields: vec![
                    ("data".into(), tyra_types::Ty::String),
                    ("len".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("icmp ult i64"), "expected bounds check");
        assert!(ir.contains("call void @abort()"), "expected abort on OOB");
        assert!(ir.contains("load i64"), "expected load from list data");
    }

    #[test]
    fn eq_string_emits_strcmp() {
        // EqString should emit call @strcmp + icmp eq
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
                    tyra_mir::Instruction::Const {
                        dest: "_t1".into(),
                        value: tyra_mir::Constant::StringRef(1),
                    },
                    tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::EqString,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec!["hello".into(), "world".into()],
            struct_defs: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@strcmp(ptr"), "expected strcmp call");
        assert!(ir.contains("icmp eq i32"), "expected icmp eq on strcmp result");
    }

    #[test]
    fn panic_emits_puts_abort_unreachable() {
        // §12.1: panic(msg) → puts(msg) + abort + unreachable
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
                        dest: None,
                        func: "panic".into(),
                        args: vec![tyra_mir::Operand::Var("_t0".into())],
                    },
                ],
                is_main: true,
            }],
            string_constants: vec!["oops".into()],
            struct_defs: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@puts(ptr"), "expected puts call for panic message");
        assert!(ir.contains("call void @abort()"), "expected abort after panic");
        assert!(ir.contains("unreachable"), "expected unreachable after abort");
    }

    #[test]
    fn data_type_struct_init_uses_malloc() {
        // §8.6: data types are heap-allocated reference types
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::Instruction::StructInit {
                        dest: "user".into(),
                        type_name: "User".into(),
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::Int(1))],
                    },
                    tyra_mir::Instruction::Return { value: None },
                ],
                is_main: true,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "User".into(),
                fields: vec![("id".into(), tyra_types::Ty::Int)],
                is_data: true,
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("call ptr @malloc"), "data StructInit must use malloc");
        assert!(ir.contains("getelementptr %struct.User"), "must use GEP to init fields");
        assert!(!ir.contains("insertvalue"), "data types must not use insertvalue");
    }

    #[test]
    fn data_type_field_get_uses_gep_load() {
        // §8.6: field access on data type uses GEP + load, not extractvalue
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "get_id".into(),
                params: vec![("user".into(), tyra_types::Ty::Named("User".into()))],
                return_type: tyra_types::Ty::Int,
                body: vec![
                    tyra_mir::Instruction::FieldGet {
                        dest: "id_val".into(),
                        obj: tyra_mir::Operand::Var("user".into()),
                        type_name: "User".into(),
                        field_index: 0,
                    },
                    tyra_mir::Instruction::Return {
                        value: Some(tyra_mir::Operand::Var("id_val".into())),
                    },
                ],
                is_main: false,
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "User".into(),
                fields: vec![("id".into(), tyra_types::Ty::Int)],
                is_data: true,
            }],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("getelementptr %struct.User"), "FieldGet on data must use GEP");
        assert!(ir.contains("load i64"), "FieldGet on data must load the field");
        assert!(!ir.contains("extractvalue"), "data FieldGet must not use extractvalue");
        assert!(ir.contains("define i64 @get_id(ptr %user)"), "data type param must be ptr");
    }
}
