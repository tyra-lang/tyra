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

/// Struct type metadata for codegen.
struct StructInfo {
    /// LLVM type name: "%struct.Point"
    llvm_name: String,
    /// Field types in declaration order.
    /// For ADTs: field_types[0] is the tag type, stored as Ty::Int in MIR
    /// but emitted as i8 in LLVM.
    field_types: Vec<Ty>,
    /// Whether this is an ADT tagged struct (Option/Result).
    /// When true, field 0 is the i8 tag regardless of field_types[0].
    is_adt: bool,
}

/// Generate LLVM IR text from a MIR program.
pub fn emit_llvm_ir(program: &Program) -> String {
    let mut out = String::new();

    // Build struct info map
    let struct_map: std::collections::HashMap<String, StructInfo> = program
        .struct_defs
        .iter()
        .map(|sd| {
            let is_adt =
                sd.name.starts_with("Option__") || sd.name.starts_with("Result__");
            let info = StructInfo {
                llvm_name: format!("%struct.{}", sd.name),
                field_types: sd.fields.iter().map(|(_, ty)| ty.clone()).collect(),
                is_adt,
            };
            (sd.name.clone(), info)
        })
        .collect();

    // Module header
    writeln!(out, "; Tyra compiler output").unwrap();
    writeln!(out, "target triple = \"{}\"", target_triple()).unwrap();
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
                    llvm_type_str(ty, &struct_map)
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
    writeln!(out, "declare ptr @malloc(i64)").unwrap();
    writeln!(out, "declare void @abort()").unwrap();
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
struct FnSig {
    param_types: Vec<Ty>,
    return_type: Ty,
}

fn emit_function(
    out: &mut String,
    func: &Function,
    strings: &[String],
    struct_map: &std::collections::HashMap<String, StructInfo>,
    fn_sigs: &std::collections::HashMap<String, FnSig>,
) {
    // Pre-scan: collect temps that hold string values
    let mut string_temps: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Pre-scan: collect temps that hold float values
    let mut float_temps: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Pre-scan: collect temps that hold bool values
    let mut bool_temps: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Register function params by their declared type
    for (name, ty) in &func.params {
        match ty {
            Ty::String => {
                string_temps.insert(name.clone());
            }
            Ty::Float => {
                float_temps.insert(name.clone());
            }
            Ty::Bool => {
                bool_temps.insert(name.clone());
            }
            _ => {}
        }
    }

    for inst in &func.body {
        match inst {
            Instruction::Const { dest, value } => match value {
                Constant::StringRef(_) => {
                    string_temps.insert(dest.clone());
                }
                Constant::Float(_) => {
                    float_temps.insert(dest.clone());
                }
                Constant::Bool(_) => {
                    bool_temps.insert(dest.clone());
                }
                _ => {}
            },
            Instruction::BinOp { dest, op, .. } => {
                if matches!(
                    op,
                    MirBinOp::AddFloat
                        | MirBinOp::SubFloat
                        | MirBinOp::MulFloat
                        | MirBinOp::DivFloat
                ) {
                    float_temps.insert(dest.clone());
                }
                // Comparison ops produce i1 (bool) results
                if matches!(
                    op,
                    MirBinOp::EqInt
                        | MirBinOp::NeqInt
                        | MirBinOp::LtInt
                        | MirBinOp::LeInt
                        | MirBinOp::GtInt
                        | MirBinOp::GeInt
                        | MirBinOp::LtFloat
                        | MirBinOp::LeFloat
                        | MirBinOp::GtFloat
                        | MirBinOp::GeFloat
                        | MirBinOp::And
                        | MirBinOp::Or
                ) {
                    bool_temps.insert(dest.clone());
                }
            }
            Instruction::Not { dest, .. } => {
                bool_temps.insert(dest.clone());
            }
            Instruction::FieldGet {
                dest,
                type_name,
                field_index,
                ..
            } => {
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        if *field_ty == Ty::Float {
                            float_temps.insert(dest.clone());
                        } else if *field_ty == Ty::String {
                            string_temps.insert(dest.clone());
                        } else if *field_ty == Ty::Bool {
                            bool_temps.insert(dest.clone());
                        }
                    }
                }
            }
            Instruction::Call {
                dest: Some(dest),
                func: fname,
                ..
            } => {
                // Track return type from function signatures
                if let Some(sig) = fn_sigs.get(fname.as_str()) {
                    match &sig.return_type {
                        Ty::String => {
                            string_temps.insert(dest.clone());
                        }
                        Ty::Float => {
                            float_temps.insert(dest.clone());
                        }
                        Ty::Bool => {
                            bool_temps.insert(dest.clone());
                        }
                        _ => {}
                    }
                }
            }
            Instruction::Neg { dest, operand } => {
                // Float negation produces a float result
                let is_float = match operand {
                    Operand::Const(Constant::Float(_)) => true,
                    Operand::Var(name) => float_temps.contains(name),
                    _ => false,
                };
                if is_float {
                    float_temps.insert(dest.clone());
                }
            }
            Instruction::Copy { dest, source } => {
                if float_temps.contains(source.as_str()) {
                    float_temps.insert(dest.clone());
                }
                if string_temps.contains(source.as_str()) {
                    string_temps.insert(dest.clone());
                }
                if bool_temps.contains(source.as_str()) {
                    bool_temps.insert(dest.clone());
                }
            }
            Instruction::StringFormat { dest, .. } => {
                string_temps.insert(dest.clone());
            }
            Instruction::AdtPayload {
                dest,
                type_name,
                field_index,
                ..
            } => {
                // Track payload type from the specified field of the ADT struct
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        if *field_ty == Ty::String {
                            string_temps.insert(dest.clone());
                        } else if *field_ty == Ty::Float {
                            float_temps.insert(dest.clone());
                        } else if *field_ty == Ty::Bool {
                            bool_temps.insert(dest.clone());
                        }
                    }
                }
            }
            Instruction::Load { dest, source } => {
                // Propagate string/float type from alloca
                // (will be resolved after alloca_llvm_types is computed)
                let _ = (dest, source);
            }
            _ => {}
        }
    }

    // Pre-scan: track struct-typed temps and alloca types
    let (struct_temps, alloca_types) = pre_scan_struct_types(func, struct_map, fn_sigs);

    // Pre-scan: determine LLVM types for alloca slots (match results, etc.)
    let alloca_llvm_types = pre_scan_alloca_llvm_types(
        func,
        &string_temps,
        &float_temps,
        &bool_temps,
        &struct_temps,
        struct_map,
    );

    // Propagate alloca types to Load destinations
    for inst in &func.body {
        if let Instruction::Load { dest, source } = inst {
            if let Some(ty) = alloca_llvm_types.get(source.as_str()) {
                match ty.as_str() {
                    "ptr" => {
                        string_temps.insert(dest.clone());
                    }
                    "double" => {
                        float_temps.insert(dest.clone());
                    }
                    "i1" => {
                        bool_temps.insert(dest.clone());
                    }
                    _ => {}
                }
            }
        }
    }

    let ret_ty = llvm_type_str(&func.return_type, struct_map);

    // Function signature
    let params: Vec<String> = func
        .params
        .iter()
        .map(|(name, ty)| format!("{} %{name}", llvm_type_str(ty, struct_map)))
        .collect();

    if func.is_main {
        writeln!(out, "define i32 @main() {{").unwrap();
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

    // Allocate parameter copies for mutation support
    for (name, ty) in &func.params {
        let lt = llvm_type_str(ty, struct_map);
        writeln!(out, "  %{name}.addr = alloca {lt}").unwrap();
        writeln!(out, "  store {lt} %{name}, ptr %{name}.addr").unwrap();
    }

    let ctx = EmitCtx {
        struct_map,
        fn_sigs,
        string_temps: &string_temps,
        float_temps: &float_temps,
        bool_temps: &bool_temps,
        struct_temps: &struct_temps,
        alloca_llvm_types: &alloca_llvm_types,
    };
    let _ = &alloca_types; // Used by alloca_llvm_types computation

    // Emit instructions
    for inst in &func.body {
        emit_instruction(out, inst, func, strings, &ctx);
    }

    writeln!(out, "}}").unwrap();
}

