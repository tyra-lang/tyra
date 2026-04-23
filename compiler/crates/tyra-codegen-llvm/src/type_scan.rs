// Pre-scan passes that compute type metadata for LLVM emission:
// - Which SSA temps hold primitive values (string/float/bool)
// - Which SSA temps hold struct values (for insertvalue/extractvalue)
// - Which alloca slots hold which LLVM type
//
// Extracted from codegen.rs to keep the main module focused on orchestration.

use std::collections::{HashMap, HashSet};

use tyra_mir::*;
use tyra_types::Ty;

use crate::codegen::{FnSig, StructInfo};

/// Return the primitive return type of a builtin intrinsic, if any.
///
/// Covers intrinsics that are matched by name in `builtins::emit_builtin_call`
/// but are not user-visible functions (no entry in `Program.functions` or
/// `fn_sigs`). Keeping the list centralized prevents string/float/bool tracking
/// from drifting each time a new intrinsic lands.
///
/// Returns `None` for intrinsics whose return is `Int`/`Unit` (default LLVM
/// `i64`/`void` — no temp-map tracking needed) or whose return is a struct
/// type (handled by `pre_scan_struct_types` instead, e.g. `parse__Int`).
fn builtin_primitive_return(fname: &str) -> Option<Ty> {
    match fname {
        // M10 phase 1: fs stdlib intrinsics.
        "__fs_read_raw" | "__fs_errmsg" => Some(Ty::String),
        "__fs_exists" => Some(Ty::Bool),
        // M11 phase 1: http client intrinsics.
        "__http_body" | "__http_errmsg" => Some(Ty::String),
        // __http_get / __http_status / __http_errno return Int (default path).
        // __fs_write_raw returns Unit (no tracking), __fs_errno returns Int
        // (default i64 path — no tracking needed).
        // M10 phase 2: json stdlib intrinsics.
        "__json_err_msg" | "__json_kind" | "__json_str" => Some(Ty::String),
        // Bool-returning intrinsics. `__json_bool` (the value accessor) returns
        // i32 in the runtime and is widened to i1 by `emit_json_i64_to_bool` —
        // the codegen path differs from `__json_int`, which stays as i64.
        "__json_is_string" | "__json_is_int" | "__json_is_bool" | "__json_bool" => Some(Ty::Bool),
        // Int-returning intrinsics (i64 default path, no tracking needed):
        //   __json_parse, __json_int, __json_get, __json_at,
        //   __json_err_line, __json_err_col.
        // stdin intrinsics.
        "__io_read_line" | "__io_read_to_end" => Some(Ty::String),
        "__io_eof" => Some(Ty::Bool),
        // §17.3.4: string stdlib intrinsics.
        "__string_trim" | "__string_to_upper" | "__string_to_lower" => Some(Ty::String),
        "__string_is_empty"
        | "__string_contains"
        | "__string_starts_with"
        | "__string_ends_with" => Some(Ty::Bool),
        // __string_len / __string_parse_int / __string_parse_errno return
        // Int (default i64 path — no tracking needed).
        // §17.3.5: list stdlib intrinsics (List<Int> only). Int / List<Int> /
        // Option<Int> returns are tracked via struct_temps in
        // pre_scan_struct_types; only the Bool return needs registration here.
        "__list_int_contains" => Some(Ty::Bool),
        _ => None,
    }
}

/// All type metadata computed by pre-scan for a single function.
/// Consumed by emit_instruction (indirectly via EmitCtx).
pub(crate) struct ScanResult {
    pub string_temps: HashSet<String>,
    pub float_temps: HashSet<String>,
    pub bool_temps: HashSet<String>,
    pub struct_temps: HashMap<String, String>,
    pub alloca_llvm_types: HashMap<String, String>,
}

