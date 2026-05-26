// LLVM IR text generation from MIR.
//
// Generates valid LLVM IR text that can be compiled with:
//   clang output.ll -o output
//
#![allow(clippy::collapsible_if)]
// For Milestone 1a, we use C library functions (puts, printf) for I/O.
// The Tyra runtime will replace these in later milestones.

use std::fmt::Write;

use tyra_mir::*;
use tyra_types::Ty;

use crate::coverage::{
    CovMap, build_cov_map, emit_counter_global, emit_cov_extern, emit_cov_increment,
    emit_cov_init_call, write_covmap_text,
};
use crate::dwarf::{DwarfCtx, patch_dbg_on_last_instruction};
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
    /// Per-field "recursive self-reference" flag for ADTs. A true entry
    /// instructs codegen to emit the field as an opaque `ptr` (GC-heap
    /// box) rather than the structural LLVM type. Non-ADT structs have
    /// this vector zero-filled and can ignore it.
    pub(crate) recursive_fields: Vec<bool>,
}

/// Generate LLVM IR text with coverage instrumentation (non-debug build).
/// Returns `(llvm_ir, covmap_text)`.  The covmap text must be written to
/// `<output_binary>.tyra-covmap` by the caller (e.g. the driver).
pub fn emit_llvm_ir_coverage(program: &Program) -> (String, String) {
    let cov_map = build_cov_map(program);
    let covmap_text = write_covmap_text(&cov_map, &program.source_files);
    let ir = emit_llvm_ir_impl(program, Some(&cov_map), false);
    (ir, covmap_text)
}

/// Generate LLVM IR text with DWARF debug info (ADR-0014 §4a).
/// Use for debug (non-release) builds to enable lldb breakpoints and step.
pub fn emit_llvm_ir_debug(program: &Program) -> String {
    emit_llvm_ir_impl(program, None, true)
}

/// Generate LLVM IR text (non-coverage, non-debug build).
pub fn emit_llvm_ir(program: &Program) -> String {
    emit_llvm_ir_impl(program, None, false)
}

