//! List<T> instruction codegen — ListInit, ListLen, ListGet, ListGetSafe.

use std::fmt::Write;

use tyra_mir::*;
use tyra_types::Ty;

use super::codegen::{llvm_type_str, operand_ref, EmitCtx};

/// Emit LLVM IR for List instructions.
/// Returns `true` if `inst` was handled, `false` otherwise.
pub(crate) fn emit_list_instruction(
    out: &mut String,
    inst: &Instruction,
    func: &Function,
    ctx: &EmitCtx,
) -> bool {
    match inst {
        Instruction::ListInit {
            dest,
            elem_type,
            elements,
        } => {
            emit_list_init(out, dest, elem_type, elements, func, ctx);
            true
        }

        Instruction::ListLen { dest, list } => {
            emit_list_len(out, dest, list, func, ctx);
            true
        }

        Instruction::ListGet {
            dest,
            list,
            index,
            elem_type,
        } => {
            emit_list_get(out, dest, list, index, elem_type, func, ctx);
            true
        }

        Instruction::ListGetSafe {
            dest,
            list,
            index,
            elem_type,
        } => {
            emit_list_get_safe(out, dest, list, index, elem_type, func, ctx);
            true
        }

        _ => false,
    }
}

// ── ListInit ───────────────────────────────────────────────────────────

fn emit_list_init(
    out: &mut String,
    dest: &str,
    elem_type: &Ty,
    elements: &[Operand],
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_ty = Ty::Generic("List".into(), vec![elem_type.clone()]);
    let mono = list_ty.monomorphized_name();
    let llvm_struct_ty = &ctx.struct_map[mono.as_str()].llvm_name;
    let elem_llvm_ty = llvm_type_str(elem_type, ctx.struct_map);
    let count = elements.len();

    if count == 0 {
        // Empty list: null pointer, length 0
        writeln!(
            out,
            "  %{dest}.s0 = insertvalue {llvm_struct_ty} undef, ptr null, 0"
        )
        .unwrap();
        writeln!(
            out,
            "  %{dest} = insertvalue {llvm_struct_ty} %{dest}.s0, i64 0, 1"
        )
        .unwrap();
    } else {
        let elem_size = llvm_elem_size(elem_type);
        let total_size = count * elem_size;
        writeln!(out, "  %{dest}.ptr = call ptr @malloc(i64 {total_size})").unwrap();
        // Null check + abort on OOM
        writeln!(out, "  %{dest}.null = icmp eq ptr %{dest}.ptr, null").unwrap();
        writeln!(
            out,
            "  br i1 %{dest}.null, label %{dest}.oom, label %{dest}.ok"
        )
        .unwrap();
        writeln!(out, "{dest}.oom:").unwrap();
        writeln!(out, "  call void @abort()").unwrap();
        writeln!(out, "  unreachable").unwrap();
        writeln!(out, "{dest}.ok:").unwrap();

        // Store each element via GEP
        for (i, elem) in elements.iter().enumerate() {
            let val = operand_ref(elem, func);
            writeln!(
                out,
                "  %{dest}.gep.{i} = getelementptr {elem_llvm_ty}, ptr %{dest}.ptr, i64 {i}"
            )
            .unwrap();
            writeln!(out, "  store {elem_llvm_ty} {val}, ptr %{dest}.gep.{i}").unwrap();
        }

        // Build struct {ptr, i64}
        writeln!(
            out,
            "  %{dest}.s0 = insertvalue {llvm_struct_ty} undef, ptr %{dest}.ptr, 0"
        )
        .unwrap();
        writeln!(
            out,
            "  %{dest} = insertvalue {llvm_struct_ty} %{dest}.s0, i64 {count}, 1"
        )
        .unwrap();
    }
}

// ── ListLen ────────────────────────────────────────────────────────────

fn emit_list_len(
    out: &mut String,
    dest: &str,
    list: &Operand,
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_val = operand_ref(list, func);
    // ListLen doesn't carry elem_type, so we must rely on struct_temps.
    // All List<T> have the same physical layout {ptr, i64}, so fallback is safe.
    let llvm_struct_ty = list_struct_type(list, ctx);
    writeln!(
        out,
        "  %{dest} = extractvalue {llvm_struct_ty} {list_val}, 1"
    )
    .unwrap();
}

// ── ListGet ────────────────────────────────────────────────────────────

fn emit_list_get(
    out: &mut String,
    dest: &str,
    list: &Operand,
    index: &Operand,
    elem_type: &Ty,
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_val = operand_ref(list, func);
    let idx_val = operand_ref(index, func);
    let elem_llvm_ty = llvm_type_str(elem_type, ctx.struct_map);
    let llvm_struct_ty = list_struct_type_from_elem(elem_type, list, ctx);

    // Extract ptr and len
    writeln!(
        out,
        "  %{dest}.data = extractvalue {llvm_struct_ty} {list_val}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.len = extractvalue {llvm_struct_ty} {list_val}, 1"
    )
    .unwrap();

    // Bounds check: index < len (unsigned)
    writeln!(
        out,
        "  %{dest}.inb = icmp ult i64 {idx_val}, %{dest}.len"
    )
    .unwrap();
    writeln!(
        out,
        "  br i1 %{dest}.inb, label %{dest}.ok, label %{dest}.oob"
    )
    .unwrap();

    // Out-of-bounds: abort
    writeln!(out, "{dest}.oob:").unwrap();
    writeln!(out, "  call void @abort()").unwrap();
    writeln!(out, "  unreachable").unwrap();

    // In-bounds: GEP + load
    writeln!(out, "{dest}.ok:").unwrap();
    writeln!(
        out,
        "  %{dest}.gep = getelementptr {elem_llvm_ty}, ptr %{dest}.data, i64 {idx_val}"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest} = load {elem_llvm_ty}, ptr %{dest}.gep"
    )
    .unwrap();
}