/// Run the full pre-scan pipeline for a function:
/// 1. Primitive temp scan (string/float/bool/data-type-ptr)
/// 2. Struct temp scan (value-type struct SSA temps)
/// 3. Alloca LLVM type determination
/// 4. Load propagation and fixed-point iteration
///
/// This replaces the inline pre-scan block in emit_function.
pub(crate) fn scan_function_types(
    func: &Function,
    struct_map: &HashMap<String, StructInfo>,
    fn_sigs: &HashMap<String, FnSig>,
) -> ScanResult {
    let (mut string_temps, mut float_temps, mut bool_temps) =
        scan_primitive_temps(func, struct_map, fn_sigs);
    let struct_temps = pre_scan_struct_types(func, struct_map, fn_sigs);
    let mut alloca_llvm_types = pre_scan_alloca_llvm_types(
        func,
        &string_temps,
        &float_temps,
        &bool_temps,
        &struct_temps,
        struct_map,
    );
    propagate_types(
        func,
        &mut string_temps,
        &mut float_temps,
        &mut bool_temps,
        &mut alloca_llvm_types,
    );
    ScanResult {
        string_temps,
        float_temps,
        bool_temps,
        struct_temps,
        alloca_llvm_types,
    }
}

/// Scan function params and instructions to collect primitive-typed SSA temps.
/// Returns (string_temps, float_temps, bool_temps).
/// Data types (§8.6) are tracked as `string_temps` because they're ptrs.
fn scan_primitive_temps(
    func: &Function,
    struct_map: &HashMap<String, StructInfo>,
    fn_sigs: &HashMap<String, FnSig>,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let mut string_temps: HashSet<String> = HashSet::new();
    let mut float_temps: HashSet<String> = HashSet::new();
    let mut bool_temps: HashSet<String> = HashSet::new();

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
            Ty::Named(type_name) => {
                if struct_map.get(type_name.as_str()).map(|i| i.is_data).unwrap_or(false) {
                    string_temps.insert(name.clone()); // data type ptr treated as ptr
                }
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
            Instruction::StructInit { dest, type_name, .. } => {
                if struct_map.get(type_name.as_str()).map(|i| i.is_data).unwrap_or(false) {
                    string_temps.insert(dest.clone()); // data type StructInit result is a ptr
                }
            }
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
                        | MirBinOp::EqString
                        | MirBinOp::NeqString
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
                        } else if let Ty::Named(ft_name) = field_ty {
                            // Field is itself a data type: result is a ptr
                            if struct_map.get(ft_name.as_str()).map(|i| i.is_data).unwrap_or(false) {
                                string_temps.insert(dest.clone());
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
                // Builtins with primitive return types that are not part of
                // fn_sigs (not emitted as user-visible functions). Centralized
                // here so new intrinsics only need one line.
                match builtin_primitive_return(fname.as_str()) {
                    Some(Ty::String) => {
                        string_temps.insert(dest.clone());
                    }
                    Some(Ty::Float) => {
                        float_temps.insert(dest.clone());
                    }
                    Some(Ty::Bool) => {
                        bool_temps.insert(dest.clone());
                    }
                    _ => {}
                }
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
                        Ty::Named(type_name) => {
                            // If return type is a data type, result is a ptr
                            if struct_map.get(type_name.as_str()).map(|i| i.is_data).unwrap_or(false) {
                                string_temps.insert(dest.clone());
                            }
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
                // Track payload type from the specified field of the ADT struct.
                // Note: this scan handles primitive/data-type payloads.
                // The complementary pre_scan_struct_types handles value-type struct payloads.
                // Both scans are needed: this one populates string_temps/float_temps/bool_temps,
                // while pre_scan_struct_types populates struct_temps (for value-type structs).
                // Inserts to string_temps are idempotent, so the overlap is harmless.
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        if *field_ty == Ty::String {
                            string_temps.insert(dest.clone());
                        } else if *field_ty == Ty::Float {
                            float_temps.insert(dest.clone());
                        } else if *field_ty == Ty::Bool {
                            bool_temps.insert(dest.clone());
                        } else if let Ty::Named(ft_name) = field_ty {
                            // If payload type is a data type, result is a ptr
                            if struct_map.get(ft_name.as_str()).map(|i| i.is_data).unwrap_or(false) {
                                string_temps.insert(dest.clone());
                            }
                        }
                    }
                }
            }
            // Task handles returned by `spawn` are carried as i64 in the
            // MIR so they can flow through lists and generic code uniformly
            // (§14.4, M9). Codegen ptrtoints the real runtime handle into
            // the Spawn dest. We therefore do NOT mark Spawn dest as a
            // string_temp / ptr; it is a plain i64.
            Instruction::Spawn { .. } => {}
            // Await result type governs the unbox; for String/data/Task the
            // dest is a ptr; otherwise fall through to i64/float/bool scans.
            Instruction::Await {
                dest, result_type, ..
            } => match result_type {
                Ty::String => {
                    string_temps.insert(dest.clone());
                }
                Ty::Float => {
                    float_temps.insert(dest.clone());
                }
                Ty::Bool => {
                    bool_temps.insert(dest.clone());
                }
                Ty::Named(name) => {
                    if struct_map
                        .get(name.as_str())
                        .map(|i| i.is_data)
                        .unwrap_or(false)
                    {
                        string_temps.insert(dest.clone());
                    }
                }
                Ty::Generic(_, _) => {
                    // Task<Option<..>>/Task<Result<..>> unbox to ADT ptrs.
                    string_temps.insert(dest.clone());
                }
                _ => {}
            },
            Instruction::ListGet {
                dest, elem_type, ..
            } => match elem_type {
                Ty::String => {
                    string_temps.insert(dest.clone());
                }
                Ty::Float => {
                    float_temps.insert(dest.clone());
                }
                Ty::Bool => {
                    bool_temps.insert(dest.clone());
                }
                Ty::Named(n) => {
                    // Data types (§8.6) are GC-heap ptrs; track as string_temps
                    // so Copy / Store use the ptr round-trip path rather
                    // than the default `add i64` that would smuggle the
                    // pointer through an i64 register (for List<User> etc.).
                    // Value types stay untracked here — pre_scan_struct_types
                    // places them into struct_temps with the full type name
                    // for llvm_type resolution in Copy / Store emit.
                    if struct_map
                        .get(n.as_str())
                        .map(|i| i.is_data)
                        .unwrap_or(false)
                    {
                        string_temps.insert(dest.clone());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    (string_temps, float_temps, bool_temps)
}

/// Propagate primitive types through Load/Store/Copy until the maps reach
/// a fixed point. Mutates all three temp sets and alloca_llvm_types in place.
fn propagate_types(
    func: &Function,
    string_temps: &mut HashSet<String>,
    float_temps: &mut HashSet<String>,
    bool_temps: &mut HashSet<String>,
    alloca_llvm_types: &mut HashMap<String, String>,
) {
    // Initial propagation: alloca types → Load destinations
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

    // Iterate: after load propagation, newly typed temps may reveal alloca types
    // that were unknown before (e.g. String loaded from ptr alloca stored into
    // a match-result alloca). Repeat until stable.
    loop {
        let mut changed = false;
        // Re-scan Store instructions: allow upgrading unknown/untyped alloca slots.
        // Removing "first store wins" guard so later Stores can refine the type.
        for inst in &func.body {
            if let Instruction::Store { dest, value } = inst {
                if let Operand::Var(name) = value {
                    let new_ty = if string_temps.contains(name) {
                        Some("ptr")
                    } else if float_temps.contains(name) {
                        Some("double")
                    } else if bool_temps.contains(name) {
                        Some("i1")
                    } else {
                        None
                    };
                    if let Some(ty) = new_ty {
                        let old = alloca_llvm_types.insert(dest.clone(), ty.into());
                        if old.as_deref() != Some(ty) {
                            changed = true;
                        }
                    }
                }
            }
        }
        // Propagate newly discovered alloca types to Load destinations
        for inst in &func.body {
            if let Instruction::Load { dest, source } = inst {
                if string_temps.contains(dest)
                    || float_temps.contains(dest)
                    || bool_temps.contains(dest)
                {
                    continue; // already typed
                }
                if let Some(ty) = alloca_llvm_types.get(source.as_str()) {
                    match ty.as_str() {
                        "ptr" => {
                            string_temps.insert(dest.clone());
                            changed = true;
                        }
                        "double" => {
                            float_temps.insert(dest.clone());
                            changed = true;
                        }
                        "i1" => {
                            bool_temps.insert(dest.clone());
                            changed = true;
                        }
                        _ => {}
                    }
                }
            }
        }
        // Propagate through Copy instructions (e.g. let name = <match result>).
        // Use independent checks (not else-if) for consistency with the initial scan.
        for inst in &func.body {
            if let Instruction::Copy { dest, source } = inst {
                if string_temps.contains(source.as_str()) && string_temps.insert(dest.clone()) {
                    changed = true;
                }
                if float_temps.contains(source.as_str()) && float_temps.insert(dest.clone()) {
                    changed = true;
                }
                if bool_temps.contains(source.as_str()) && bool_temps.insert(dest.clone()) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Pre-scan function body to track which SSA temps hold struct-typed values.
/// Returns struct_temps: SSA temp name → MIR struct type name.
///
/// `alloca_types` is maintained internally to propagate struct types through
/// Store→Load chains, but the final map is not exposed because callers rely
/// on `pre_scan_alloca_llvm_types` for alloca typing instead.
///
/// Data types (§8.6) are excluded here — they are tracked as `string_temps` (ptrs)
/// by the caller scan.
fn pre_scan_struct_types(
    func: &Function,
    struct_map: &HashMap<String, StructInfo>,
    fn_sigs: &HashMap<String, FnSig>,
) -> HashMap<String, String> {
    let mut struct_temps: HashMap<String, String> = HashMap::new();
    let mut alloca_types: HashMap<String, String> = HashMap::new();

    // Function params that are struct-typed (value types only; data types go to string_temps)
    for (name, ty) in &func.params {
        if let Ty::Named(type_name) = ty {
            if let Some(info) = struct_map.get(type_name.as_str()) {
                if !info.is_data {
                    struct_temps.insert(name.clone(), type_name.clone());
                }
                // data types already tracked as string_temps (ptr) in the caller
            }
        }
        // Generic params (List<T>, Option<T>, Result<T,E>)
        if let Ty::Generic(..) = ty {
            let mono = ty.monomorphized_name();
            if struct_map.contains_key(mono.as_str()) {
                struct_temps.insert(name.clone(), mono);
            }
        }
    }

    // Scan instructions
    for inst in &func.body {
        match inst {
            Instruction::StructInit {
                dest, type_name, ..
            } => {
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if !info.is_data {
                        struct_temps.insert(dest.clone(), type_name.clone());
                    }
                    // data types tracked as string_temps (ptr) in the caller scan
                }
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
                // Check if the extracted field is itself a value-type struct
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        if let Ty::Named(ft_name) = field_ty {
                            if let Some(ft_info) = struct_map.get(ft_name.as_str()) {
                                if !ft_info.is_data {
                                    struct_temps.insert(dest.clone(), ft_name.clone());
                                }
                                // data type fields are ptrs, tracked as string_temps in caller
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
                // Built-in functions with struct return types
                match fname.as_str() {
                    "sys__args" => {
                        if struct_map.contains_key("List__String") {
                            struct_temps.insert(dest.clone(), "List__String".into());
                        }
                    }
                    "parse__Int" => {
                        if struct_map.contains_key("Option__Int") {
                            struct_temps.insert(dest.clone(), "Option__Int".into());
                        }
                    }
                    // §17.3.5: list stdlib intrinsics returning aggregate types.
                    "__list_int_push" => {
                        if struct_map.contains_key("List__Int") {
                            struct_temps.insert(dest.clone(), "List__Int".into());
                        }
                    }
                    "__list_int_max" | "__list_int_min" | "__list_int_index_of" => {
                        if struct_map.contains_key("Option__Int") {
                            struct_temps.insert(dest.clone(), "Option__Int".into());
                        }
                    }
                    _ => {}
                }
                // Check if the called function returns a value-type struct (not data types)
                if let Some(sig) = fn_sigs.get(fname.as_str()) {
                    if let Ty::Named(type_name) = &sig.return_type {
                        if let Some(ret_info) = struct_map.get(type_name.as_str()) {
                            if !ret_info.is_data {
                                struct_temps.insert(dest.clone(), type_name.clone());
                            }
                            // data type return values are ptrs, tracked as string_temps in caller
                        }
                    }
                    // Also check for generic return types (Option/Result/List)
                    if sig.return_type.is_option() || sig.return_type.is_result() {
                        let mono_name = sig.return_type.monomorphized_name();
                        if struct_map.contains_key(mono_name.as_str()) {
                            struct_temps.insert(dest.clone(), mono_name);
                        }
                    }
                    // List<T> returns (e.g. `list.push(_) -> List<Int>` from the
                    // §17.3.5 stdlib wrappers): propagate the monomorphized
                    // struct so downstream Copy / Store uses struct-aware paths.
                    if let Ty::Generic(name, _) = &sig.return_type {
                        if name == "List" {
                            let mono_name = sig.return_type.monomorphized_name();
                            if struct_map.contains_key(mono_name.as_str()) {
                                struct_temps.insert(dest.clone(), mono_name);
                            }
                        }
                    }
                }
            }
            Instruction::ListInit {
                dest, elem_type, ..
            } => {
                let list_ty = Ty::Generic("List".into(), vec![elem_type.clone()]);
                let mono = list_ty.monomorphized_name();
                if struct_map.contains_key(mono.as_str()) {
                    struct_temps.insert(dest.clone(), mono);
                }
            }
            Instruction::JoinAll {
                dest, elem_type, ..
            } => {
                // JoinAll produces a List<T> struct (M9).
                let list_ty = Ty::Generic("List".into(), vec![elem_type.clone()]);
                let mono = list_ty.monomorphized_name();
                if struct_map.contains_key(mono.as_str()) {
                    struct_temps.insert(dest.clone(), mono);
                }
            }
            Instruction::ListGetSafe {
                dest, elem_type, ..
            } => {
                let opt_ty = Ty::Generic("Option".into(), vec![elem_type.clone()]);
                let mono = opt_ty.monomorphized_name();
                if struct_map.contains_key(mono.as_str()) {
                    struct_temps.insert(dest.clone(), mono);
                }
            }
            // ListGet on an element that is itself a struct (nested List,
            // Option/Result/user value-type) needs struct_temps tracking so
            // downstream Copy / Store uses the struct-aware codegen path.
            // Without this, `for row in matrix` (List<List<Int>>) lowered
            // the outer Copy to `add i64` and tripped E0500.
            // Data types remain `ptr` and are tracked by the caller scan.
            Instruction::ListGet {
                dest, elem_type, ..
            } => match elem_type {
                Ty::Named(n) => {
                    if let Some(info) = struct_map.get(n.as_str()) {
                        if !info.is_data {
                            struct_temps.insert(dest.clone(), n.clone());
                        }
                    }
                }
                Ty::Generic(_, _) => {
                    let mono = elem_type.monomorphized_name();
                    if struct_map.contains_key(mono.as_str()) {
                        struct_temps.insert(dest.clone(), mono);
                    }
                }
                _ => {}
            },
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
                    // Only propagate to struct_temps for value types (data types are ptrs)
                    if struct_map.get(stype.as_str()).map(|i| !i.is_data).unwrap_or(true) {
                        struct_temps.insert(dest.clone(), stype);
                    }
                }
            }
            Instruction::AdtPayload {
                dest,
                type_name,
                field_index,
                ..
            } => {
                // If the extracted payload field is a value-type struct, track it.
                // Data types are ptrs — tracked as string_temps in the caller scan.
                if let Some(info) = struct_map.get(type_name.as_str()) {
                    if let Some(field_ty) = info.field_types.get(*field_index as usize) {
                        match field_ty {
                            Ty::Named(ft_name) => {
                                if let Some(ft_info) = struct_map.get(ft_name.as_str()) {
                                    if !ft_info.is_data {
                                        struct_temps.insert(dest.clone(), ft_name.clone());
                                    }
                                    // data type ptrs tracked as string_temps in caller
                                }
                            }
                            Ty::Generic(..) => {
                                let mono = field_ty.monomorphized_name();
                                if struct_map.contains_key(mono.as_str()) {
                                    struct_temps.insert(dest.clone(), mono);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    struct_temps
}

/// Pre-scan function body to determine the LLVM type for each alloca slot.
/// This handles match result allocas that may store strings, floats, or structs.
fn pre_scan_alloca_llvm_types(
    func: &Function,
    string_temps: &HashSet<String>,
    float_temps: &HashSet<String>,
    bool_temps: &HashSet<String>,
    struct_temps: &HashMap<String, String>,
    struct_map: &HashMap<String, StructInfo>,
) -> HashMap<String, String> {
    let mut alloca_llvm_types: HashMap<String, String> = HashMap::new();

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