/// Pre-scan function body to track which temps hold struct-typed values.
fn pre_scan_struct_types(
    func: &Function,
    struct_map: &std::collections::HashMap<String, StructInfo>,
    fn_sigs: &std::collections::HashMap<String, FnSig>,
) -> (
    std::collections::HashMap<String, String>,
    std::collections::HashMap<String, String>,
) {
    let mut struct_temps: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut alloca_types: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    // Function params that are struct-typed
    for (name, ty) in &func.params {
        if let Ty::Named(type_name) = ty {
            if struct_map.contains_key(type_name.as_str()) {
                struct_temps.insert(name.clone(), type_name.clone());
            }
        }
    }

    // Scan instructions
    for inst in &func.body {
        match inst {
            Instruction::StructInit {
                dest, type_name, ..
            } => {
                struct_temps.insert(dest.clone(), type_name.clone());
            }
            Instruction::AdtInit {
                dest, type_name, ..
            } => {
                struct_temps.insert(dest.clone(), type_name.clone());
            }
            Instruction::FieldGet {
                dest,
                type_name,
                field_index,
                ..
            } => {
                // Check if the extracted field is itself a struct type
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        if let Ty::Named(ft_name) = field_ty {
                            if struct_map.contains_key(ft_name.as_str()) {
                                struct_temps.insert(dest.clone(), ft_name.clone());
                            }
                        }
                    }
                }
            }
            Instruction::Call {
                dest: Some(dest),
                func: fname,
                ..
            } => {
                // Check if the called function returns a struct type
                if let Some(sig) = fn_sigs.get(fname.as_str()) {
                    if let Ty::Named(type_name) = &sig.return_type {
                        if struct_map.contains_key(type_name.as_str()) {
                            struct_temps.insert(dest.clone(), type_name.clone());
                        }
                    }
                    // Also check for generic return types (Option/Result)
                    if sig.return_type.is_option() || sig.return_type.is_result() {
                        let mono_name = sig.return_type.monomorphized_name();
                        if struct_map.contains_key(mono_name.as_str()) {
                            struct_temps.insert(dest.clone(), mono_name);
                        }
                    }
                }
            }
            Instruction::Copy { dest, source } => {
                if let Some(stype) = struct_temps.get(source).cloned() {
                    struct_temps.insert(dest.clone(), stype);
                }
            }
            Instruction::Store { dest, value } => {
                if let Operand::Var(name) = value {
                    if let Some(stype) = struct_temps.get(name).cloned() {
                        alloca_types.insert(dest.clone(), stype);
                    }
                }
            }
            Instruction::Load { dest, source } => {
                if let Some(stype) = alloca_types.get(source).cloned() {
                    struct_temps.insert(dest.clone(), stype);
                }
            }
            _ => {}
        }
    }

    (struct_temps, alloca_types)
}

