// LLVM IR text generation from MIR.
//
// Generates valid LLVM IR text that can be compiled with:
//   clang output.ll -o output
//
// For Milestone 1a, we use C library functions (puts, printf) for I/O.
// The Tyra runtime will replace these in later milestones.

use std::fmt::Write;

use tyra_mir::*;
use tyra_types::Ty;

use crate::helpers::{llvm_escape_string, llvm_type_str, target_triple};
use crate::instr_emit::emit_instruction;
use crate::type_scan::scan_function_types;

/// Struct type metadata for codegen.
pub(crate) struct StructInfo {
    /// LLVM type name: "%struct.Point"
    pub(crate) llvm_name: String,
    /// Field types in declaration order.
    /// For ADTs: field_types[0] is the tag type, stored as Ty::Int in MIR
    /// but emitted as i8 in LLVM.
    pub(crate) field_types: Vec<Ty>,
    /// Whether this is an ADT tagged struct (Option/Result).
    /// When true, field 0 is the i8 tag regardless of field_types[0].
    pub(crate) is_adt: bool,
    /// true = data type, heap-allocated and passed as ptr (§8.6 reference semantics).
    pub(crate) is_data: bool,
}

/// Generate LLVM IR text from a MIR program.
pub fn emit_llvm_ir(program: &Program) -> String {
    let mut out = String::new();

    // Build struct info map
    let struct_map: std::collections::HashMap<String, StructInfo> = program
        .struct_defs
        .iter()
        .map(|sd| {
            let is_adt = sd.fields.first().map(|(n, _)| n == "tag").unwrap_or(false);
            let info = StructInfo {
                llvm_name: format!("%struct.{}", sd.name),
                field_types: sd.fields.iter().map(|(_, ty)| ty.clone()).collect(),
                is_adt,
                is_data: sd.is_data,
            };
            (sd.name.clone(), info)
        })
        .collect();

    // Module header
    writeln!(out, "; Tyra compiler output").unwrap();
    writeln!(out, "target triple = \"{}\"", target_triple()).unwrap();
    writeln!(out).unwrap();

    // Globals for sys.args() (argc/argv captured at main entry)
    writeln!(out, "@.tyra.argc = internal global i32 0").unwrap();
    writeln!(out, "@.tyra.argv = internal global ptr null").unwrap();
    writeln!(out).unwrap();

    // Struct type declarations
    for sd in &program.struct_defs {
        let info = &struct_map[sd.name.as_str()];
        let field_tys: Vec<String> = sd
            .fields
            .iter()
            .enumerate()
            .map(|(i, (_, ty))| {
                // ADT tag field (field 0) is i8 in LLVM regardless of MIR type
                if info.is_adt && i == 0 {
                    "i8".into()
                } else {
                    let ty_str = llvm_type_str(ty, &struct_map);
                    // Unit (void) is not valid as a struct field; use i64 placeholder
                    if ty_str == "void" { "i64".into() } else { ty_str }
                }
            })
            .collect();
        writeln!(
            out,
            "%struct.{} = type {{ {} }}",
            sd.name,
            field_tys.join(", ")
        )
        .unwrap();
    }
    if !program.struct_defs.is_empty() {
        writeln!(out).unwrap();
    }

    // String constants
    for (idx, s) in program.string_constants.iter().enumerate() {
        let escaped = llvm_escape_string(s);
        // +1 for null terminator
        let len = s.len() + 1;
        writeln!(
            out,
            "@.str.{idx} = private unnamed_addr constant [{len} x i8] c\"{escaped}\\00\""
        )
        .unwrap();
    }
    if !program.string_constants.is_empty() {
        writeln!(out).unwrap();
    }

    // Format strings for print
    writeln!(
        out,
        "@.fmt.str = private unnamed_addr constant [3 x i8] c\"%s\\00\""
    )
    .unwrap();
    writeln!(
        out,
        "@.fmt.int = private unnamed_addr constant [4 x i8] c\"%ld\\00\""
    )
    .unwrap();
    writeln!(
        out,
        "@.fmt.int_ln = private unnamed_addr constant [5 x i8] c\"%ld\\0A\\00\""
    )
    .unwrap();
    writeln!(
        out,
        "@.fmt.float = private unnamed_addr constant [3 x i8] c\"%g\\00\""
    )
    .unwrap();
    writeln!(
        out,
        "@.fmt.float_ln = private unnamed_addr constant [4 x i8] c\"%g\\0A\\00\""
    )
    .unwrap();
    writeln!(out).unwrap();

    // External declarations
    writeln!(out, "; External declarations").unwrap();
    writeln!(out, "declare i32 @puts(ptr)").unwrap();
    writeln!(out, "declare i32 @printf(ptr, ...)").unwrap();
    writeln!(out, "declare i32 @snprintf(ptr, i64, ptr, ...)").unwrap();
    // Boehm GC (libgc): tracing conservative collector. See ADR-0007.
    // All heap allocations go through GC_malloc; GC_init is called at @main entry.
    writeln!(out, "declare ptr @GC_malloc(i64)").unwrap();
    writeln!(out, "declare void @GC_init()").unwrap();
    // Tyra async runtime (libtyra_runtime.a). See runtime/ crate.
    // tyra_rt_init is invoked at @main entry after GC_init.
    writeln!(out, "declare void @tyra_rt_init()").unwrap();
    writeln!(out, "declare void @abort()").unwrap();
    writeln!(out, "declare void @exit(i32)").unwrap();
    writeln!(out, "declare i32 @strcmp(ptr, ptr)").unwrap();
    // strtol returns long (i64 on LP64 platforms). TODO: use strtoll for Windows LLP64.
    writeln!(out, "declare i64 @strtol(ptr, ptr, i32)").unwrap();
    writeln!(out).unwrap();

    // Build function signature map for cross-function type resolution
    let fn_sigs: std::collections::HashMap<String, FnSig> = program
        .functions
        .iter()
        .map(|f| {
            let sig = FnSig {
                param_types: f.params.iter().map(|(_, ty)| ty.clone()).collect(),
                return_type: f.return_type.clone(),
            };
            (f.name.clone(), sig)
        })
        .collect();

    // Functions
    for func in &program.functions {
        emit_function(
            &mut out,
            func,
            &program.string_constants,
            &struct_map,
            &fn_sigs,
        );
        writeln!(out).unwrap();
    }

    out
}