fn emit_llvm_ir_impl(program: &Program, cov_map: Option<&CovMap>, emit_dwarf: bool) -> String {
    let mut out = String::new();

    // Build DWARF metadata context if debug info is requested (ADR-0014 §4a).
    let mut dwarf_ctx: Option<DwarfCtx> = if emit_dwarf {
        Some(DwarfCtx::build(program))
    } else {
        None
    };

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
                recursive_fields: sd.recursive_fields.clone(),
            };
            (sd.name.clone(), info)
        })
        .collect();

    // Module header
    writeln!(out, "; Tyra compiler output").unwrap();
    writeln!(out, "target triple = \"{}\"", target_triple()).unwrap();
    writeln!(out).unwrap();

    // Coverage: counter global array (must appear before any function that uses it).
    if let Some(cm) = cov_map {
        emit_counter_global(&mut out, cm.n);
        writeln!(out).unwrap();
    }

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
                } else if info.recursive_fields.get(i).copied().unwrap_or(false) {
                    // Recursive self-reference: boxed as GC-heap ptr.
                    "ptr".into()
                } else {
                    let ty_str = llvm_type_str(ty, &struct_map);
                    // Unit (void) is not valid as a struct field; use i64 placeholder
                    if ty_str == "void" {
                        "i64".into()
                    } else {
                        ty_str
                    }
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

    // Closure fat pointer struct (ADR-0011): { fn_ptr: ptr, env_ptr: ptr }.
    // Always declared so indirect-call emission can reference the type unconditionally.
    writeln!(out, "%struct.__closure_fat = type {{ ptr, ptr }}").unwrap();
    writeln!(out).unwrap();

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

    // Source file name constants (ADR 0014) — referenced by panic location output.
    for (idx, path) in program.source_files.iter().enumerate() {
        let escaped = llvm_escape_string(path);
        let len = path.len() + 1;
        writeln!(
            out,
            "@.src.{idx} = private unnamed_addr constant [{len} x i8] c\"{escaped}\\00\""
        )
        .unwrap();
    }
    if !program.source_files.is_empty() {
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
    // Panic location format: "panic at %s:%ld:\n" (ADR 0014)
    // Byte count: "panic at %s:%ld:\n\0" = 9 + 7 + 1 + 1 = 18
    writeln!(
        out,
        "@.fmt.panic_loc = private unnamed_addr constant [18 x i8] c\"panic at %s:%ld:\\0A\\00\""
    )
    .unwrap();
    // "%s\n" for stderr message line
    writeln!(
        out,
        "@.fmt.str_ln = private unnamed_addr constant [4 x i8] c\"%s\\0A\\00\""
    )
    .unwrap();
    // Sentinel written to stderr before exit(101) so the test runner can distinguish
    // intentional panic() from sys.exit(101) (ADR 0012).
    // "__TYRA_PANIC__\n\0" = 16 bytes
    writeln!(
        out,
        "@.str.panic_sentinel = private unnamed_addr constant [16 x i8] c\"__TYRA_PANIC__\\0A\\00\""
    )
    .unwrap();
    writeln!(out).unwrap();

    // External declarations
    writeln!(out, "; External declarations").unwrap();
    // Coverage runtime (only declared when coverage is active, but harmless if always present).
    if cov_map.is_some() {
        emit_cov_extern(&mut out);
    }
    // DWARF local variable intrinsic (ADR-0014 §4a-ii).
    if emit_dwarf {
        writeln!(out, "declare void @llvm.dbg.declare(metadata, metadata, metadata)").unwrap();
    }
    writeln!(out, "declare i32 @puts(ptr)").unwrap();
    writeln!(out, "declare i32 @printf(ptr, ...)").unwrap();
    writeln!(out, "declare i32 @snprintf(ptr, i64, ptr, ...)").unwrap();
    writeln!(out, "declare i32 @dprintf(i32, ptr, ...)").unwrap();
    // Boehm GC (libgc): tracing conservative collector. See ADR-0007.
    // All heap allocations go through GC_malloc; GC_init is called at @main entry.
    writeln!(out, "declare ptr @GC_malloc(i64)").unwrap();
    writeln!(out, "declare void @GC_init()").unwrap();
    // Tyra async runtime (libtyra_runtime.a). See runtime/ crate.
    // tyra_rt_init is invoked at @main entry after GC_init.
    writeln!(out, "declare void @tyra_rt_init()").unwrap();
    writeln!(out, "declare ptr @tyra_task_spawn(ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_task_await(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_task_select(ptr, i64)").unwrap();
    // M10 phase 1: fs stdlib. See runtime/src/stdlib_fs.rs.
    writeln!(out, "declare ptr @tyra_fs_read(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_fs_errno()").unwrap();
    writeln!(out, "declare ptr @tyra_fs_errmsg()").unwrap();
    writeln!(out, "declare void @tyra_fs_write(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_fs_exists(ptr)").unwrap();
    // M10 phase 2: json stdlib. See runtime/src/stdlib_json.rs.
    writeln!(out, "declare i64 @tyra_json_parse(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_json_err_msg()").unwrap();
    writeln!(out, "declare i64 @tyra_json_err_line()").unwrap();
    writeln!(out, "declare i64 @tyra_json_err_col()").unwrap();
    writeln!(out, "declare ptr @tyra_json_kind(i64)").unwrap();
    writeln!(out, "declare i32 @tyra_json_is_string(i64)").unwrap();
    writeln!(out, "declare i32 @tyra_json_is_int(i64)").unwrap();
    writeln!(out, "declare i32 @tyra_json_is_bool(i64)").unwrap();
    writeln!(out, "declare ptr @tyra_json_str(i64)").unwrap();
    writeln!(out, "declare i64 @tyra_json_int(i64)").unwrap();
    writeln!(out, "declare i32 @tyra_json_bool(i64)").unwrap();
    writeln!(out, "declare i64 @tyra_json_get(i64, ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_json_at(i64, i64)").unwrap();
    // M11 phase 1: http client. See runtime/src/stdlib_http.rs.
    writeln!(out, "declare i64 @tyra_http_get(ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_http_status(i64)").unwrap();
    writeln!(out, "declare ptr @tyra_http_body(i64)").unwrap();
    writeln!(out, "declare i32 @tyra_http_errno()").unwrap();
    writeln!(out, "declare ptr @tyra_http_errmsg()").unwrap();
    // M11 phase 2: http server. The Server handle is declared as `ptr`
    // so LLVM's IR-level type semantics match the Rust-side `*const
    // Server`. Tyra's `AppServer._handle` is typed `Int` (i64), so the
    // builtin emit helpers ptrtoint/inttoptr across the boundary.
    writeln!(out, "declare ptr @tyra_http_server_new()").unwrap();
    writeln!(
        out,
        "declare void @tyra_http_server_route(ptr, ptr, ptr, ptr)"
    )
    .unwrap();
    writeln!(out, "declare i32 @tyra_http_server_listen(ptr, i64)").unwrap();
    // stdin stdlib. See runtime/src/stdlib_io.rs.
    writeln!(out, "declare ptr @tyra_io_read_line()").unwrap();
    writeln!(out, "declare ptr @tyra_io_read_to_end()").unwrap();
    writeln!(out, "declare i32 @tyra_io_eof()").unwrap();
    // §17.3.4: string stdlib. See runtime/src/stdlib_string.rs.
    writeln!(out, "declare i64 @tyra_string_len(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_string_is_empty(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_trim(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_to_upper(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_to_lower(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_string_contains(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_string_starts_with(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_string_ends_with(ptr, ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_string_parse_int(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_string_parse_errno()").unwrap();
    writeln!(out, "declare i64 @tyra_string_byte_at(ptr, i64)").unwrap();
    writeln!(out, "declare ptr @tyra_string_substring(ptr, i64, i64)").unwrap();
    writeln!(out, "declare ptr @tyra_string_reverse(ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_from_byte(i64)").unwrap();
    writeln!(out, "declare void @tyra_string_split_whitespace(ptr, ptr)").unwrap();
    writeln!(out, "declare void @tyra_string_split(ptr, ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_replace(ptr, ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_string_join(ptr, ptr)").unwrap();
    // §17.3.x: time stdlib. See runtime/src/stdlib_time.rs.
    writeln!(out, "declare i64 @tyra_time_now_unix()").unwrap();
    writeln!(out, "declare i64 @tyra_time_monotonic_millis()").unwrap();
    // §17.3.x: log stdlib. See runtime/src/stdlib_log.rs.
    writeln!(out, "declare void @tyra_log_info(ptr)").unwrap();
    writeln!(out, "declare void @tyra_log_warn(ptr)").unwrap();
    writeln!(out, "declare void @tyra_log_error(ptr)").unwrap();
    // §17.3.x: float stdlib. See runtime/src/stdlib_float.rs.
    writeln!(out, "declare i32 @tyra_float_eq(double, double)").unwrap();
    writeln!(
        out,
        "declare i32 @tyra_float_approx_eq(double, double, double)"
    )
    .unwrap();
    writeln!(out, "declare double @tyra_float_abs(double)").unwrap();
    writeln!(out, "declare double @tyra_float_floor(double)").unwrap();
    writeln!(out, "declare double @tyra_float_ceil(double)").unwrap();
    writeln!(out, "declare double @tyra_float_round(double)").unwrap();
    writeln!(out, "declare double @tyra_float_min(double, double)").unwrap();
    writeln!(out, "declare double @tyra_float_max(double, double)").unwrap();
    writeln!(out, "declare ptr @tyra_float_to_string(double)").unwrap();
    writeln!(out, "declare double @tyra_float_parse(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_float_parse_errno()").unwrap();
    writeln!(out, "declare double @tyra_float_from_int(i64)").unwrap();
    writeln!(out, "declare i64 @tyra_float_to_int(double)").unwrap();
    writeln!(out, "declare i32 @tyra_float_is_nan(double)").unwrap();
    writeln!(out, "declare i32 @tyra_float_is_infinite(double)").unwrap();
    // §17.3.6 Map<K,V> generic runtime (ADR-0015).
    writeln!(out, "declare ptr @tyra_map_new(ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_map_insert(ptr, ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_map_get(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_map_contains(ptr, ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_map_len(ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_hash_cstr(ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_cstr_eq(ptr, ptr)").unwrap();
    // §17.3.x Set<T> generic runtime (ADR-0015).
    writeln!(out, "declare ptr @tyra_set_new(ptr, ptr)").unwrap();
    writeln!(out, "declare ptr @tyra_set_insert(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_set_contains(ptr, ptr)").unwrap();
    writeln!(out, "declare i64 @tyra_set_len(ptr)").unwrap();
    // Zero-slot for null-safe map-get unboxing (read-only; never written).
    writeln!(
        out,
        "@.tyra_zero_slot = private unnamed_addr constant i64 0"
    )
    .unwrap();
    writeln!(out, "declare void @abort()").unwrap();
    writeln!(out, "declare void @exit(i32)").unwrap();
    writeln!(out, "declare i32 @strcmp(ptr, ptr)").unwrap();
    // §18.8: bench clock intrinsic (v0.4.0). See runtime/src/lib.rs.
    writeln!(out, "declare i64 @__bench_clock_ns()").unwrap();
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

    // Pre-scan all Spawn sites across the program so arg-struct types can be
    // declared before any function references them (LLVM requires struct
    // types to be sized at use). Ids are assigned in program order and the
    // same iteration order is used during function emission, so references
    // and definitions line up.
    let spawn_thunks_vec = collect_spawn_thunks(program);
    for thunk in &spawn_thunks_vec {
        if !thunk.arg_types.is_empty() {
            let fields: Vec<String> = thunk
                .arg_types
                .iter()
                .map(|ty| llvm_type_str(ty, &struct_map))
                .collect();
            writeln!(
                out,
                "%struct.__tyra_spawn_args_{} = type {{ {} }}",
                thunk.id,
                fields.join(", ")
            )
            .unwrap();
        }
    }
    if !spawn_thunks_vec.is_empty() {
        writeln!(out).unwrap();
    }
    let spawn_thunks: std::cell::RefCell<Vec<SpawnThunk>> = std::cell::RefCell::new(Vec::new());

    // Functions
    for func in &program.functions {
        emit_function(
            &mut out,
            func,
            &program.string_constants,
            &program.source_files,
            &struct_map,
            &fn_sigs,
            &spawn_thunks,
            cov_map,
            dwarf_ctx.as_mut(),
        );
        writeln!(out).unwrap();
    }

    // Emit thunks — the collection appended during function emission must
    // match the pre-scan one-for-one. A mismatch is always a compiler bug,
    // not user input, so this is an unconditional assert: a release build
    // with a broken pre-scan would otherwise reference
    // `@__tyra_spawn_thunk_N` without ever defining it, yielding a linker
    // error at best and silent miscompilation at worst.
    assert_eq!(
        spawn_thunks.borrow().len(),
        spawn_thunks_vec.len(),
        "pre-scan and emission disagree on spawn thunk count"
    );
    for thunk in spawn_thunks.borrow().iter() {
        emit_spawn_thunk(&mut out, thunk, &struct_map);
        writeln!(&mut out).unwrap();
    }

    // Emit compiler-generated eq/hash functions for every K/T type used in
    // Map<K,_> or Set<T> intrinsic calls in this program.
    let elem_types = collect_elem_types(program);
    for k in &elem_types {
        emit_map_eq_hash(&mut out, k);
    }

    // DWARF metadata section (ADR-0014 §4a) — must come after all function defs.
    if let Some(dwarf) = dwarf_ctx {
        writeln!(out).unwrap();
        out.push_str(&dwarf.emit_metadata());
    }

    out
}

/// Collect all element type names that need compiler-emitted eq/hash functions.
/// Covers Map<K,_> key types and Set<T> element types.
fn collect_elem_types(program: &Program) -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    for func in &program.functions {
        for stmt in &func.body {
            collect_elem_types_stmt(stmt, &mut keys);
        }
    }
    keys
}

