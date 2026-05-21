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
                recursive_fields: sd.recursive_fields.clone(),
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
    writeln!(out, "declare ptr @tyra_map_new_string_int()").unwrap();
    writeln!(
        out,
        "declare ptr @tyra_map_insert_string_int(ptr, ptr, i64)"
    )
    .unwrap();
    writeln!(out, "declare i64 @tyra_map_get_string_int(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_map_contains_string_int(ptr, ptr)").unwrap();
    writeln!(out, "declare i32 @tyra_map_get_present()").unwrap();
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
            &struct_map,
            &fn_sigs,
            &spawn_thunks,
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

    out
}

/// Walk every instruction in program order and return a SpawnThunk descriptor
/// per `Instruction::Spawn`. Ids are sequential starting at 0 and match the
/// order in which instruction emission later pushes into `spawn_thunks`.
fn collect_spawn_thunks(program: &Program) -> Vec<SpawnThunk> {
    let mut thunks = Vec::new();
    for f in &program.functions {
        for inst in &f.body {
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

fn emit_function(
    out: &mut String,
    func: &Function,
    strings: &[String],
    struct_map: &std::collections::HashMap<String, StructInfo>,
    fn_sigs: &std::collections::HashMap<String, FnSig>,
    spawn_thunks: &std::cell::RefCell<Vec<SpawnThunk>>,
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

    // Hoist all alloca slots to the entry block. An alloca in a non-entry
    // block consumes stack on every execution of that block and is never
    // freed until the function returns, so allocas inside loop bodies cause
    // unbounded O(iterations) stack growth (docs/notes/099-sum-column-diagnosis.md).
    // Unreachable (dead-code) allocas are included conservatively — a dead slot
    // allocated once in entry is harmless and prevents load-of-undefined-alloca.
    // MIR guarantees dest names are unique within a function, so name-dedup is
    // safe; the HashSet is a defensive guard against any future duplication.
    let mut hoisted = std::collections::HashSet::new();
    for inst in &func.body {
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
    };

    // Emit instructions, skipping dead code after block terminators.
    let mut block_terminated = false;
    for inst in &func.body {
        match inst {
            Instruction::Label(_) => {
                block_terminated = false;
            }
            // Alloca slots were already emitted in the entry block above.
            Instruction::Alloca { .. } => continue,
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
    /// Per-site spawn thunks collected during instruction emission and
    /// emitted after all user functions (M9).
    pub(crate) spawn_thunks: &'a std::cell::RefCell<Vec<SpawnThunk>>,
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