/// Pre-scan function body to determine the LLVM type for each alloca slot.
/// This handles match result allocas that may store strings, floats, or structs.
fn pre_scan_alloca_llvm_types(
    func: &Function,
    string_temps: &std::collections::HashSet<String>,
    float_temps: &std::collections::HashSet<String>,
    bool_temps: &std::collections::HashSet<String>,
    struct_temps: &std::collections::HashMap<String, String>,
    struct_map: &std::collections::HashMap<String, StructInfo>,
) -> std::collections::HashMap<String, String> {
    let mut alloca_llvm_types: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for inst in &func.body {
        if let Instruction::Store { dest, value } = inst {
            if alloca_llvm_types.contains_key(dest) {
                continue; // First store determines type
            }
            if let Operand::Var(name) = value {
                if string_temps.contains(name) {
                    alloca_llvm_types.insert(dest.clone(), "ptr".into());
                } else if float_temps.contains(name) {
                    alloca_llvm_types.insert(dest.clone(), "double".into());
                } else if bool_temps.contains(name) {
                    alloca_llvm_types.insert(dest.clone(), "i1".into());
                } else if let Some(stype) = struct_temps.get(name.as_str()) {
                    alloca_llvm_types.insert(
                        dest.clone(),
                        struct_map[stype.as_str()].llvm_name.clone(),
                    );
                }
                // Otherwise default to i64 (handled in emit)
            }
        }
    }

    alloca_llvm_types
}

/// Context passed to instruction emitters.
struct EmitCtx<'a> {
    struct_map: &'a std::collections::HashMap<String, StructInfo>,
    fn_sigs: &'a std::collections::HashMap<String, FnSig>,
    string_temps: &'a std::collections::HashSet<String>,
    float_temps: &'a std::collections::HashSet<String>,
    bool_temps: &'a std::collections::HashSet<String>,
    struct_temps: &'a std::collections::HashMap<String, String>,
    /// Resolved LLVM type for alloca slots (from Store analysis).
    alloca_llvm_types: &'a std::collections::HashMap<String, String>,
}