fn collect_elem_types_stmt(stmt: &MirStmt, keys: &mut std::collections::HashSet<String>) {
    let instr = &stmt.instr;
    if let Instruction::Call { func, .. } = instr {
        // Map: __map_new__K__V  or  __map_insert__K__V  or  __map_contains__K
        if let Some(rest) = func.strip_prefix("__map_new__") {
            // rest = "K__V"
            if let Some(k) = rest.split("__").next() {
                keys.insert(k.to_string());
            }
        } else if let Some(rest) = func.strip_prefix("__map_contains__") {
            keys.insert(rest.to_string());
        }
        // Set: __set_new__T  or  __set_insert__T  or  __set_contains__T
        if let Some(t) = func.strip_prefix("__set_new__") {
            keys.insert(t.to_string());
        } else if let Some(t) = func.strip_prefix("__set_contains__") {
            keys.insert(t.to_string());
        }
    }
    if let Instruction::MapGetOption { key_ty, .. } = instr {
        keys.insert(key_ty.monomorphized_name());
    }
}

/// Emit `@tyra_eq_<K>` and `@tyra_hash_<K>` as private LLVM functions.
fn emit_map_eq_hash(out: &mut String, k: &str) {
    match k {
        "Int" => {
            // EqFn ABI: returns i32 (matches runtime EqFn = fn(...) -> i32).
            writeln!(out, "define private i32 @tyra_eq_Int(ptr %a, ptr %b) {{").unwrap();
            writeln!(out, "  %va = load i64, ptr %a").unwrap();
            writeln!(out, "  %vb = load i64, ptr %b").unwrap();
            writeln!(out, "  %r1 = icmp eq i64 %va, %vb").unwrap();
            writeln!(out, "  %r = zext i1 %r1 to i32").unwrap();
            writeln!(out, "  ret i32 %r").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "define private i64 @tyra_hash_Int(ptr %a) {{").unwrap();
            writeln!(out, "  %v = load i64, ptr %a").unwrap();
            // Knuth multiplicative hash (odd 64-bit constant).
            writeln!(out, "  %h = mul i64 %v, -3932073806218323177").unwrap();
            writeln!(out, "  ret i64 %h").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
        }
        "Bool" => {
            writeln!(out, "define private i32 @tyra_eq_Bool(ptr %a, ptr %b) {{").unwrap();
            writeln!(out, "  %va = load i8, ptr %a").unwrap();
            writeln!(out, "  %vb = load i8, ptr %b").unwrap();
            writeln!(out, "  %r1 = icmp eq i8 %va, %vb").unwrap();
            writeln!(out, "  %r = zext i1 %r1 to i32").unwrap();
            writeln!(out, "  ret i32 %r").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "define private i64 @tyra_hash_Bool(ptr %a) {{").unwrap();
            writeln!(out, "  %v = load i8, ptr %a").unwrap();
            writeln!(out, "  %h = zext i8 %v to i64").unwrap();
            writeln!(out, "  ret i64 %h").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
        }
        "String" => {
            writeln!(out, "define private i32 @tyra_eq_String(ptr %a, ptr %b) {{").unwrap();
            writeln!(out, "  %sa = load ptr, ptr %a").unwrap();
            writeln!(out, "  %sb = load ptr, ptr %b").unwrap();
            // tyra_cstr_eq already returns i32; use it directly.
            writeln!(out, "  %r = call i32 @tyra_cstr_eq(ptr %sa, ptr %sb)").unwrap();
            writeln!(out, "  ret i32 %r").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "define private i64 @tyra_hash_String(ptr %a) {{").unwrap();
            writeln!(out, "  %sp = load ptr, ptr %a").unwrap();
            writeln!(out, "  %h = call i64 @tyra_hash_cstr(ptr %sp)").unwrap();
            writeln!(out, "  ret i64 %h").unwrap();
            writeln!(out, "}}").unwrap();
            writeln!(out).unwrap();
        }
        _ => {
            // User-defined key type: eq/hash generation for value structs/ADTs
            // is not yet implemented. The type checker rejects non-primitive K
            // before reaching codegen, so this arm is unreachable for valid programs.
        }
    }
}