// ── ListGetSafe ────────────────────────────────────────────────────────

fn emit_list_get_safe(
    out: &mut String,
    dest: &str,
    list: &Operand,
    index: &Operand,
    elem_type: &Ty,
    func: &Function,
    ctx: &EmitCtx,
) {
    let list_val = operand_ref(list, func);
    let idx_val = operand_ref(index, func);
    let elem_llvm_ty = llvm_type_str(elem_type, ctx.struct_map);

    let opt_ty = Ty::Generic("Option".into(), vec![elem_type.clone()]);
    let opt_mono = opt_ty.monomorphized_name();
    let opt_llvm_ty = &ctx.struct_map[opt_mono.as_str()].llvm_name;

    let llvm_struct_ty = list_struct_type_from_elem(elem_type, list, ctx);

    // Extract ptr and len
    writeln!(
        out,
        "  %{dest}.data = extractvalue {llvm_struct_ty} {list_val}, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.len = extractvalue {llvm_struct_ty} {list_val}, 1"
    )
    .unwrap();

    // Bounds check
    writeln!(
        out,
        "  %{dest}.inb = icmp ult i64 {idx_val}, %{dest}.len"
    )
    .unwrap();

    // Alloca for result
    writeln!(out, "  %{dest}.slot = alloca {opt_llvm_ty}").unwrap();
    writeln!(
        out,
        "  br i1 %{dest}.inb, label %{dest}.some, label %{dest}.none"
    )
    .unwrap();

    // Some path: load element, wrap in Option(tag=0, value=elem)
    writeln!(out, "{dest}.some:").unwrap();
    writeln!(
        out,
        "  %{dest}.gep = getelementptr {elem_llvm_ty}, ptr %{dest}.data, i64 {idx_val}"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.elem = load {elem_llvm_ty}, ptr %{dest}.gep"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.some.s0 = insertvalue {opt_llvm_ty} undef, i8 0, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.some.val = insertvalue {opt_llvm_ty} %{dest}.some.s0, {elem_llvm_ty} %{dest}.elem, 1"
    )
    .unwrap();
    writeln!(
        out,
        "  store {opt_llvm_ty} %{dest}.some.val, ptr %{dest}.slot"
    )
    .unwrap();
    writeln!(out, "  br label %{dest}.end").unwrap();

    // None path: Option(tag=1, value=zero)
    writeln!(out, "{dest}.none:").unwrap();
    let zero_val = llvm_zero_val(elem_type, &elem_llvm_ty);
    writeln!(
        out,
        "  %{dest}.none.s0 = insertvalue {opt_llvm_ty} undef, i8 1, 0"
    )
    .unwrap();
    writeln!(
        out,
        "  %{dest}.none.val = insertvalue {opt_llvm_ty} %{dest}.none.s0, {elem_llvm_ty} {zero_val}, 1"
    )
    .unwrap();
    writeln!(
        out,
        "  store {opt_llvm_ty} %{dest}.none.val, ptr %{dest}.slot"
    )
    .unwrap();
    writeln!(out, "  br label %{dest}.end").unwrap();

    // Merge
    writeln!(out, "{dest}.end:").unwrap();
    writeln!(
        out,
        "  %{dest} = load {opt_llvm_ty}, ptr %{dest}.slot"
    )
    .unwrap();
}

// ── Helper functions ───────────────────────────────────────────────────

fn llvm_elem_size(ty: &Ty) -> usize {
    match ty {
        Ty::Bool => 1,
        _ => 8, // i64, double, ptr
    }
}

/// LLVM zero/null value for a type, used in Option None payloads.
fn llvm_zero_val(ty: &Ty, llvm_ty_str: &str) -> &'static str {
    match ty {
        Ty::String => "null",
        Ty::Float => "0.0",
        Ty::Bool => "0",
        _ if llvm_ty_str.starts_with("%struct.") => "zeroinitializer",
        _ => "0",
    }
}

/// Resolve the LLVM struct type name for a List operand.
/// Prefers struct_temps lookup; falls back to deriving from elem_type.
fn list_struct_type_from_elem(
    elem_type: &Ty,
    list: &Operand,
    ctx: &EmitCtx,
) -> String {
    if let Operand::Var(name) = list {
        if let Some(stype) = ctx.struct_temps.get(name.as_str()) {
            return ctx.struct_map[stype.as_str()].llvm_name.clone();
        }
    }
    // Derive from elem_type: List<T> → "List__T"
    let list_ty = Ty::Generic("List".into(), vec![elem_type.clone()]);
    let mono = list_ty.monomorphized_name();
    ctx.struct_map
        .get(mono.as_str())
        .map(|info| info.llvm_name.clone())
        .unwrap_or_else(|| format!("%struct.{mono}"))
}

/// Resolve the LLVM struct type name for a List operand (when elem_type is unknown).
fn list_struct_type(list: &Operand, ctx: &EmitCtx) -> String {
    if let Operand::Var(name) = list {
        if let Some(stype) = ctx.struct_temps.get(name.as_str()) {
            return ctx.struct_map[stype.as_str()].llvm_name.clone();
        }
    }
    // All List<T> have identical physical layout {ptr, i64}, so any is valid.
    "%struct.List__Int".into()
}