fn emit_instruction(
    out: &mut String,
    inst: &Instruction,
    func: &Function,
    strings: &[String],
    ctx: &EmitCtx,
) {
    match inst {
        Instruction::Const { dest, value } => match value {
            Constant::Int(n) => {
                writeln!(out, "  %{dest} = add i64 {n}, 0").unwrap();
            }
            Constant::Float(f) => {
                let lit = llvm_float_literal(*f);
                writeln!(out, "  %{dest} = fadd double {lit}, 0.0").unwrap();
            }
            Constant::Bool(b) => {
                let val = if *b { 1 } else { 0 };
                writeln!(out, "  %{dest} = add i1 {val}, 0").unwrap();
            }
            Constant::StringRef(idx) => {
                let len = strings[*idx].len() + 1;
                writeln!(
                    out,
                    "  %{dest} = getelementptr [{len} x i8], ptr @.str.{idx}, i64 0, i64 0"
                )
                .unwrap();
            }
            Constant::Unit => {
                // Unit has no runtime representation; emit a dummy
                writeln!(out, "  ; {dest} = unit (no-op)").unwrap();
            }
        },

        Instruction::Call {
            dest,
            func: fname,
            args,
        } => {
            // Map Tyra builtins to C functions
            match fname.as_str() {
                "print" | "eprint" | "println" | "eprintln" => {
                    let is_println = fname == "println" || fname == "eprintln";
                    emit_print_call(out, dest.as_deref(), args, func, is_println, ctx);
                }
                _ => {
                    // User-defined function call — look up signature for types
                    let ret_ty = if let Some(sig) = ctx.fn_sigs.get(fname.as_str()) {
                        llvm_type_str(&sig.return_type, ctx.struct_map)
                    } else {
                        "i64".into()
                    };
                    let user_args = emit_call_args_typed(args, fname, func, ctx);
                    if ret_ty == "void" {
                        writeln!(out, "  call void @{fname}({user_args})").unwrap();
                    } else if let Some(d) = dest {
                        writeln!(out, "  %{d} = call {ret_ty} @{fname}({user_args})")
                            .unwrap();
                    } else {
                        writeln!(out, "  call {ret_ty} @{fname}({user_args})").unwrap();
                    }
                }
            }
        }

        Instruction::BinOp { dest, op, lhs, rhs } => {
            let l = operand_ref(lhs, func);
            let r = operand_ref(rhs, func);
            let instr = match op {
                MirBinOp::AddInt => format!("add i64 {l}, {r}"),
                MirBinOp::SubInt => format!("sub i64 {l}, {r}"),
                MirBinOp::MulInt => format!("mul i64 {l}, {r}"),
                MirBinOp::DivInt => format!("sdiv i64 {l}, {r}"),
                MirBinOp::AddFloat => format!("fadd double {l}, {r}"),
                MirBinOp::SubFloat => format!("fsub double {l}, {r}"),
                MirBinOp::MulFloat => format!("fmul double {l}, {r}"),
                MirBinOp::DivFloat => format!("fdiv double {l}, {r}"),
                MirBinOp::EqInt => {
                    let is_bool = is_bool_operand(lhs, ctx) || is_bool_operand(rhs, ctx);
                    if is_bool {
                        format!("icmp eq i1 {l}, {r}")
                    } else {
                        format!("icmp eq i64 {l}, {r}")
                    }
                }
                MirBinOp::NeqInt => {
                    let is_bool = is_bool_operand(lhs, ctx) || is_bool_operand(rhs, ctx);
                    if is_bool {
                        format!("icmp ne i1 {l}, {r}")
                    } else {
                        format!("icmp ne i64 {l}, {r}")
                    }
                }
                MirBinOp::LtInt => format!("icmp slt i64 {l}, {r}"),
                MirBinOp::LeInt => format!("icmp sle i64 {l}, {r}"),
                MirBinOp::GtInt => format!("icmp sgt i64 {l}, {r}"),
                MirBinOp::GeInt => format!("icmp sge i64 {l}, {r}"),
                MirBinOp::LtFloat => format!("fcmp olt double {l}, {r}"),
                MirBinOp::LeFloat => format!("fcmp ole double {l}, {r}"),
                MirBinOp::GtFloat => format!("fcmp ogt double {l}, {r}"),
                MirBinOp::GeFloat => format!("fcmp oge double {l}, {r}"),
                MirBinOp::And => format!("and i1 {l}, {r}"),
                MirBinOp::Or => format!("or i1 {l}, {r}"),
            };
            writeln!(out, "  %{dest} = {instr}").unwrap();
        }

        Instruction::Neg { dest, operand } => {
            let v = operand_ref(operand, func);
            let is_float = match operand {
                Operand::Const(Constant::Float(_)) => true,
                Operand::Var(name) => ctx.float_temps.contains(name),
                _ => false,
            };
            if is_float {
                writeln!(out, "  %{dest} = fneg double {v}").unwrap();
            } else {
                writeln!(out, "  %{dest} = sub i64 0, {v}").unwrap();
            }
        }

        Instruction::Not { dest, operand } => {
            let v = operand_ref(operand, func);
            writeln!(out, "  %{dest} = xor i1 {v}, 1").unwrap();
        }

        Instruction::Copy { dest, source } => {
            if is_param(source, func) {
                let lt = param_llvm_type(source, func, ctx.struct_map);
                writeln!(out, "  %{dest} = load {lt}, ptr %{source}.addr").unwrap();
            } else if ctx.struct_temps.contains_key(source.as_str()) {
                // Struct SSA copy: use alloca+store+load to create a new SSA name
                let stype = &ctx.struct_temps[source.as_str()];
                let llvm_ty = &ctx.struct_map[stype.as_str()].llvm_name;
                writeln!(out, "  %{dest}.copy.addr = alloca {llvm_ty}").unwrap();
                writeln!(out, "  store {llvm_ty} %{source}, ptr %{dest}.copy.addr")
                    .unwrap();
                writeln!(out, "  %{dest} = load {llvm_ty}, ptr %{dest}.copy.addr")
                    .unwrap();
            } else if ctx.string_temps.contains(source.as_str()) {
                // String (ptr) SSA copy via inttoptr/ptrtoint round-trip
                writeln!(out, "  %{dest}.ptr.int = ptrtoint ptr %{source} to i64")
                    .unwrap();
                writeln!(out, "  %{dest} = inttoptr i64 %{dest}.ptr.int to ptr")
                    .unwrap();
            } else if ctx.float_temps.contains(source.as_str()) {
                writeln!(out, "  %{dest} = fadd double %{source}, 0.0").unwrap();
            } else if ctx.bool_temps.contains(source.as_str()) {
                writeln!(out, "  %{dest} = xor i1 %{source}, 0").unwrap();
            } else {
                // SSA alias: create a new SSA value identical to the source
                writeln!(out, "  %{dest} = add i64 %{source}, 0").unwrap();
            }
        }

        Instruction::Return { value } => {
            if func.is_main {
                writeln!(out, "  ret i32 0").unwrap();
            } else {
                match value {
                    Some(v) => {
                        let ret_ty = llvm_type_str(&func.return_type, ctx.struct_map);
                        let val = operand_ref(v, func);
                        writeln!(out, "  ret {ret_ty} {val}").unwrap();
                    }
                    None => {
                        writeln!(out, "  ret void").unwrap();
                    }
                }
            }
        }

        Instruction::Label(name) => {
            writeln!(out, "{name}:").unwrap();
        }

        Instruction::BranchIf {
            cond,
            true_label,
            false_label,
        } => {
            let c = operand_ref(cond, func);
            writeln!(
                out,
                "  br i1 {c}, label %{true_label}, label %{false_label}"
            )
            .unwrap();
        }

        Instruction::Jump { label } => {
            writeln!(out, "  br label %{label}").unwrap();
        }

        Instruction::Phi { dest, branches } => {
            let phi_ty = if let Some((first_val, _)) = branches.first() {
                infer_operand_type(first_val, ctx)
            } else {
                "i64".into()
            };
            let entries: Vec<String> = branches
                .iter()
                .map(|(val, label)| format!("[{}, %{label}]", operand_ref(val, func)))
                .collect();
            writeln!(out, "  %{dest} = phi {phi_ty} {}", entries.join(", ")).unwrap();
        }

        Instruction::Alloca { dest } => {
            let llvm_ty = ctx
                .alloca_llvm_types
                .get(dest.as_str())
                .map(String::as_str)
                .unwrap_or("i64");
            writeln!(out, "  %{dest} = alloca {llvm_ty}").unwrap();
        }

        Instruction::Store { dest, value } => {
            let val = operand_ref(value, func);
            let llvm_ty = ctx
                .alloca_llvm_types
                .get(dest.as_str())
                .map(String::as_str)
                .unwrap_or("i64");
            writeln!(out, "  store {llvm_ty} {val}, ptr %{dest}").unwrap();
        }

        Instruction::Load { dest, source } => {
            let llvm_ty = ctx
                .alloca_llvm_types
                .get(source.as_str())
                .map(String::as_str)
                .unwrap_or("i64");
            writeln!(out, "  %{dest} = load {llvm_ty}, ptr %{source}").unwrap();
        }

        Instruction::StructInit {
            dest,
            type_name,
            fields,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            if fields.is_empty() {
                // Zero-field struct: just produce undef
                writeln!(out, "  ; %{dest} = {llvm_ty} undef (zero-field struct)")
                    .unwrap();
            } else {
                // Build struct value via insertvalue chain starting from undef
                let mut current = "undef".to_string();
                for (i, field_op) in fields.iter().enumerate() {
                    let val = operand_ref(field_op, func);
                    let field_ty = llvm_type_str(&info.field_types[i], ctx.struct_map);
                    let step_dest = if i + 1 == fields.len() {
                        dest.clone()
                    } else {
                        format!("{dest}.s{i}")
                    };
                    writeln!(
                        out,
                        "  %{step_dest} = insertvalue {llvm_ty} {current}, {field_ty} {val}, {i}"
                    )
                    .unwrap();
                    current = format!("%{step_dest}");
                }
            }
        }

        Instruction::FieldGet {
            dest,
            obj,
            type_name,
            field_index,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let val = operand_ref(obj, func);
            writeln!(
                out,
                "  %{dest} = extractvalue {llvm_ty} {val}, {field_index}"
            )
            .unwrap();
        }

        Instruction::StringFormat {
            dest,
            format_ref,
            args,
        } => {
            // Heap-allocate a 1024-byte buffer for the formatted string.
            // Uses malloc so the string survives function return (needed for to_string()).
            // Strings longer than 1024 bytes are truncated by snprintf.
            // TODO: GC integration to free these buffers.
            writeln!(
                out,
                "  %{dest} = call ptr @malloc(i64 1024)"
            )
            .unwrap();
            // Abort if malloc returns null (out of memory)
            writeln!(
                out,
                "  %{dest}.null = icmp eq ptr %{dest}, null"
            )
            .unwrap();
            writeln!(
                out,
                "  br i1 %{dest}.null, label %{dest}.oom, label %{dest}.ok"
            )
            .unwrap();
            writeln!(out, "{dest}.oom:").unwrap();
            writeln!(out, "  call void @abort()").unwrap();
            writeln!(out, "  unreachable").unwrap();
            writeln!(out, "{dest}.ok:").unwrap();

            // Build format string reference
            let fmt_len = strings[*format_ref].len() + 1;
            let fmt_ref = format!(
                "getelementptr ([{fmt_len} x i8], ptr @.str.{format_ref}, i64 0, i64 0)"
            );

            // Build snprintf argument list
            let mut snprintf_args = vec![
                format!("ptr %{dest}"),
                "i64 1024".to_string(),
                format!("ptr {fmt_ref}"),
            ];

            for (j, arg) in args.iter().enumerate() {
                let val = operand_ref(arg, func);
                let ty = infer_operand_type(arg, ctx);
                if ty == "i1" {
                    // Bool needs widening for printf varargs
                    writeln!(out, "  %{dest}.zext.{j} = zext i1 {val} to i64").unwrap();
                    snprintf_args.push(format!("i64 %{dest}.zext.{j}"));
                } else {
                    snprintf_args.push(format!("{ty} {val}"));
                }
            }

            writeln!(
                out,
                "  %{dest}.len = call i32 (ptr, i64, ptr, ...) @snprintf({})",
                snprintf_args.join(", ")
            )
            .unwrap();
        }

        Instruction::AdtInit {
            dest,
            type_name,
            tag,
            payload,
            payload_field_index,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let num_fields = info.field_types.len();

            // Field 0: tag (i8)
            let mut current = format!("%{dest}.s0");
            writeln!(
                out,
                "  {current} = insertvalue {llvm_ty} undef, i8 {tag}, 0"
            )
            .unwrap();

            // Fill remaining fields: payload goes to payload_field_index, others get zero
            for fi in 1..num_fields {
                let field_ty_str = llvm_type_str(&info.field_types[fi], ctx.struct_map);
                let is_last = fi + 1 == num_fields;
                let step_dest = if is_last {
                    format!("%{dest}")
                } else {
                    format!("%{dest}.s{fi}")
                };

                if fi as u32 == *payload_field_index {
                    if let Some(payload_op) = payload {
                        let val = operand_ref(payload_op, func);
                        writeln!(
                            out,
                            "  {step_dest} = insertvalue {llvm_ty} {current}, {field_ty_str} {val}, {fi}"
                        )
                        .unwrap();
                    } else {
                        let zero = match field_ty_str.as_str() {
                            "ptr" => "null",
                            "double" => "0.0",
                            _ => "0",
                        };
                        writeln!(
                            out,
                            "  {step_dest} = insertvalue {llvm_ty} {current}, {field_ty_str} {zero}, {fi}"
                        )
                        .unwrap();
                    }
                } else {
                    // Non-payload field: insert zero
                    let zero = match field_ty_str.as_str() {
                        "ptr" => "null",
                        "double" => "0.0",
                        _ => "0",
                    };
                    writeln!(
                        out,
                        "  {step_dest} = insertvalue {llvm_ty} {current}, {field_ty_str} {zero}, {fi}"
                    )
                    .unwrap();
                }
                current = step_dest;
            }
        }

        Instruction::AdtTag {
            dest,
            obj,
            type_name,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let val = operand_ref(obj, func);
            // Extract tag (field 0, i8) and extend to i64 for comparison
            writeln!(
                out,
                "  %{dest}.i8 = extractvalue {llvm_ty} {val}, 0"
            )
            .unwrap();
            writeln!(
                out,
                "  %{dest} = zext i8 %{dest}.i8 to i64"
            )
            .unwrap();
        }

        Instruction::AdtPayload {
            dest,
            obj,
            type_name,
            field_index,
        } => {
            let info = &ctx.struct_map[type_name.as_str()];
            let llvm_ty = &info.llvm_name;
            let val = operand_ref(obj, func);
            writeln!(
                out,
                "  %{dest} = extractvalue {llvm_ty} {val}, {field_index}"
            )
            .unwrap();
        }
    }
}