/// Walk every instruction in program order and return a SpawnThunk descriptor
/// per `Instruction::Spawn`. Ids are sequential starting at 0 and match the
/// order in which instruction emission later pushes into `spawn_thunks`.
fn collect_spawn_thunks(program: &Program) -> Vec<SpawnThunk> {
    let mut thunks = Vec::new();
    for f in &program.functions {
        for stmt in &f.body {
            let inst = &stmt.instr;
            if let Instruction::Spawn {
                func,
                arg_types,
                result_type,
                ..
            } = inst
            {
                thunks.push(SpawnThunk {
                    id: thunks.len(),
                    func: func.clone(),
                    arg_types: arg_types.clone(),
                    result_type: result_type.clone(),
                });
            }
        }
    }
    thunks
}

/// Emit a synthesized `__tyra_spawn_thunk_N(ptr %args) -> ptr` that matches
/// the C ABI expected by `tyra_task_spawn`. The thunk unpacks the GC-managed
/// argument struct, calls the target function, boxes the result via
/// `GC_malloc`, and returns the box pointer. For Unit returns it returns
/// null.
fn emit_spawn_thunk(
    out: &mut String,
    thunk: &SpawnThunk,
    struct_map: &std::collections::HashMap<String, StructInfo>,
) {
    let id = thunk.id;
    writeln!(
        out,
        "define internal ptr @__tyra_spawn_thunk_{id}(ptr %args) {{"
    )
    .unwrap();
    writeln!(out, "entry:").unwrap();

    // Load each arg from the per-site arg struct.
    let args_ty = format!("%struct.__tyra_spawn_args_{id}");
    let mut call_args: Vec<String> = Vec::with_capacity(thunk.arg_types.len());
    for (i, ty) in thunk.arg_types.iter().enumerate() {
        let llvm_ty = llvm_type_str(ty, struct_map);
        writeln!(
            out,
            "  %a{i}.ptr = getelementptr {args_ty}, ptr %args, i32 0, i32 {i}"
        )
        .unwrap();
        writeln!(out, "  %a{i} = load {llvm_ty}, ptr %a{i}.ptr").unwrap();
        call_args.push(format!("{llvm_ty} %a{i}"));
    }

    let ret_ty = llvm_type_str(&thunk.result_type, struct_map);
    let call_args_joined = call_args.join(", ");

    // NOTE: large-struct returns (Sys V AMD64 `sret` attribute) are not
    // emitted by Tyra's current codegen, so this direct `%result = call`
    // pattern is ABI-safe today. If Tyra ever starts emitting `sret` on
    // user functions, this thunk call site needs to match.
    if matches!(thunk.result_type, Ty::Unit) {
        // Unit-returning fn: call, discard result, return null.
        writeln!(out, "  call void @{}({})", thunk.func, call_args_joined).unwrap();
        writeln!(out, "  ret ptr null").unwrap();
    } else {
        writeln!(
            out,
            "  %result = call {ret_ty} @{}({})",
            thunk.func, call_args_joined
        )
        .unwrap();
        // Box the result: GC_malloc(sizeof(result_type)) then store.
        writeln!(
            out,
            "  %box.sz_ptr = getelementptr {ret_ty}, ptr null, i32 1"
        )
        .unwrap();
        writeln!(out, "  %box.sz = ptrtoint ptr %box.sz_ptr to i64").unwrap();
        writeln!(out, "  %box = call ptr @GC_malloc(i64 %box.sz)").unwrap();
        writeln!(out, "  store {ret_ty} %result, ptr %box").unwrap();
        writeln!(out, "  ret ptr %box").unwrap();
    }

    writeln!(out, "}}").unwrap();
}