/// Function signature for cross-function type resolution.
pub(crate) struct FnSig {
    pub(crate) param_types: Vec<Ty>,
    pub(crate) return_type: Ty,
}

fn emit_function(
    out: &mut String,
    func: &Function,
    strings: &[String],
    struct_map: &std::collections::HashMap<String, StructInfo>,
    fn_sigs: &std::collections::HashMap<String, FnSig>,
) {
    // Pre-scan: collect type metadata for all SSA temps and alloca slots.
    let scan = scan_function_types(func, struct_map, fn_sigs);

    let ret_ty = llvm_type_str(&func.return_type, struct_map);

    // Function signature
    let params: Vec<String> = func
        .params
        .iter()
        .map(|(name, ty)| format!("{} %{name}", llvm_type_str(ty, struct_map)))
        .collect();

    if func.is_main {
        writeln!(out, "define i32 @main(i32 %argc, ptr %argv) {{").unwrap();
    } else {
        writeln!(
            out,
            "define {ret_ty} @{}({}) {{",
            func.name,
            params.join(", ")
        )
        .unwrap();
    }

    writeln!(out, "entry:").unwrap();

    // Save argc/argv to globals for sys.args()
    if func.is_main {
        // Initialize Boehm GC before any heap allocation (ADR-0007).
        writeln!(out, "  call void @GC_init()").unwrap();
        // Initialize Tyra async runtime (scheduler + thread pool). M9.
        writeln!(out, "  call void @tyra_rt_init()").unwrap();
        writeln!(out, "  store i32 %argc, ptr @.tyra.argc").unwrap();
        writeln!(out, "  store ptr %argv, ptr @.tyra.argv").unwrap();
    }

    // Allocate parameter copies for mutation support
    for (name, ty) in &func.params {
        let lt = llvm_type_str(ty, struct_map);
        writeln!(out, "  %{name}.addr = alloca {lt}").unwrap();
        writeln!(out, "  store {lt} %{name}, ptr %{name}.addr").unwrap();
    }

    let ctx = EmitCtx {
        struct_map,
        fn_sigs,
        string_temps: &scan.string_temps,
        float_temps: &scan.float_temps,
        bool_temps: &scan.bool_temps,
        struct_temps: &scan.struct_temps,
        alloca_llvm_types: &scan.alloca_llvm_types,
    };

    // Emit instructions, skipping dead code after block terminators.
    let mut block_terminated = false;
    for inst in &func.body {
        match inst {
            Instruction::Label(_) => {
                block_terminated = false;
            }
            _ if block_terminated => continue,
            _ => {}
        }
        emit_instruction(out, inst, func, strings, &ctx);
        match inst {
            Instruction::Return { .. }
            | Instruction::Jump { .. }
            | Instruction::BranchIf { .. } => {
                block_terminated = true;
            }
            Instruction::Call { func: fname, .. }
                if matches!(fname.as_str(), "panic" | "sys__exit") =>
            {
                block_terminated = true;
            }
            _ => {}
        }
    }

    writeln!(out, "}}").unwrap();
}

/// Context passed to instruction emitters.
pub(crate) struct EmitCtx<'a> {
    pub(crate) struct_map: &'a std::collections::HashMap<String, StructInfo>,
    pub(crate) fn_sigs: &'a std::collections::HashMap<String, FnSig>,
    pub(crate) string_temps: &'a std::collections::HashSet<String>,
    pub(crate) float_temps: &'a std::collections::HashSet<String>,
    pub(crate) bool_temps: &'a std::collections::HashSet<String>,
    pub(crate) struct_temps: &'a std::collections::HashMap<String, String>,
    /// Resolved LLVM type for alloca slots (from Store analysis).
    pub(crate) alloca_llvm_types: &'a std::collections::HashMap<String, String>,
}

// Extracted modules:
// - helpers.rs: operand_ref, llvm_type_str, llvm_escape_string, etc.
// - type_scan.rs: pre_scan_struct_types, pre_scan_alloca_llvm_types
// - instr_emit.rs: emit_instruction, emit_call_args_typed