/// Emit a print/println call, auto-detecting argument type.
/// String args use %s format, Int args use %ld format, Float args use %g format.
fn emit_print_call(
    out: &mut String,
    dest: Option<&str>,
    args: &[Operand],
    func: &Function,
    is_println: bool,
    ctx: &EmitCtx,
) {
    if args.is_empty() {
        if is_println {
            let call = "call i32 @puts(ptr @.fmt.str)";
            if let Some(d) = dest {
                writeln!(out, "  %{d} = {call}").unwrap();
            } else {
                writeln!(out, "  {call}").unwrap();
            }
        }
        return;
    }

    let arg = &args[0];
    let val = operand_ref(arg, func);

    // Detect type using pre-scanned temp sets
    let is_string = match arg {
        Operand::Const(Constant::StringRef(_)) => true,
        Operand::Var(name) => ctx.string_temps.contains(name),
        _ => false,
    };
    let is_float = match arg {
        Operand::Const(Constant::Float(_)) => true,
        Operand::Var(name) => ctx.float_temps.contains(name),
        _ => false,
    };
    let is_bool = is_bool_operand(arg, ctx);

    let call_str = if is_string && is_println {
        format!("call i32 @puts(ptr {val})")
    } else if is_string {
        format!("call i32 (ptr, ...) @printf(ptr @.fmt.str, ptr {val})")
    } else if is_float {
        let fmt = if is_println {
            "@.fmt.float_ln"
        } else {
            "@.fmt.float"
        };
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, double {val})")
    } else if is_bool {
        // Bool (i1) must be widened to i64 for printf varargs
        let fmt = if is_println {
            "@.fmt.int_ln"
        } else {
            "@.fmt.int"
        };
        // Emit zext inline before the call
        let wide = format!("{val}.wide");
        writeln!(out, "  {wide} = zext i1 {val} to i64").unwrap();
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, i64 {wide})")
    } else {
        let fmt = if is_println {
            "@.fmt.int_ln"
        } else {
            "@.fmt.int"
        };
        format!("call i32 (ptr, ...) @printf(ptr {fmt}, i64 {val})")
    };

    if let Some(d) = dest {
        // printf/puts return i32; widen to i64 so the dest can be used as i64 elsewhere
        writeln!(out, "  %{d}.i32 = {call_str}").unwrap();
        writeln!(out, "  %{d} = sext i32 %{d}.i32 to i64").unwrap();
    } else {
        writeln!(out, "  {call_str}").unwrap();
    }
}