/// Function signature for cross-function type resolution.
pub(crate) struct FnSig {
    pub(crate) param_types: Vec<Ty>,
    pub(crate) return_type: Ty,
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    out: &mut String,
    func: &Function,
    strings: &[String],
    source_files: &[String],
    struct_map: &std::collections::HashMap<String, StructInfo>,
    fn_sigs: &std::collections::HashMap<String, FnSig>,
    spawn_thunks: &std::cell::RefCell<Vec<SpawnThunk>>,
    cov_map: Option<&CovMap>,
    mut dwarf_ctx: Option<&mut DwarfCtx>,
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

    // Pre-compute the DISubprogram id for this function (used for !dbg annotations).
    let sp_id = dwarf_ctx.as_ref().and_then(|d| d.subprogram_id(&func.name));

    if func.is_main {
        if let Some(sp) = sp_id {
            writeln!(out, "define i32 @main(i32 %argc, ptr %argv) !dbg !{sp} {{").unwrap();
        } else {
            writeln!(out, "define i32 @main(i32 %argc, ptr %argv) {{").unwrap();
        }
    } else if let Some(sp) = sp_id {
        writeln!(
            out,
            "define {ret_ty} @{}({}) !dbg !{sp} {{",
            func.name,
            params.join(", ")
        )
        .unwrap();
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
        // Coverage: register counter array with the runtime atexit flusher.
        if let Some(cm) = cov_map {
            emit_cov_init_call(out, cm.n);
        }
    }

