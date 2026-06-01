// tyra-codegen-llvm: Generate LLVM IR text from MIR.
//
// Milestone 1a approach: generate LLVM IR as text (.ll file),
// then compile with clang. No LLVM library dependency needed.
// Can be upgraded to inkwell for direct LLVM API access later.

mod builtins;
mod codegen;
pub mod coverage;
pub mod dwarf;
mod helpers;
mod inkwell_builtins;
mod inkwell_codegen;
mod inkwell_instr;
mod inkwell_list;
mod instr_emit;
mod list_codegen;
mod type_scan;

pub use codegen::{emit_llvm_ir, emit_llvm_ir_coverage, emit_llvm_ir_debug};
pub use coverage::{CovMap, format_report, merge_covraw, parse_covmap, write_covmap_text};

use tyra_diagnostics::Diagnostic;
use tyra_mir::{Instruction, Program};
use tyra_types::Ty;

/// Guard: `Ty::Error` or an unresolved `Ty::Var` must never reach codegen.
///
/// Walks the MIR Program looking for unresolved types in function signatures,
/// struct definitions, local metadata, and instruction operands.  If any are
/// found, returns a `Vec<Diagnostic>` containing one or more E9001 entries
/// (Internal Compiler Error).  Returning `Err` keeps LLVM from crashing on
/// malformed IR and presents the user with a normal compiler error instead of
/// a Rust panic / backtrace.
///
/// Why this is an ICE (E9001) and not a regular error:
///   By the time MIR is generated, the type checker should have either fully
///   resolved every type or emitted a user-facing diagnostic (E0xxx) and
///   refused to proceed.  An unresolved type at this stage therefore signals
///   a checker bug, not user error.
pub fn check_no_type_errors(program: &Program) -> Result<(), Vec<Diagnostic>> {
    let mut diags: Vec<Diagnostic> = Vec::new();

    fn push_ice(ctx: &str, ty: &Ty, diags: &mut Vec<Diagnostic>) {
        if has_unresolved(ty) {
            diags.push(
                Diagnostic::error(format!(
                    "internal compiler error: unresolved type reached code generation \
                     ({ctx}: `{}`)",
                    ty.display_name()
                ))
                .with_code("E9001")
                .with_help(
                    "This is a compiler bug. Please report at \
                     https://github.com/tyra-lang/tyra/issues",
                ),
            );
        }
    }

    for sd in &program.struct_defs {
        for (fname, fty) in &sd.fields {
            push_ice(
                &format!("struct `{}` field `{}`", sd.name, fname),
                fty,
                &mut diags,
            );
        }
    }

    for func in &program.functions {
        for (pname, pty) in &func.params {
            push_ice(
                &format!("function `{}` parameter `{}`", func.name, pname),
                pty,
                &mut diags,
            );
        }
        push_ice(
            &format!("function `{}` return type", func.name),
            &func.return_type,
            &mut diags,
        );
        for lm in &func.local_metas {
            push_ice(
                &format!("function `{}` local `{}`", func.name, lm.name),
                &lm.ty,
                &mut diags,
            );
        }
        for stmt in &func.body {
            for ty in instruction_types(&stmt.instr) {
                push_ice(
                    &format!("function `{}` instruction", func.name),
                    ty,
                    &mut diags,
                );
            }
        }
    }

    if diags.is_empty() { Ok(()) } else { Err(diags) }
}

/// Recursively check whether `ty` contains a `Ty::Error` or an unresolved
/// `Ty::Var` placeholder.
fn has_unresolved(ty: &Ty) -> bool {
    match ty {
        Ty::Error | Ty::Var(_) => true,
        Ty::Generic(_, args) => args.iter().any(has_unresolved),
        Ty::Fn(params, ret) => params.iter().any(has_unresolved) || has_unresolved(ret),
        _ => false,
    }
}