/// Emit call arguments using the callee's function signature for type info.
fn emit_call_args_typed(
    args: &[Operand],
    callee_name: &str,
    func: &Function,
    ctx: &EmitCtx,
) -> String {
    let sig = ctx.fn_sigs.get(callee_name);
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            let val = operand_ref(a, func);
            // Use callee's param type if available, otherwise infer
            let ty = if let Some(sig) = sig {
                if let Some(param_ty) = sig.param_types.get(i) {
                    llvm_type_str(param_ty, ctx.struct_map)
                } else {
                    infer_operand_type(a, ctx)
                }
            } else {
                infer_operand_type(a, ctx)
            };
            format!("{ty} {val}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Infer the LLVM type of an operand from pre-scanned temp sets.
fn infer_operand_type(op: &Operand, ctx: &EmitCtx) -> String {
    match op {
        Operand::Var(name) => {
            if ctx.string_temps.contains(name) {
                "ptr".into()
            } else if ctx.float_temps.contains(name) {
                "double".into()
            } else if ctx.bool_temps.contains(name) {
                "i1".into()
            } else if let Some(stype) = ctx.struct_temps.get(name.as_str()) {
                ctx.struct_map[stype.as_str()].llvm_name.clone()
            } else {
                "i64".into()
            }
        }
        Operand::Const(c) => match c {
            Constant::Float(_) => "double".into(),
            Constant::Bool(_) => "i1".into(),
            Constant::StringRef(_) => "ptr".into(),
            _ => "i64".into(),
        },
    }
}

/// Check if an operand holds a bool (i1) value.
fn is_bool_operand(op: &Operand, ctx: &EmitCtx) -> bool {
    match op {
        Operand::Const(Constant::Bool(_)) => true,
        Operand::Var(name) => ctx.bool_temps.contains(name),
        _ => false,
    }
}

fn operand_ref(op: &Operand, func: &Function) -> String {
    match op {
        Operand::Var(name) => {
            if is_param(name, func) {
                // Params are loaded from their alloca
                format!("%{name}")
            } else {
                format!("%{name}")
            }
        }
        Operand::Const(c) => match c {
            Constant::Int(n) => n.to_string(),
            Constant::Float(f) => llvm_float_literal(*f),
            Constant::Bool(b) => {
                if *b {
                    "1".into()
                } else {
                    "0".into()
                }
            }
            Constant::StringRef(_) => "null".into(),
            Constant::Unit => "void".into(),
        },
    }
}

fn is_param(name: &str, func: &Function) -> bool {
    func.params.iter().any(|(n, _)| n == name)
}

fn llvm_type_str(
    ty: &Ty,
    struct_map: &std::collections::HashMap<String, StructInfo>,
) -> String {
    match ty {
        Ty::Int => "i64".into(),
        Ty::Float => "double".into(),
        Ty::Bool => "i1".into(),
        Ty::String => "ptr".into(),
        Ty::Unit => "void".into(),
        Ty::Never => "void".into(),
        Ty::Named(name) => {
            if let Some(info) = struct_map.get(name.as_str()) {
                info.llvm_name.clone()
            } else {
                "i64".into() // fallback for ADTs, etc.
            }
        }
        Ty::Generic(..) => {
            // Monomorphized generic type: look up by monomorphized name
            let mono_name = ty.monomorphized_name();
            if let Some(info) = struct_map.get(mono_name.as_str()) {
                info.llvm_name.clone()
            } else {
                "i64".into() // fallback
            }
        }
        _ => "i64".into(), // fallback for unresolved types
    }
}

/// Get the LLVM type for a function parameter by name.
fn param_llvm_type(
    name: &str,
    func: &Function,
    struct_map: &std::collections::HashMap<String, StructInfo>,
) -> String {
    func.params
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, ty)| llvm_type_str(ty, struct_map))
        .unwrap_or_else(|| "i64".into())
}

/// Format a float literal for LLVM IR.
/// LLVM requires a decimal point to distinguish floats from integers in
/// scientific notation (e.g., `3.0e0` is valid, `3e0` is not).
fn llvm_float_literal(f: f64) -> String {
    let s = format!("{f:e}");
    // Ensure there's a decimal point before the 'e'
    if let Some(e_idx) = s.find('e') {
        let mantissa = &s[..e_idx];
        if !mantissa.contains('.') {
            return format!("{mantissa}.0{}", &s[e_idx..]);
        }
    }
    s
}

fn llvm_escape_string(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\n' => out.push_str("\\0A"),
            b'\r' => out.push_str("\\0D"),
            b'\t' => out.push_str("\\09"),
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0 => out.push_str("\\00"),
            0x20..=0x7e => out.push(b as char),
            _ => write!(out, "\\{b:02X}").unwrap(),
        }
    }
    out
}

fn target_triple() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "arm64-apple-macosx14.0.0"
        } else {
            "x86_64-apple-macosx14.0.0"
        }
    } else if cfg!(target_os = "linux") {
        "x86_64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-unknown"
    }
}