    // Allocate parameter copies for mutation support
    for (name, ty) in &func.params {
        let lt = llvm_type_str(ty, struct_map);
        writeln!(out, "  %{name}.addr = alloca {lt}").unwrap();
        writeln!(out, "  store {lt} %{name}, ptr %{name}.addr").unwrap();
    }

    // Hoist all alloca slots to the entry block. An alloca in a non-entry
    // block consumes stack on every execution of that block and is never
    // freed until the function returns, so allocas inside loop bodies cause
    // unbounded O(iterations) stack growth (docs/notes/099-sum-column-diagnosis.md).
    // Unreachable (dead-code) allocas are included conservatively — a dead slot
    // allocated once in entry is harmless and prevents load-of-undefined-alloca.
    // MIR guarantees dest names are unique within a function, so name-dedup is
    // safe; the HashSet is a defensive guard against any future duplication.
    let mut hoisted = std::collections::HashSet::new();
    for stmt in &func.body {
        let inst = &stmt.instr;
        if let Instruction::Alloca { dest } = inst {
            if hoisted.insert(dest.as_str()) {
                let llvm_ty = scan
                    .alloca_llvm_types
                    .get(dest.as_str())
                    .map(String::as_str)
                    .unwrap_or("i64");
                writeln!(out, "  %{dest} = alloca {llvm_ty}").unwrap();
            }
        }
    }

