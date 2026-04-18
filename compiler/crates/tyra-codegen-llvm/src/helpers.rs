// LLVM IR emission helpers: operand references, type strings, escaping.
//
// Extracted from codegen.rs to keep the main module focused on orchestration.

use std::fmt::Write;

use tyra_mir::*;
use tyra_types::Ty;

use crate::codegen::{EmitCtx, StructInfo};

/// Infer the LLVM type string for an operand based on pre-scanned type sets.
pub(crate) fn infer_operand_type(op: &Operand, ctx: &EmitCtx) -> String {
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
pub(crate) fn is_bool_operand(op: &Operand, ctx: &EmitCtx) -> bool {
    match op {
        Operand::Const(Constant::Bool(_)) => true,
        Operand::Var(name) => ctx.bool_temps.contains(name),
        _ => false,
    }
}

/// Render an operand as its LLVM IR reference (e.g. `%name` or a literal).
pub(crate) fn operand_ref(op: &Operand, _func: &Function) -> String {
    match op {
        Operand::Var(name) => format!("%{name}"),
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
            Constant::Unit => "0".into(),
        },
    }
}

pub(crate) fn is_param(name: &str, func: &Function) -> bool {
    func.params.iter().any(|(n, _)| n == name)
}

/// Convert a Tyra `Ty` into an LLVM type string.
/// Data types (§8.6) are heap-allocated and rendered as `ptr`.
pub(crate) fn llvm_type_str(
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
                if info.is_data {
                    "ptr".into() // data types are heap-allocated pointers (§8.6)
                } else {
                    info.llvm_name.clone()
                }
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
pub(crate) fn param_llvm_type(
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
pub(crate) fn llvm_float_literal(f: f64) -> String {
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

pub(crate) fn llvm_escape_string(s: &str) -> String {
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

pub(crate) fn target_triple() -> &'static str {
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