/// Extract every `Ty` carried by a single MIR `Instruction`.
///
/// Exhaustive over all instruction variants that embed a `Ty`. When a new
/// instruction variant with a `Ty` field is added to `ir.rs`, this function
/// must be updated — the explicit wildcard-free match below ensures the
/// compiler will flag the omission.
fn instruction_types(instr: &Instruction) -> Vec<&Ty> {
    match instr {
        Instruction::PtrLoad { ty, .. } => vec![ty],

        Instruction::ListInit { elem_type, .. }
        | Instruction::ListGet { elem_type, .. }
        | Instruction::ListGetSafe { elem_type, .. }
        | Instruction::ListPush { elem_type, .. }
        | Instruction::JoinAll { elem_type, .. }
        | Instruction::Select { elem_type, .. } => vec![elem_type],

        Instruction::MapGetOption { key_ty, val_ty, .. }
        | Instruction::LinkedMapGetOption { key_ty, val_ty, .. } => vec![key_ty, val_ty],

        Instruction::Spawn {
            arg_types,
            result_type,
            ..
        } => {
            let mut tys: Vec<&Ty> = arg_types.iter().collect();
            tys.push(result_type);
            tys
        }

        Instruction::Await { result_type, .. } => vec![result_type],

        Instruction::ClosureBuild {
            param_types,
            return_type,
            ..
        } => {
            let mut tys: Vec<&Ty> = param_types.iter().collect();
            tys.push(return_type);
            tys
        }

        Instruction::IndirectCall {
            param_types,
            return_type,
            ..
        } => {
            let mut tys: Vec<&Ty> = param_types.iter().collect();
            tys.push(return_type);
            tys
        }

        // No embedded Ty fields:
        Instruction::Const { .. }
        | Instruction::Call { .. }
        | Instruction::BinOp { .. }
        | Instruction::Neg { .. }
        | Instruction::Not { .. }
        | Instruction::Copy { .. }
        | Instruction::Return { .. }
        | Instruction::Label { .. }
        | Instruction::BranchIf { .. }
        | Instruction::Jump { .. }
        | Instruction::Phi { .. }
        | Instruction::Alloca { .. }
        | Instruction::Store { .. }
        | Instruction::Load { .. }
        | Instruction::StructInit { .. }
        | Instruction::FieldGet { .. }
        | Instruction::FieldSet { .. }
        | Instruction::AdtInit { .. }
        | Instruction::AdtTag { .. }
        | Instruction::AdtPayload { .. }
        | Instruction::StringFormat { .. }
        | Instruction::ListLen { .. }
        | Instruction::MapForEachCall { .. }
        | Instruction::SetForEachCall { .. }
        | Instruction::LinkedMapForEachCall { .. }
        | Instruction::LinkedSetForEachCall { .. } => Vec::new(),
    }
}

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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::StringRef(0),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Call {
                        dest: Some("_t1".into()),
                        func: "print".into(),
                        args: vec![tyra_mir::Operand::Var("_t0".into())],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["hello, tyra".into()],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@.str.0"));
        assert!(ir.contains("hello, tyra"));
        assert!(ir.contains("define i32 @main(i32 %argc, ptr %argv)"));
        assert!(ir.contains("call"));
        assert!(ir.contains("ret i32 0"));
        // ADR-0007: main must initialize Boehm GC before any allocation.
        assert!(
            ir.contains("call void @GC_init()"),
            "main must invoke GC_init at entry (ADR-0007)"
        );
        assert!(
            ir.contains("declare void @GC_init()"),
            "GC_init extern declaration must be emitted"
        );
        assert!(
            ir.contains("declare ptr @GC_malloc(i64)"),
            "GC_malloc extern declaration must be emitted"
        );
        // M9: main must also initialize the Tyra async runtime.
        assert!(
            ir.contains("call void @tyra_rt_init()"),
            "main must invoke tyra_rt_init at entry (M9)"
        );
        assert!(
            ir.contains("declare void @tyra_rt_init()"),
            "tyra_rt_init extern declaration must be emitted"
        );
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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t0".into(),
                        op: tyra_mir::MirBinOp::AddInt,
                        lhs: tyra_mir::Operand::Var("x".into()),
                        rhs: tyra_mir::Operand::Var("y".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return {
                        value: Some(tyra_mir::Operand::Var("_t0".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
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
                body: vec![tyra_mir::MirStmt::synthetic(
                    tyra_mir::Instruction::Return { value: None },
                )],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["hello".into(), "world".into()],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
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
                body: vec![tyra_mir::MirStmt::synthetic(
                    tyra_mir::Instruction::Return { value: None },
                )],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Point".into(),
                fields: vec![
                    ("x".into(), tyra_types::Ty::Float),
                    ("y".into(), tyra_types::Ty::Float),
                ],
                is_data: false,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::Int(10),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t1".into(),
                        value: tyra_mir::Constant::Int(20),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::StructInit {
                        dest: "_t2".into(),
                        type_name: "Pair".into(),
                        fields: vec![
                            tyra_mir::Operand::Var("_t0".into()),
                            tyra_mir::Operand::Var("_t1".into()),
                        ],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::FieldGet {
                        dest: "_t3".into(),
                        obj: tyra_mir::Operand::Var("_t2".into()),
                        type_name: "Pair".into(),
                        field_index: 0,
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::FieldGet {
                        dest: "_t4".into(),
                        obj: tyra_mir::Operand::Var("_t2".into()),
                        type_name: "Pair".into(),
                        field_index: 1,
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![
                    ("first".into(), tyra_types::Ty::Int),
                    ("second".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.Pair = type { i64, i64 }"));
        assert!(ir.contains("insertvalue %struct.Pair undef, i64 %_t0, 0"));
        assert!(ir.contains("insertvalue %struct.Pair %_t2.s0, i64 %_t1, 1"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 0"));
        assert!(ir.contains("extractvalue %struct.Pair %_t2, 1"));
    }

    #[test]
    fn list_init_emits_gc_malloc_and_stores() {
        // §11: ListInit should emit GC_malloc, GEP+store per element, insertvalue
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::Int(10),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t1".into(),
                        value: tyra_mir::Constant::Int(20),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::ListInit {
                        dest: "_t2".into(),
                        elem_type: tyra_types::Ty::Int,
                        elements: vec![
                            tyra_mir::Operand::Var("_t0".into()),
                            tyra_mir::Operand::Var("_t1".into()),
                        ],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "List__Int".into(),
                fields: vec![
                    ("data".into(), tyra_types::Ty::String), // ptr
                    ("len".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("%struct.List__Int = type { ptr, i64 }"));
        assert!(ir.contains("@GC_malloc(i64 16)")); // 2 elements * 8 bytes
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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::Int(10),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::ListInit {
                        dest: "_t1".into(),
                        elem_type: tyra_types::Ty::Int,
                        elements: vec![tyra_mir::Operand::Var("_t0".into())],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t2".into(),
                        value: tyra_mir::Constant::Int(0),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::ListGet {
                        dest: "_t3".into(),
                        list: tyra_mir::Operand::Var("_t1".into()),
                        index: tyra_mir::Operand::Var("_t2".into()),
                        elem_type: tyra_types::Ty::Int,
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "List__Int".into(),
                fields: vec![
                    ("data".into(), tyra_types::Ty::String),
                    ("len".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::StringRef(0),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t1".into(),
                        value: tyra_mir::Constant::StringRef(1),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::EqString,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["hello".into(), "world".into()],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(ir.contains("@strcmp(ptr"), "expected strcmp call");
        assert!(
            ir.contains("icmp eq i32"),
            "expected icmp eq on strcmp result"
        );
    }

    #[test]
    fn eq_option_int_compares_payload() {
        // Option<Int> == Option<Int> must compare both tag (field 0) AND
        // payload (field 1), not just the tag. Before the fix, the codegen
        // only extracted field 0 and returned early, making Some(5)==Some(99)
        // emit `true` (both tags are 0).
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    // _t0 = Some(5)
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t0".into(),
                        type_name: "Option__Int".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::Int(5))],
                    }),
                    // _t1 = Some(99)
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t1".into(),
                        type_name: "Option__Int".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::Int(99))],
                    }),
                    // _t2 = _t0 == _t1
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::EqInt,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Option__Int".into(),
                // fields[0] named "tag" → is_adt=true; fields[1] = payload
                fields: vec![
                    ("tag".into(), tyra_types::Ty::Int),
                    ("0".into(), tyra_types::Ty::Int),
                ],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        // Must extract field 1 (payload), not only field 0 (tag)
        assert!(
            ir.contains("extractvalue %struct.Option__Int") && ir.contains(", 1"),
            "expected field-1 extractvalue for payload comparison; IR:\n{ir}"
        );
        // Must AND the per-field equalities together
        assert!(
            ir.contains("and i1"),
            "expected `and i1` from structural field-by-field compare; IR:\n{ir}"
        );
    }

    #[test]
    fn eq_option_string_compares_payload() {
        // Option<String> == Option<String> must compare the String payload via
        // null-safe strcmp, not just the tag. Before the fix, the ptr payload
        // caused a tag-only fallback, making Some("a")==Some("b") emit true.
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t0".into(),
                        type_name: "Option__String".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(0))],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t1".into(),
                        type_name: "Option__String".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(1))],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::EqInt,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["a".into(), "b".into()],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Option__String".into(),
                fields: vec![
                    ("tag".into(), tyra_types::Ty::Int),
                    ("0".into(), tyra_types::Ty::String),
                ],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("call i32 @strcmp"),
            "expected strcmp for String payload; IR:\n{ir}"
        );
        assert!(
            ir.contains("icmp eq ptr") && ir.contains("br i1"),
            "expected null guard (icmp eq ptr + br i1); IR:\n{ir}"
        );
        assert!(
            ir.contains("phi i1"),
            "expected phi i1 in sdone merge block; IR:\n{ir}"
        );
        assert!(
            ir.contains("and i1"),
            "expected and i1 combining tag and payload; IR:\n{ir}"
        );
        assert!(
            ir.contains("extractvalue %struct.Option__String") && ir.contains(", 1"),
            "expected field-1 extractvalue for String payload; IR:\n{ir}"
        );
    }

    #[test]
    fn neq_option_string_compares_payload() {
        // Option<String> != Option<String> must also go through null-safe strcmp and
        // invert the result via xor i1 .., true, not fall back to tag-only !=.
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t0".into(),
                        type_name: "Option__String".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(0))],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t1".into(),
                        type_name: "Option__String".into(),
                        tag: 0,
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(1))],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::NeqInt,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["a".into(), "b".into()],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Option__String".into(),
                fields: vec![
                    ("tag".into(), tyra_types::Ty::Int),
                    ("0".into(), tyra_types::Ty::String),
                ],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        // strcmp path must still be emitted (not tag-only)
        assert!(
            ir.contains("call i32 @strcmp"),
            "expected strcmp for String payload in != path; IR:\n{ir}"
        );
        // Result is inverted via xor, not icmp ne
        assert!(
            ir.contains("xor i1"),
            "expected xor i1 to invert structural equality for !=; IR:\n{ir}"
        );
        // Null guard and phi must still be present
        assert!(
            ir.contains("phi i1"),
            "expected phi i1 in sdone block; IR:\n{ir}"
        );
    }

    #[test]
    fn eq_result_int_string_compares_string_payload() {
        // Result<Int, String>: field 0=tag i8, field 1=Int payload, field 2=String payload.
        // Mixed Scalar+StrPtr layout must NOT fall back to tag-only; field 2 must go
        // through null-safe strcmp.
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t0".into(),
                        type_name: "Result__Int__String".into(),
                        tag: 1,
                        fields: vec![
                            tyra_mir::Operand::Const(tyra_mir::Constant::Int(0)),
                            tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(0)),
                        ],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::AdtInit {
                        dest: "_t1".into(),
                        type_name: "Result__Int__String".into(),
                        tag: 1,
                        fields: vec![
                            tyra_mir::Operand::Const(tyra_mir::Constant::Int(0)),
                            tyra_mir::Operand::Const(tyra_mir::Constant::StringRef(1)),
                        ],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                        dest: "_t2".into(),
                        op: tyra_mir::MirBinOp::EqInt,
                        lhs: tyra_mir::Operand::Var("_t0".into()),
                        rhs: tyra_mir::Operand::Var("_t1".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["x".into(), "y".into()],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Result__Int__String".into(),
                fields: vec![
                    ("tag".into(), tyra_types::Ty::Int),
                    ("0".into(), tyra_types::Ty::Int),
                    ("1".into(), tyra_types::Ty::String),
                ],
                is_data: false,
                recursive_fields: vec![false, false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("call i32 @strcmp"),
            "expected strcmp for String field 2; IR:\n{ir}"
        );
        // Each assert checks that the struct name AND field index appear on the same
        // line, preventing false positives from unrelated IR constants like `i32 2`.
        assert!(
            ir.lines().any(
                |l| l.contains("extractvalue %struct.Result__Int__String") && l.contains(", 2")
            ),
            "expected extractvalue of field 2 (String payload) on same IR line; IR:\n{ir}"
        );
        assert!(
            ir.lines().any(
                |l| l.contains("extractvalue %struct.Result__Int__String") && l.contains(", 1")
            ),
            "expected extractvalue of field 1 (Int payload, not tag-only fallback) on same IR line; IR:\n{ir}"
        );
    }

    #[test]
    fn panic_emits_sentinel_exit101_unreachable() {
        // §12.1 + ADR 0012: panic(msg) → puts(msg) + sentinel to stderr + exit(101) + unreachable
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Const {
                        dest: "_t0".into(),
                        value: tyra_mir::Constant::StringRef(0),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Call {
                        dest: None,
                        func: "panic".into(),
                        args: vec![tyra_mir::Operand::Var("_t0".into())],
                    }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec!["oops".into()],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("@puts(ptr"),
            "expected puts call for panic message"
        );
        assert!(
            ir.contains("@.str.panic_sentinel"),
            "expected sentinel global written to stderr"
        );
        assert!(
            ir.contains("call void @exit(i32 101)"),
            "expected exit(101) after panic (ADR 0012)"
        );
        assert!(
            !ir.contains("call void @abort()"),
            "panic must NOT use abort() — must use exit(101)"
        );
        assert!(
            ir.contains("unreachable"),
            "expected unreachable after exit"
        );
    }

    #[test]
    fn data_type_struct_init_uses_gc_malloc() {
        // §8.6: data types are heap-allocated reference types
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "main".into(),
                params: vec![],
                return_type: tyra_types::Ty::Unit,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::StructInit {
                        dest: "user".into(),
                        type_name: "User".into(),
                        fields: vec![tyra_mir::Operand::Const(tyra_mir::Constant::Int(1))],
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                ],
                is_main: true,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "User".into(),
                fields: vec![("id".into(), tyra_types::Ty::Int)],
                is_data: true,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("call ptr @GC_malloc"),
            "data StructInit must use GC_malloc"
        );
        assert!(
            ir.contains("getelementptr %struct.User"),
            "must use GEP to init fields"
        );
        assert!(
            !ir.contains("insertvalue"),
            "data types must not use insertvalue"
        );
    }

    #[test]
    fn spawn_emits_thunk_and_runtime_call() {
        // M9: `spawn f(x)` must (1) register a per-site thunk struct type,
        // (2) call tyra_task_spawn with the thunk address, and (3) emit
        // the synthesized thunk function that unboxes args and boxes the
        // result via GC_malloc. The task handle is carried as i64 through
        // the MIR so it can flow through lists.
        let program = tyra_mir::Program {
            functions: vec![
                tyra_mir::Function {
                    name: "double".into(),
                    params: vec![("x".into(), tyra_types::Ty::Int)],
                    return_type: tyra_types::Ty::Int,
                    body: vec![
                        tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::BinOp {
                            dest: "_t0".into(),
                            op: tyra_mir::MirBinOp::MulInt,
                            lhs: tyra_mir::Operand::Var("x".into()),
                            rhs: tyra_mir::Operand::Const(tyra_mir::Constant::Int(2)),
                        }),
                        tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return {
                            value: Some(tyra_mir::Operand::Var("_t0".into())),
                        }),
                    ],
                    is_main: false,
                    local_metas: vec![],
                },
                tyra_mir::Function {
                    name: "main".into(),
                    params: vec![],
                    return_type: tyra_types::Ty::Unit,
                    body: vec![
                        tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Spawn {
                            dest: "_t1".into(),
                            func: "double".into(),
                            args: vec![tyra_mir::Operand::Const(tyra_mir::Constant::Int(21))],
                            arg_types: vec![tyra_types::Ty::Int],
                            result_type: tyra_types::Ty::Int,
                        }),
                        tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Await {
                            dest: "_t2".into(),
                            task: tyra_mir::Operand::Var("_t1".into()),
                            result_type: tyra_types::Ty::Int,
                        }),
                        tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return { value: None }),
                    ],
                    is_main: true,
                    local_metas: vec![],
                },
            ],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("%struct.__tyra_spawn_args_0 = type { i64 }"),
            "must declare per-site arg struct type"
        );
        assert!(
            ir.contains("call ptr @tyra_task_spawn(ptr @__tyra_spawn_thunk_0,"),
            "Spawn must call tyra_task_spawn with the thunk pointer"
        );
        assert!(
            ir.contains("ptrtoint ptr %_t1.h to i64"),
            "Spawn handle must be carried as i64 through the MIR"
        );
        assert!(
            ir.contains("define internal ptr @__tyra_spawn_thunk_0(ptr %args)"),
            "must emit the synthesized thunk definition"
        );
        assert!(
            ir.contains("call i64 @double(i64 %a0)"),
            "thunk must invoke the target function with loaded args"
        );
        assert!(
            ir.contains("inttoptr i64") && ir.contains("to ptr"),
            "Await must convert the i64 handle back to ptr"
        );
        assert!(
            ir.contains("call ptr @tyra_task_await(ptr "),
            "Await must call tyra_task_await"
        );
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
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::FieldGet {
                        dest: "id_val".into(),
                        obj: tyra_mir::Operand::Var("user".into()),
                        type_name: "User".into(),
                        field_index: 0,
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Return {
                        value: Some(tyra_mir::Operand::Var("id_val".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "User".into(),
                fields: vec![("id".into(), tyra_types::Ty::Int)],
                is_data: true,
                recursive_fields: vec![],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        assert!(
            ir.contains("getelementptr %struct.User"),
            "FieldGet on data must use GEP"
        );
        assert!(
            ir.contains("load i64"),
            "FieldGet on data must load the field"
        );
        assert!(
            !ir.contains("extractvalue"),
            "data FieldGet must not use extractvalue"
        );
        assert!(
            ir.contains("define i64 @get_id(ptr %user)"),
            "data type param must be ptr"
        );
    }

    #[test]
    fn alloca_hoisted_to_entry_block() {
        // Alloca inside a loop body (non-entry block) must be hoisted to entry
        // so it is allocated once per call rather than once per iteration.
        // See docs/notes/099-sum-column-diagnosis.md.
        let program = tyra_mir::Program {
            functions: vec![tyra_mir::Function {
                name: "loop_fn".into(),
                params: vec![("x".into(), tyra_types::Ty::Int)],
                return_type: tyra_types::Ty::Int,
                body: vec![
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Jump {
                        label: "loop_body".into(),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Label("loop_body".into())),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Alloca {
                        dest: "_t0".into(),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Store {
                        dest: "_t0".into(),
                        value: tyra_mir::Operand::Var("x".into()),
                    }),
                    tyra_mir::MirStmt::synthetic(tyra_mir::Instruction::Jump {
                        label: "loop_body".into(),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };

        let ir = emit_llvm_ir(&program);
        let entry_pos = ir.find("entry:").expect("entry block must be present");
        let alloca_pos = ir.find("  %_t0 = alloca").expect("alloca must be emitted");
        let loop_pos = ir
            .find("loop_body:")
            .expect("loop_body label must be present");
        assert!(
            alloca_pos > entry_pos,
            "alloca must come after entry: (was before)"
        );
        assert!(
            alloca_pos < loop_pos,
            "alloca must be hoisted before loop_body: (was after, causing per-iteration stack growth)"
        );
        // Verify no second alloca appears after loop_body: (catches double-emit bugs).
        let after_loop = &ir[loop_pos..];
        assert!(
            !after_loop.contains("alloca"),
            "no alloca must appear in or after loop_body: — got:\n{after_loop}"
        );
    }
}