    let ctx = EmitCtx {
        struct_map,
        fn_sigs,
        string_temps: &scan.string_temps,
        float_temps: &scan.float_temps,
        bool_temps: &scan.bool_temps,
        struct_temps: &scan.struct_temps,
        alloca_llvm_types: &scan.alloca_llvm_types,
        spawn_thunks,
        source_files,
    };

    // Coverage: counter id used for unique SSA temp names within this function.
    let mut cov_id: u32 = 0;

    // Coverage: increment the entry-block counter (the "entry:" label is
    // already emitted above; this increment goes right after alloca hoisting).
    if let Some(cm) = cov_map {
        if let Some(&entry_idx) = cm.fn_entry_ctr.get(&func.name) {
            // Use the counter index directly (counter_for map lookup not needed here).
            let _ = entry_idx; // suppress unused warning
            // Find the first non-dummy loc to look up in counter_for.
            if let Some(first_stmt) = func.body.iter().find(|s| !s.loc.is_dummy()) {
                emit_cov_increment(out, first_stmt.loc, cm, &mut cov_id);
            }
        }
    }

    // DWARF locals (ADR-0014 §4a-ii): emit llvm.dbg.declare after alloca hoisting.
    // This binds each alloca slot to its DILocalVariable so the debugger can
    // display variable values by name.
    if let Some(dwarf) = dwarf_ctx.as_deref_mut() {
        if let Some(sp) = sp_id {
            let first_line = func
                .body
                .iter()
                .find(|s| !s.loc.is_dummy())
                .map(|s| s.loc.line)
                .unwrap_or(1);
            let first_file_id = func
                .body
                .iter()
                .find(|s| !s.loc.is_dummy())
                .map(|s| s.loc.file_id)
                .unwrap_or(0);
            let decl_loc = dwarf.get_or_create_loc(sp, first_line);
            for meta in &func.local_metas {
                if meta.alloca_name.is_empty() {
                    continue;
                }
                let type_id = dwarf.type_node(&meta.ty);
                let var_id =
                    dwarf.emit_local_var(&meta.name, sp, first_file_id, first_line, type_id);
                writeln!(
                    out,
                    "  call void @llvm.dbg.declare(metadata ptr %{}, metadata !{var_id}, metadata !DIExpression()), !dbg !{decl_loc}",
                    meta.alloca_name
                )
                .unwrap();
            }
        }
    }

    // Emit instructions, skipping dead code after block terminators.
    let mut block_terminated = false;
    for (stmt_idx, stmt) in func.body.iter().enumerate() {
        let inst = &stmt.instr;
        match inst {
            Instruction::Label(_) => {
                block_terminated = false;
                // Emit the label itself.
                emit_instruction(out, inst, stmt.loc, func, strings, &ctx);
                // Coverage: increment the BB-entry counter immediately after the label.
                if let Some(cm) = cov_map {
                    let loc = if !stmt.loc.is_dummy() {
                        stmt.loc
                    } else {
                        func.body[stmt_idx + 1..]
                            .iter()
                            .find(|s| !s.loc.is_dummy())
                            .map(|s| s.loc)
                            .unwrap_or_else(tyra_mir::SourceLoc::dummy)
                    };
                    emit_cov_increment(out, loc, cm, &mut cov_id);
                }
                continue;
            }
            // Alloca slots were already emitted in the entry block above.
            Instruction::Alloca { .. } => continue,
            _ if block_terminated => continue,
            _ => {}
        }
        let prev_len = out.len();
        emit_instruction(out, inst, stmt.loc, func, strings, &ctx);
        // Attach !dbg to the last LLVM instruction emitted for this MIR stmt.
        if !stmt.loc.is_dummy() {
            if let Some(sp) = sp_id {
                if let Some(dwarf) = dwarf_ctx.as_deref_mut() {
                    let dbg_id = dwarf.get_or_create_loc(sp, stmt.loc.line);
                    patch_dbg_on_last_instruction(out, prev_len, dbg_id);
                }
            }
        }
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
    /// Per-site spawn thunks collected during instruction emission and
    /// emitted after all user functions (M9).
    pub(crate) spawn_thunks: &'a std::cell::RefCell<Vec<SpawnThunk>>,
    /// Source file paths indexed by SourceLoc::file_id, for panic diagnostics (ADR 0014).
    pub(crate) source_files: &'a [String],
}

/// Metadata for a synthetic spawn thunk. Codegen emits one per `spawn` site
/// with a unique id. The thunk unboxes args, calls the target, boxes the
/// result, and matches the `ThunkFn` C ABI expected by `tyra_task_spawn`.
#[derive(Debug, Clone)]
pub(crate) struct SpawnThunk {
    pub(crate) id: usize,
    pub(crate) func: String,
    pub(crate) arg_types: Vec<Ty>,
    pub(crate) result_type: Ty,
}

// Extracted modules:
// - helpers.rs: operand_ref, llvm_type_str, llvm_escape_string, etc.
// - type_scan.rs: pre_scan_struct_types, pre_scan_alloca_llvm_types
// - instr_emit.rs: emit_instruction, emit_call_args_typed
