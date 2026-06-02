//! Inkwell-based LLVM IR generation (v0.9.0, ADR-0018 Theme A).
//!
//! **I0 scaffold.** Establishes the `CodeGen<'ctx>` value-handle model that
//! replaces the string-based IR builder in `codegen.rs`. SSA values become
//! typed `BasicValueEnum` handles keyed by MIR temp name, so a value's LLVM
//! type travels with it to every use site — structurally eliminating the
//! "default an unknown operand to i64" mistyping class the text backend was
//! prone to (see `helpers::infer_operand_type`).
//!
//! The legacy text path in `codegen.rs` remains the production backend until
//! the migration completes (I7). This module is compiled and exercised in
//! parallel so each phase (I1 declarations, I2 instructions, …) lands
//! incrementally behind its own verification gate. I0 itself changes no
//! observable output.

use std::collections::{HashMap, HashSet};

use inkwell::AddressSpace;
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::targets::TargetTriple;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, PointerType, StructType};
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};

use tyra_mir::{Program, SourceLoc};
use tyra_types::Ty;

use crate::helpers::target_triple;

/// Inkwell codegen state for one module.
///
/// Replaces the string-based value/type tracking of the legacy text backend:
/// every SSA value is a typed `BasicValueEnum` handle keyed by its MIR temp
/// name, and named types resolve through `struct_types`/`data_types` rather
/// than string lookups.
// Several fields/methods are unread in the I0 scaffold; they are populated and
// consumed from I1 (declarations) and I2 (instructions, blocks, allocas) on.
#[allow(dead_code)]
pub(crate) struct CodeGen<'ctx> {
    pub(crate) ctx: &'ctx Context,
    pub(crate) module: Module<'ctx>,
    pub(crate) builder: Builder<'ctx>,
    /// MIR temp name -> SSA value handle.
    pub(crate) values: HashMap<String, BasicValueEnum<'ctx>>,
    /// Named (and monomorphized) struct/ADT types.
    pub(crate) struct_types: HashMap<String, StructType<'ctx>>,
    /// `data` types (§8.6): heap-allocated, represented as `ptr`.
    pub(crate) data_types: HashSet<String>,
    /// Declared functions by name.
    pub(crate) fn_values: HashMap<String, FunctionValue<'ctx>>,
    /// **Entry** block of each MIR label — the branch *target* for
    /// `Jump`/`BranchIf`. Built once (pre-creation) and never overwritten, so a
    /// jump to a label always lands at the region's start. Reset per function (I2).
    pub(crate) blocks: HashMap<String, BasicBlock<'ctx>>,
    /// **Exit** block of each MIR label — the block its terminator actually
    /// branches *from*, used only for deferred phi predecessor resolution. For
    /// an unsplit region this equals `blocks[label]`; when an instruction splits
    /// the block mid-emission (I3 ListGet/ListGetSafe bounds checks) it advances
    /// to the final block. Kept separate from `blocks` so the jump-target table
    /// stays intact. Reset per function.
    pub(crate) pred_blocks: HashMap<String, BasicBlock<'ctx>>,
    /// alloca slots (param/local addresses) by name. Reset per function (I2).
    pub(crate) addr_slots: HashMap<String, PointerValue<'ctx>>,
    /// Load type for each alloca slot (slots are `alloca i64` for size, but
    /// loads use the stored value's type). Reset per function (I2b).
    pub(crate) slot_types: HashMap<String, BasicTypeEnum<'ctx>>,
    /// Per-struct "is field a recursive self-reference" flags (ADT boxing, I2d).
    pub(crate) recursive_fields: HashMap<String, Vec<bool>>,
    /// MIR label of the block currently being emitted, if any (None in the
    /// entry region). Set on `Label`; after each instruction `pred_blocks` for
    /// this label is re-synced to the builder's current block. Reset per function.
    pub(crate) cur_label: Option<String>,
    /// I4c type-scan bridge (ADR-0018 Theme A, Option A). The legacy text
    /// backend's `StructInfo`/`FnSig` maps + per-function `type_scan` results
    /// give the inkwell backend an operand's *Tyra* type, which the opaque-`ptr`
    /// value handle cannot recover (String vs data/fn/handle ptr). Needed by
    /// `print` routing (String→%s vs other). Transitional coupling to the legacy
    /// structures; removed when the legacy backend is deleted (I7).
    pub(crate) struct_map: HashMap<String, crate::codegen::StructInfo>,
    pub(crate) fn_sigs: HashMap<String, crate::codegen::FnSig>,
    /// Type scan for the function currently being emitted (set per function in
    /// `emit_bodies`, consumed by the emittability gate and `print`).
    pub(crate) scan: Option<crate::type_scan::ScanResult>,
    /// Source location of the instruction currently being emitted (set per stmt
    /// in `emit_bodies`). Consumed by `panic` for the "panic at FILE:LINE"
    /// diagnostic; the forthcoming I6 DWARF wiring will read it too. Reset to
    /// `dummy()` per function (set before each instruction).
    pub(crate) cur_loc: SourceLoc,
}

impl<'ctx> CodeGen<'ctx> {
    pub(crate) fn new(ctx: &'ctx Context, module_name: &str) -> Self {
        let module = ctx.create_module(module_name);
        module.set_triple(&TargetTriple::create(target_triple()));
        CodeGen {
            ctx,
            module,
            builder: ctx.create_builder(),
            values: HashMap::new(),
            struct_types: HashMap::new(),
            data_types: HashSet::new(),
            fn_values: HashMap::new(),
            blocks: HashMap::new(),
            pred_blocks: HashMap::new(),
            addr_slots: HashMap::new(),
            slot_types: HashMap::new(),
            recursive_fields: HashMap::new(),
            cur_label: None,
            struct_map: HashMap::new(),
            fn_sigs: HashMap::new(),
            scan: None,
            cur_loc: SourceLoc::dummy(),
        }
    }

    /// Opaque `ptr` type (LLVM 15+ opaque pointers).
    #[allow(dead_code)] // used from I1 on (declarations, value emission)
    pub(crate) fn ptr(&self) -> PointerType<'ctx> {
        self.ctx.ptr_type(AddressSpace::default())
    }

    /// Bridge a Tyra `Ty` to an inkwell **value** type. Mirrors
    /// `helpers::llvm_type_str` but yields a typed handle.
    ///
    /// `Unit`/`Never` map to `i64` here because the legacy backend stores Unit
    /// as `i64` in value position (e.g. ADT struct fields, codegen.rs). The
    /// `void` *return* type is not a `BasicTypeEnum` and is handled separately
    /// at function-signature emission (I1).
    #[allow(dead_code)] // used from I1 on (declarations, value emission)
    pub(crate) fn ty_to_basic_type(&self, ty: &Ty) -> BasicTypeEnum<'ctx> {
        match ty {
            Ty::Int => self.ctx.i64_type().into(),
            Ty::Float => self.ctx.f64_type().into(),
            Ty::Bool => self.ctx.bool_type().into(),
            Ty::String => self.ptr().into(),
            Ty::Fn(..) => self.ptr().into(),
            Ty::Named(name) => {
                if self.data_types.contains(name) {
                    self.ptr().into()
                } else if let Some(st) = self.struct_types.get(name) {
                    (*st).into()
                } else {
                    self.ctx.i64_type().into()
                }
            }
            Ty::Generic(..) => {
                // Check `data_types` first: a monomorphized generic *data* type
                // (e.g. `data Box<Int>` → `Box__Int`) is heap-allocated and
                // represented as `ptr` (§8.6), like the `Named` branch above.
                // The monomorphized name is what `register_struct_types` inserts
                // into both maps, so the lookup key matches.
                let mono = ty.monomorphized_name();
                if self.data_types.contains(&mono) {
                    self.ptr().into()
                } else if let Some(st) = self.struct_types.get(&mono) {
                    (*st).into()
                } else {
                    self.ctx.i64_type().into()
                }
            }
            // Unit / Never / unresolved → i64 in value position.
            _ => self.ctx.i64_type().into(),
        }
    }

    /// I4c: build the legacy-shaped `StructInfo`/`FnSig` maps that
    /// `type_scan::scan_function_types` consumes, so the inkwell backend can
    /// recover an operand's Tyra type (see the `scan` field). Mirrors the inline
    /// builders in `codegen.rs` (the legacy text path).
    fn build_type_scan_maps(&mut self, program: &Program) {
        use crate::codegen::{FnSig, StructInfo};
        self.struct_map = program
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
        self.fn_sigs = program
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
    }

    /// Register an (opaque) named struct type for every struct/ADT definition
    /// so the type bridge resolves `Named`/`Generic` types. Bodies are filled
    /// in I1 (declaration phase) once field layout is computed.
    fn register_struct_types(&mut self, program: &Program) {
        for sd in &program.struct_defs {
            if sd.is_data {
                self.data_types.insert(sd.name.clone());
            }
            let st = self.ctx.opaque_struct_type(&format!("struct.{}", sd.name));
            self.struct_types.insert(sd.name.clone(), st);
            self.recursive_fields
                .insert(sd.name.clone(), sd.recursive_fields.clone());
        }
    }

    // ---- I1: declaration phase ----

    /// Closure fat pointer `{ ptr, ptr }` (ADR-0011: fn_ptr + env_ptr). Always
    /// declared so indirect-call emission can reference it unconditionally.
    fn declare_closure_type(&mut self) {
        let st = self.ctx.opaque_struct_type("struct.__closure_fat");
        st.set_body(&[self.ptr().into(), self.ptr().into()], false);
        self.struct_types.insert("__closure_fat".into(), st);
    }

    /// Fill bodies on the opaque struct types registered by
    /// `register_struct_types`. All opaque types exist first so mutually
    /// referencing layouts resolve (recursive self-references are boxed as
    /// `ptr` via `recursive_fields`, so no infinite value nesting occurs).
    fn set_struct_bodies(&self, program: &Program) {
        for sd in &program.struct_defs {
            // ADT tag field (field 0) is i8 regardless of its MIR type.
            let is_adt = sd.fields.first().map(|(n, _)| n == "tag").unwrap_or(false);
            let fields: Vec<BasicTypeEnum<'ctx>> = sd
                .fields
                .iter()
                .enumerate()
                .map(|(i, (_, ty))| {
                    if is_adt && i == 0 {
                        self.ctx.i8_type().into()
                    } else if sd.recursive_fields.get(i).copied().unwrap_or(false) {
                        self.ptr().into()
                    } else {
                        // `ty_to_basic_type` already maps Unit -> i64 (Unit is
                        // not a valid struct field type).
                        self.ty_to_basic_type(ty)
                    }
                })
                .collect();
            if let Some(st) = self.struct_types.get(&sd.name) {
                st.set_body(&fields, false);
            }
        }
    }

    /// Add a private, unnamed_addr, null-terminated C string constant.
    fn add_cstring(&self, name: &str, s: &str) {
        let init = self.ctx.const_string(s.as_bytes(), true);
        let g = self.module.add_global(init.get_type(), None, name);
        g.set_initializer(&init);
        g.set_constant(true);
        g.set_linkage(Linkage::Private);
        g.set_unnamed_addr(true);
    }

    /// Module-level globals: argc/argv capture slots, string/source/format
    /// constants, the panic sentinel, and the null-safe zero slot.
    fn declare_globals(&self, program: &Program) {
        let i32t = self.ctx.i32_type();
        let argc = self.module.add_global(i32t, None, ".tyra.argc");
        argc.set_initializer(&i32t.const_zero());
        argc.set_linkage(Linkage::Internal);
        let argv = self.module.add_global(self.ptr(), None, ".tyra.argv");
        argv.set_initializer(&self.ptr().const_null());
        argv.set_linkage(Linkage::Internal);

        for (idx, s) in program.string_constants.iter().enumerate() {
            self.add_cstring(&format!(".str.{idx}"), s);
        }
        for (idx, path) in program.source_files.iter().enumerate() {
            self.add_cstring(&format!(".src.{idx}"), path);
        }

        // printf/snprintf format strings (literal bytes incl. newlines).
        self.add_cstring(".fmt.str", "%s");
        self.add_cstring(".fmt.int", "%ld");
        self.add_cstring(".fmt.int_ln", "%ld\n");
        self.add_cstring(".fmt.float", "%g");
        self.add_cstring(".fmt.float_ln", "%g\n");
        self.add_cstring(".fmt.panic_loc", "panic at %s:%ld:\n");
        self.add_cstring(".fmt.str_ln", "%s\n");
        // Panic sentinel (ADR-0012): distinguishes panic() from sys.exit(101).
        self.add_cstring(".str.panic_sentinel", "__TYRA_PANIC__\n");

        // Zero slot for null-safe map-get unboxing (read-only).
        let i64t = self.ctx.i64_type();
        let zero = self.module.add_global(i64t, None, ".tyra_zero_slot");
        zero.set_initializer(&i64t.const_zero());
        zero.set_constant(true);
        zero.set_linkage(Linkage::Private);
        zero.set_unnamed_addr(true);
    }

    /// Declare the libc / Boehm GC / Tyra runtime externs. Data-driven from a
    /// single table to keep the ABI signatures centralized and reduce
    /// transcription error vs ~70 hand-written `declare` lines.
    fn declare_externs(&self) {
        use ExternKind::*;
        // (name, return, params, varargs)
        let externs: &[(&str, ExternKind, &[ExternKind], bool)] = &[
            ("puts", I32, &[P], false),
            ("printf", I32, &[P], true),
            ("snprintf", I32, &[P, I64, P], true),
            ("dprintf", I32, &[I32, P], true),
            ("GC_malloc", P, &[I64], false),
            ("GC_init", V, &[], false),
            ("tyra_rt_init", V, &[], false),
            ("tyra_task_spawn", P, &[P, P], false),
            ("tyra_task_await", P, &[P], false),
            ("tyra_task_select", P, &[P, I64], false),
            ("tyra_fs_read", P, &[P], false),
            ("tyra_fs_errno", I32, &[], false),
            ("tyra_fs_errmsg", P, &[], false),
            ("tyra_fs_write", V, &[P, P], false),
            ("tyra_fs_exists", I32, &[P], false),
            ("tyra_json_parse", I64, &[P], false),
            ("tyra_json_err_msg", P, &[], false),
            ("tyra_json_err_line", I64, &[], false),
            ("tyra_json_err_col", I64, &[], false),
            ("tyra_json_kind", P, &[I64], false),
            ("tyra_json_is_string", I32, &[I64], false),
            ("tyra_json_is_int", I32, &[I64], false),
            ("tyra_json_is_bool", I32, &[I64], false),
            ("tyra_json_str", P, &[I64], false),
            ("tyra_json_int", I64, &[I64], false),
            ("tyra_json_bool", I32, &[I64], false),
            ("tyra_json_get", I64, &[I64, P], false),
            ("tyra_json_at", I64, &[I64, I64], false),
            ("tyra_http_get", I64, &[P], false),
            ("tyra_http_status", I64, &[I64], false),
            ("tyra_http_body", P, &[I64], false),
            ("tyra_http_errno", I32, &[], false),
            ("tyra_http_errmsg", P, &[], false),
            ("tyra_http_server_new", P, &[], false),
            ("tyra_http_server_route", V, &[P, P, P, P], false),
            ("tyra_http_server_listen", I32, &[P, I64], false),
            ("tyra_io_read_line", P, &[], false),
            ("tyra_io_read_to_end", P, &[], false),
            ("tyra_io_eof", I32, &[], false),
            ("tyra_string_len", I64, &[P], false),
            ("tyra_string_is_empty", I32, &[P], false),
            ("tyra_string_trim", P, &[P], false),
            ("tyra_string_to_upper", P, &[P], false),
            ("tyra_string_to_lower", P, &[P], false),
            ("tyra_string_contains", I32, &[P, P], false),
            ("tyra_string_starts_with", I32, &[P, P], false),
            ("tyra_string_ends_with", I32, &[P, P], false),
            ("tyra_string_parse_int", I64, &[P], false),
            ("tyra_string_parse_errno", I32, &[], false),
            ("tyra_string_byte_at", I64, &[P, I64], false),
            ("tyra_string_substring", P, &[P, I64, I64], false),
            ("tyra_string_reverse", P, &[P], false),
            ("tyra_string_from_byte", P, &[I64], false),
            ("tyra_string_split_whitespace", V, &[P, P], false),
            ("tyra_string_split", V, &[P, P, P], false),
            ("tyra_string_replace", P, &[P, P, P], false),
            ("tyra_string_join", P, &[P, P], false),
            ("tyra_time_now_unix", I64, &[], false),
            ("tyra_time_monotonic_millis", I64, &[], false),
            ("tyra_log_info", V, &[P], false),
            ("tyra_log_warn", V, &[P], false),
            ("tyra_log_error", V, &[P], false),
            ("tyra_float_eq", I32, &[F64, F64], false),
            ("tyra_float_approx_eq", I32, &[F64, F64, F64], false),
            ("tyra_float_abs", F64, &[F64], false),
            ("tyra_float_floor", F64, &[F64], false),
            ("tyra_float_ceil", F64, &[F64], false),
            ("tyra_float_round", F64, &[F64], false),
            ("tyra_float_min", F64, &[F64, F64], false),
            ("tyra_float_max", F64, &[F64, F64], false),
            ("tyra_float_to_string", P, &[F64], false),
            ("tyra_float_parse", F64, &[P], false),
            ("tyra_float_parse_errno", I32, &[], false),
            ("tyra_float_from_int", F64, &[I64], false),
            ("tyra_float_to_int", I64, &[F64], false),
            ("tyra_float_is_nan", I32, &[F64], false),
            ("tyra_float_is_infinite", I32, &[F64], false),
            ("tyra_map_new", P, &[P, P], false),
            ("tyra_map_insert", P, &[P, P, P], false),
            ("tyra_map_remove", P, &[P, P], false),
            ("tyra_map_get", P, &[P, P], false),
            ("tyra_map_contains", I32, &[P, P], false),
            ("tyra_map_contains_key", I32, &[P, P], false),
            ("tyra_map_len", I64, &[P], false),
            ("tyra_map_for_each", V, &[P, P, P], false),
            ("tyra_hash_cstr", I64, &[P], false),
            ("tyra_cstr_eq", I32, &[P, P], false),
            ("tyra_set_new", P, &[P, P], false),
            ("tyra_set_insert", P, &[P, P], false),
            ("tyra_set_remove", P, &[P, P], false),
            ("tyra_set_contains", I32, &[P, P], false),
            ("tyra_set_len", I64, &[P], false),
            ("tyra_set_for_each", V, &[P, P, P], false),
            ("tyra_linked_map_new", P, &[P, P], false),
            ("tyra_linked_map_insert", P, &[P, P, P], false),
            ("tyra_linked_map_remove", P, &[P, P], false),
            ("tyra_linked_map_get", P, &[P, P], false),
            ("tyra_linked_map_contains_key", I32, &[P, P], false),
            ("tyra_linked_map_len", I64, &[P], false),
            ("tyra_linked_map_for_each", V, &[P, P, P], false),
            ("tyra_linked_set_new", P, &[P, P], false),
            ("tyra_linked_set_insert", P, &[P, P], false),
            ("tyra_linked_set_remove", P, &[P, P], false),
            ("tyra_linked_set_contains", I32, &[P, P], false),
            ("tyra_linked_set_len", I64, &[P], false),
            ("tyra_linked_set_for_each", V, &[P, P, P], false),
            ("abort", V, &[], false),
            ("exit", V, &[I32], false),
            ("strcmp", I32, &[P, P], false),
            ("__bench_clock_ns", I64, &[], false),
            ("strtoll", I64, &[P, P, I32], false),
        ];
        for (name, ret, params, varargs) in externs {
            if self.module.get_function(name).is_some() {
                continue;
            }
            let pty: Vec<BasicMetadataTypeEnum<'ctx>> =
                params.iter().map(|k| self.kind_meta(*k)).collect();
            let fn_ty = match ret {
                V => self.ctx.void_type().fn_type(&pty, *varargs),
                other => self.kind_basic(*other).fn_type(&pty, *varargs),
            };
            self.module
                .add_function(name, fn_ty, Some(Linkage::External));
        }
    }

    fn kind_basic(&self, k: ExternKind) -> BasicTypeEnum<'ctx> {
        match k {
            ExternKind::V => unreachable!("void is not a BasicTypeEnum"),
            ExternKind::I32 => self.ctx.i32_type().into(),
            ExternKind::I64 => self.ctx.i64_type().into(),
            ExternKind::F64 => self.ctx.f64_type().into(),
            ExternKind::P => self.ptr().into(),
        }
    }

    fn kind_meta(&self, k: ExternKind) -> BasicMetadataTypeEnum<'ctx> {
        self.kind_basic(k).into()
    }

    /// Declare a function signature for every program function. Bodies are
    /// emitted later (I2); I1 fills them with `unreachable` so the module
    /// verifies. `is_main` functions get the C entry signature
    /// `i32 @main(i32 %argc, ptr %argv)`; `Unit`/`Never` returns map to `void`.
    fn declare_functions(&mut self, program: &Program) {
        for f in &program.functions {
            let fn_ty = if f.is_main {
                let i32t = self.ctx.i32_type();
                i32t.fn_type(&[i32t.into(), self.ptr().into()], false)
            } else {
                let params: Vec<BasicMetadataTypeEnum<'ctx>> = f
                    .params
                    .iter()
                    .map(|(_, ty)| self.ty_to_basic_type(ty).into())
                    .collect();
                match &f.return_type {
                    Ty::Unit | Ty::Never => self.ctx.void_type().fn_type(&params, false),
                    ret => self.ty_to_basic_type(ret).fn_type(&params, false),
                }
            };
            let llvm_name = if f.is_main { "main" } else { f.name.as_str() };
            let fv = self.module.add_function(llvm_name, fn_ty, None);
            self.fn_values.insert(f.name.clone(), fv);
        }
    }

}

/// Compact extern signature kinds for the data-driven `declare_externs` table.
#[derive(Clone, Copy)]
enum ExternKind {
    V,
    I32,
    I64,
    F64,
    P,
}

/// Build a module from `program` via the inkwell backend and return its IR text.
///
/// **I1**: emits the full declaration surface — struct bodies, the closure fat
/// pointer type, module globals (argc/argv, string/source/format constants,
/// panic sentinel, zero slot), runtime externs, and function signatures —
/// then fills each function with a single `unreachable` entry block so the
/// module verifies. Real instruction bodies, builtins, coverage and DWARF land
/// in I2–I6. Not yet wired into the public `emit_llvm_ir*` entry points (the
/// legacy text path remains production until I7).
#[allow(dead_code)]
pub(crate) fn emit_inkwell(program: &Program) -> String {
    let ctx = Context::create();
    build_module(&ctx, program).module.print_to_string().to_string()
}

/// Run the I1 declaration pipeline and return the populated `CodeGen` (module
/// not yet finalized to text). Shared by `emit_inkwell` and tests that need to
/// run `Module::verify()` on the result.
#[allow(dead_code)]
fn build_module<'ctx>(ctx: &'ctx Context, program: &Program) -> CodeGen<'ctx> {
    let mut cg = CodeGen::new(ctx, "tyra");
    cg.register_struct_types(program);
    cg.build_type_scan_maps(program);
    cg.declare_closure_type();
    cg.set_struct_bodies(program);
    cg.declare_globals(program);
    cg.declare_externs();
    cg.emit_collection_eq_hash(program);
    cg.declare_functions(program);
    cg.emit_bodies(program);
    cg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_program() -> Program {
        Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    #[test]
    fn emit_inkwell_produces_module_with_target_triple() {
        let ir = emit_inkwell(&empty_program());
        assert!(ir.contains("target triple"), "IR missing target triple:\n{ir}");
    }

    #[test]
    fn type_bridge_maps_primitives() {
        let ctx = Context::create();
        let cg = CodeGen::new(&ctx, "t");
        assert!(cg.ty_to_basic_type(&Ty::Int).is_int_type());
        assert!(cg.ty_to_basic_type(&Ty::Float).is_float_type());
        assert!(cg.ty_to_basic_type(&Ty::Bool).is_int_type()); // i1
        assert!(cg.ty_to_basic_type(&Ty::String).is_pointer_type());
        assert!(
            cg.ty_to_basic_type(&Ty::Fn(vec![Ty::Int], Box::new(Ty::Int)))
                .is_pointer_type()
        );
    }

    #[test]
    fn data_type_bridges_to_ptr_named_struct_to_struct() {
        let ctx = Context::create();
        let mut cg = CodeGen::new(&ctx, "t");
        let program = Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![
                tyra_mir::StructDef {
                    name: "Heap".into(),
                    fields: vec![("x".into(), Ty::Int)],
                    is_data: true,
                    recursive_fields: vec![false],
                },
                tyra_mir::StructDef {
                    name: "Pair".into(),
                    fields: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                    is_data: false,
                    recursive_fields: vec![false, false],
                },
            ],
            source_files: vec![],
            lower_errors: vec![],
        };
        cg.register_struct_types(&program);
        // data type → ptr
        assert!(cg.ty_to_basic_type(&Ty::Named("Heap".into())).is_pointer_type());
        // value struct → struct type
        assert!(cg.ty_to_basic_type(&Ty::Named("Pair".into())).is_struct_type());
    }

    #[test]
    fn generic_data_type_bridges_to_ptr() {
        let ctx = Context::create();
        let mut cg = CodeGen::new(&ctx, "t");
        // A monomorphized generic data type must resolve to `ptr`, not a struct.
        let boxed = Ty::Generic("Box".into(), vec![Ty::Int]);
        let mono = boxed.monomorphized_name();
        let program = Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: mono,
                fields: vec![("v".into(), Ty::Int)],
                is_data: true,
                recursive_fields: vec![false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        cg.register_struct_types(&program);
        assert!(cg.ty_to_basic_type(&boxed).is_pointer_type());
    }

    fn sample_program() -> Program {
        use tyra_mir::Function;
        Program {
            functions: vec![
                Function {
                    name: "add".into(),
                    params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                    return_type: Ty::Int,
                    body: vec![],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "noop".into(),
                    params: vec![],
                    return_type: Ty::Unit,
                    body: vec![],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "main".into(),
                    params: vec![],
                    return_type: Ty::Int,
                    body: vec![],
                    is_main: true,
                    local_metas: vec![],
                },
            ],
            string_constants: vec!["hello".into()],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec!["main.tyra".into()],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i1_declarations_module_verifies() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &sample_program());
        assert!(
            cg.module.verify().is_ok(),
            "module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
    }

    #[test]
    fn i2a_add_function_emits_real_body() {
        use tyra_mir::{Function, Instruction, MirBinOp, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "add".into(),
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::BinOp {
                        dest: "r".into(),
                        op: MirBinOp::AddInt,
                        lhs: Operand::Var("a".into()),
                        rhs: Operand::Var("b".into()),
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
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
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("add i64"), "missing real add instruction:\n{ir}");
        assert!(ir.contains("ret i64"), "missing typed return:\n{ir}");
    }

    #[test]
    fn i2a_if_expression_emits_phi() {
        // fn pick(c: Bool) -> Int = if c then 1 else 2  (Phi over two consts)
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "pick".into(),
                params: vec![("c".into(), Ty::Bool)],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::BranchIf {
                        cond: Operand::Var("c".into()),
                        true_label: "then".into(),
                        false_label: "els".into(),
                    }),
                    MirStmt::synthetic(Instruction::Label("then".into())),
                    MirStmt::synthetic(Instruction::Const {
                        dest: "t".into(),
                        value: Constant::Int(1),
                    }),
                    MirStmt::synthetic(Instruction::Jump { label: "merge".into() }),
                    MirStmt::synthetic(Instruction::Label("els".into())),
                    MirStmt::synthetic(Instruction::Const {
                        dest: "e".into(),
                        value: Constant::Int(2),
                    }),
                    MirStmt::synthetic(Instruction::Jump { label: "merge".into() }),
                    MirStmt::synthetic(Instruction::Label("merge".into())),
                    MirStmt::synthetic(Instruction::Phi {
                        dest: "r".into(),
                        branches: vec![
                            (Operand::Var("t".into()), "then".into()),
                            (Operand::Var("e".into()), "els".into()),
                        ],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
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
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "phi module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        assert!(cg.module.print_to_string().to_string().contains("phi i64"));
    }

    #[test]
    fn i2b_mutable_local_emits_alloca_store_load() {
        // fn f() -> Int { mut x = 5; x = x + 1; return x }
        use tyra_mir::{Constant, Function, Instruction, MirBinOp, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::Alloca { dest: "x".into() }),
                    MirStmt::synthetic(Instruction::Const {
                        dest: "c5".into(),
                        value: Constant::Int(5),
                    }),
                    MirStmt::synthetic(Instruction::Store {
                        dest: "x".into(),
                        value: Operand::Var("c5".into()),
                    }),
                    MirStmt::synthetic(Instruction::Load {
                        dest: "cur".into(),
                        source: "x".into(),
                    }),
                    MirStmt::synthetic(Instruction::BinOp {
                        dest: "inc".into(),
                        op: MirBinOp::AddInt,
                        lhs: Operand::Var("cur".into()),
                        rhs: Operand::Const(Constant::Int(1)),
                    }),
                    MirStmt::synthetic(Instruction::Store {
                        dest: "x".into(),
                        value: Operand::Var("inc".into()),
                    }),
                    MirStmt::synthetic(Instruction::Load {
                        dest: "r".into(),
                        source: "x".into(),
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
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
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "mut-local module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("alloca i64"), "missing alloca:\n{ir}");
        assert!(ir.contains("store i64"), "missing store:\n{ir}");
        assert!(ir.contains("load i64"), "missing load:\n{ir}");
    }

    #[test]
    fn i2c_struct_init_and_field_get() {
        // fn first(a: Int, b: Int) -> Int { p = Pair{a,b}; return p.0 }
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "first".into(),
                params: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::StructInit {
                        dest: "p".into(),
                        type_name: "Pair".into(),
                        fields: vec![Operand::Var("a".into()), Operand::Var("b".into())],
                    }),
                    MirStmt::synthetic(Instruction::FieldGet {
                        dest: "x".into(),
                        obj: Operand::Var("p".into()),
                        type_name: "Pair".into(),
                        field_index: 0,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("x".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![("a".into(), Ty::Int), ("b".into(), Ty::Int)],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "struct module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("insertvalue"), "missing insertvalue:\n{ir}");
        assert!(ir.contains("extractvalue"), "missing extractvalue:\n{ir}");
    }

    #[test]
    fn i2d_adt_init_tag_payload() {
        // Option<Int>-like ADT: struct { i8 tag, i64 value }.
        // fn unwrap_or(o: OptionInt) -> Int { t = adt_tag o; p = adt_payload o[1]; ... return p }
        // Build Some(7), then read tag + payload, return payload.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "mk_some".into(),
                params: vec![("v".into(), Ty::Int)],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::AdtInit {
                        dest: "o".into(),
                        type_name: "OptionInt".into(),
                        tag: 1,
                        // Param payload (not a constant) so the insertvalue is
                        // not constant-folded away by the IR builder.
                        fields: vec![Operand::Var("v".into())],
                    }),
                    MirStmt::synthetic(Instruction::AdtTag {
                        dest: "tg".into(),
                        obj: Operand::Var("o".into()),
                        type_name: "OptionInt".into(),
                    }),
                    MirStmt::synthetic(Instruction::AdtPayload {
                        dest: "p".into(),
                        obj: Operand::Var("o".into()),
                        type_name: "OptionInt".into(),
                        field_index: 1,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("p".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                // field 0 named "tag" → ADT; i8 tag + i64 payload.
                name: "OptionInt".into(),
                fields: vec![("tag".into(), Ty::Int), ("value".into(), Ty::Int)],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ADT module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        // verify() above is the real gate (it checks types flow correctly
        // through insert/extract). Constant tag/extracts may be folded, so only
        // the non-constant payload insertvalue is reliably present.
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("insertvalue"), "missing adt insertvalue:\n{ir}");
    }

    #[test]
    fn i2d_adt_bool_payload_inactive_zero_verifies() {
        // ADT { i8 tag, i1 value }. The None-like variant fills the i1 payload
        // with the MIR Int(0) placeholder — it must become i1 0, not i64 0.
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "mk_none".into(),
                params: vec![],
                return_type: Ty::Bool,
                body: vec![
                    MirStmt::synthetic(Instruction::AdtInit {
                        dest: "o".into(),
                        type_name: "BoolOpt".into(),
                        tag: 0,
                        fields: vec![Operand::Const(Constant::Int(0))],
                    }),
                    MirStmt::synthetic(Instruction::AdtPayload {
                        dest: "p".into(),
                        obj: Operand::Var("o".into()),
                        type_name: "BoolOpt".into(),
                        field_index: 1,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("p".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "BoolOpt".into(),
                fields: vec![("tag".into(), Ty::Int), ("value".into(), Ty::Bool)],
                is_data: false,
                recursive_fields: vec![false, false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "Bool-payload ADT failed to verify (i64 0 into i1 field?):\n{}",
            cg.module.print_to_string().to_string()
        );
    }

    #[test]
    fn i2e_string_format_emits_snprintf() {
        // fn fmt(n: Int) -> String { s = StringFormat(0, [n]); return s }
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "fmt".into(),
                params: vec![("n".into(), Ty::Int)],
                return_type: Ty::String,
                body: vec![
                    MirStmt::synthetic(Instruction::StringFormat {
                        dest: "s".into(),
                        format_ref: 0,
                        args: vec![Operand::Var("n".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("s".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec!["n=%ld".into()],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "StringFormat module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("@GC_malloc"), "missing buffer alloc:\n{ir}");
        assert!(ir.contains("@snprintf"), "missing snprintf:\n{ir}");
    }

    #[test]
    fn i2a_unsupported_instruction_falls_back_to_unreachable() {
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        // StructInit is not in I2a scope → function must fall back to unreachable.
        let program = Program {
            functions: vec![Function {
                name: "mk".into(),
                params: vec![],
                return_type: Ty::Named("Pair".into()),
                body: vec![MirStmt::synthetic(Instruction::StructInit {
                    dest: "p".into(),
                    type_name: "Pair".into(),
                    fields: vec![Operand::Const(tyra_mir::Constant::Int(1))],
                })],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![tyra_mir::StructDef {
                name: "Pair".into(),
                fields: vec![("a".into(), Ty::Int)],
                is_data: false,
                recursive_fields: vec![false],
            }],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(cg.module.verify().is_ok());
        assert!(cg.module.print_to_string().to_string().contains("unreachable"));
    }

    #[test]
    fn i1_emits_expected_declarations() {
        let ir = emit_inkwell(&sample_program());
        // main entry signature
        assert!(ir.contains("define i32 @main(i32"), "missing @main:\n{ir}");
        // NOTE: named struct types (%struct.Pair, %struct.__closure_fat) are
        // intentionally NOT asserted here — LLVM elides unreferenced named
        // structs from the textual IR, and I1 bodies are `unreachable` so
        // nothing references them yet. Their definition is validated by
        // `Module::verify()` (i1_declarations_module_verifies) and the type
        // bridge tests; they reappear in the IR once I2 emits instructions.
        // runtime/libc externs
        assert!(ir.contains("@printf"));
        assert!(ir.contains("@GC_malloc"));
        assert!(ir.contains("@tyra_rt_init"));
        // globals / format constants
        assert!(ir.contains("@.tyra.argc"));
        assert!(ir.contains("@.fmt.int"));
        assert!(ir.contains("@.str.0")); // "hello"
        // Unit-returning fn lowers to void
        assert!(ir.contains("@noop"));
    }

    // ---- I3: List<T> instructions ----

    /// `List<Int> = { data: ptr, len: i64 }` (§11). `data` is `Ty::String`
    /// (a pointer in LLVM), matching the MIR lowering in `lower/adt.rs`.
    fn list_int_def() -> tyra_mir::StructDef {
        tyra_mir::StructDef {
            name: "List__Int".into(),
            fields: vec![("data".into(), Ty::String), ("len".into(), Ty::Int)],
            is_data: false,
            recursive_fields: vec![false, false],
        }
    }

    /// `Option<Int>` ADT: field 0 named "tag" (→ i8), payload `value: Int`.
    fn option_int_def() -> tyra_mir::StructDef {
        tyra_mir::StructDef {
            name: "Option__Int".into(),
            fields: vec![("tag".into(), Ty::Int), ("value".into(), Ty::Int)],
            is_data: false,
            recursive_fields: vec![false, false],
        }
    }

    #[test]
    fn i3_list_init_emits_gc_malloc_and_verifies() {
        // fn mk() -> List<Int> = [10, 20, 30]
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "mk".into(),
                params: vec![],
                return_type: Ty::Generic("List".into(), vec![Ty::Int]),
                body: vec![
                    MirStmt::synthetic(Instruction::ListInit {
                        dest: "l".into(),
                        elem_type: Ty::Int,
                        elements: vec![
                            Operand::Const(Constant::Int(10)),
                            Operand::Const(Constant::Int(20)),
                            Operand::Const(Constant::Int(30)),
                        ],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("l".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ListInit module failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("@GC_malloc"), "missing GC_malloc:\n{ir}");
        assert!(ir.contains("insertvalue"), "missing struct build:\n{ir}");
    }

    #[test]
    fn i3_empty_list_init_is_null_zero() {
        // fn mk() -> List<Int> = []  → { null, 0 }, no GC_malloc.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "mk".into(),
                params: vec![],
                return_type: Ty::Generic("List".into(), vec![Ty::Int]),
                body: vec![
                    MirStmt::synthetic(Instruction::ListInit {
                        dest: "l".into(),
                        elem_type: Ty::Int,
                        elements: vec![],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("l".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "empty ListInit failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
    }

    #[test]
    fn i3_list_len_and_get_bounds_check_verify() {
        // fn at0(xs: List<Int>) -> Int { n = len xs; _unused; return xs[0] }
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "at0".into(),
                params: vec![("xs".into(), Ty::Generic("List".into(), vec![Ty::Int]))],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::ListLen {
                        dest: "n".into(),
                        list: Operand::Var("xs".into()),
                    }),
                    MirStmt::synthetic(Instruction::ListGet {
                        dest: "e".into(),
                        list: Operand::Var("xs".into()),
                        index: Operand::Const(Constant::Int(0)),
                        elem_type: Ty::Int,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("e".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ListLen/ListGet failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("extractvalue"), "missing len extract:\n{ir}");
        assert!(ir.contains("icmp ult"), "missing bounds compare:\n{ir}");
        assert!(ir.contains("@exit"), "missing OOB exit:\n{ir}");
    }

    #[test]
    fn i3_list_get_safe_emits_option_phi() {
        // fn safe(xs: List<Int>) -> Option<Int> = xs.get(0)
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "safe".into(),
                params: vec![("xs".into(), Ty::Generic("List".into(), vec![Ty::Int]))],
                return_type: Ty::Generic("Option".into(), vec![Ty::Int]),
                body: vec![
                    MirStmt::synthetic(Instruction::ListGetSafe {
                        dest: "o".into(),
                        list: Operand::Var("xs".into()),
                        index: Operand::Const(Constant::Int(0)),
                        elem_type: Ty::Int,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("o".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def(), option_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ListGetSafe failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("phi"), "missing Some/None merge phi:\n{ir}");
    }

    #[test]
    fn i3_list_push_emits_memcpy() {
        // fn add(xs: List<Int>, v: Int) -> List<Int> = xs.push(v)
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "add".into(),
                params: vec![
                    ("xs".into(), Ty::Generic("List".into(), vec![Ty::Int])),
                    ("v".into(), Ty::Int),
                ],
                return_type: Ty::Generic("List".into(), vec![Ty::Int]),
                body: vec![
                    MirStmt::synthetic(Instruction::ListPush {
                        dest: "l2".into(),
                        list: Operand::Var("xs".into()),
                        elem: Operand::Var("v".into()),
                        elem_type: Ty::Int,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("l2".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ListPush failed to verify:\n{}",
            cg.module.print_to_string().to_string()
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("@GC_malloc"), "missing alloc:\n{ir}");
        assert!(ir.contains("llvm.memcpy"), "missing prefix memcpy:\n{ir}");
    }

    #[test]
    fn i3_list_get_in_phi_predecessor_block_verifies() {
        // The phi-predecessor regression: a ListGet whose bounds check splits a
        // block that is itself a phi predecessor. Without the per-instruction
        // label→block sync, the phi would record the *entry* of the `then`
        // region as its predecessor, but the branch actually leaves from the
        // split `e.ok` block — and Module::verify() would reject the mismatch.
        //
        // fn pick(xs: List<Int>, c: Bool) -> Int {
        //   branch c ? then : els
        //   then: a = xs[0]; jump merge
        //   els:        jump merge
        //   merge: r = phi [a, then], [0, els]; return r
        // }
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "pick".into(),
                params: vec![
                    ("xs".into(), Ty::Generic("List".into(), vec![Ty::Int])),
                    ("c".into(), Ty::Bool),
                ],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::BranchIf {
                        cond: Operand::Var("c".into()),
                        true_label: "then".into(),
                        false_label: "els".into(),
                    }),
                    MirStmt::synthetic(Instruction::Label("then".into())),
                    MirStmt::synthetic(Instruction::ListGet {
                        dest: "a".into(),
                        list: Operand::Var("xs".into()),
                        index: Operand::Const(Constant::Int(0)),
                        elem_type: Ty::Int,
                    }),
                    MirStmt::synthetic(Instruction::Jump { label: "merge".into() }),
                    MirStmt::synthetic(Instruction::Label("els".into())),
                    MirStmt::synthetic(Instruction::Jump { label: "merge".into() }),
                    MirStmt::synthetic(Instruction::Label("merge".into())),
                    MirStmt::synthetic(Instruction::Phi {
                        dest: "r".into(),
                        branches: vec![
                            (Operand::Var("a".into()), "then".into()),
                            (Operand::Const(Constant::Int(0)), "els".into()),
                        ],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        assert!(
            cg.module.verify().is_ok(),
            "ListGet-in-phi-predecessor failed to verify (block-sync regression?):\n{}",
            cg.module.print_to_string().to_string()
        );
    }

    #[test]
    fn i3_backedge_to_split_label_targets_entry() {
        // Regression: a back-edge that jumps to a label whose region was split
        // by a ListGet must land at the region *entry* (re-running the bounds
        // check + extractvalue), NOT at the split `.ok` block. If the
        // jump-target table is corrupted by the phi-predecessor sync, the
        // back-edge enters `.ok` directly — and the `data`/`len` extractvalues
        // (defined in the entry block) no longer dominate their uses in `.ok`,
        // so Module::verify() rejects the function.
        //
        // fn f(xs: List<Int>, c: Bool) -> Int {
        //   jump loop
        //   loop: e = xs[0]; branch c ? loop : done   // loop is split by ListGet
        //   done: return 0
        // }
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![
                    ("xs".into(), Ty::Generic("List".into(), vec![Ty::Int])),
                    ("c".into(), Ty::Bool),
                ],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::Jump { label: "loop".into() }),
                    MirStmt::synthetic(Instruction::Label("loop".into())),
                    MirStmt::synthetic(Instruction::ListGet {
                        dest: "e".into(),
                        list: Operand::Var("xs".into()),
                        index: Operand::Const(Constant::Int(0)),
                        elem_type: Ty::Int,
                    }),
                    MirStmt::synthetic(Instruction::BranchIf {
                        cond: Operand::Var("c".into()),
                        true_label: "loop".into(),
                        false_label: "done".into(),
                    }),
                    MirStmt::synthetic(Instruction::Label("done".into())),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Const(Constant::Int(0))),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_int_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        // `verify()` does NOT catch this: corrupting the jump target to `%e.ok`
        // still produces dominator-valid IR (every path to `%e.ok` enters via
        // the loop entry), it just miscompiles control flow. The guard is
        // structural: the back-edge (the conditional branch in the bounds-check
        // `.ok` block) must target the loop *entry* `%loop`, which re-runs the
        // bounds check each iteration — the buggy sync would emit `label %e.ok`.
        assert!(cg.module.verify().is_ok(), "back-edge module failed to verify:\n{ir}");
        assert!(
            ir.contains("br i1 %c, label %loop, label %done"),
            "back-edge must target loop entry %loop (not the split %e.ok block):\n{ir}"
        );
    }

    // ---- I4a: table-driven mechanical builtins ----

    /// Build a one-function program whose body is a single builtin Call (with
    /// `dest`) followed by `return dest`, for exercising the I4a table.
    fn builtin_call_program(
        fn_name: &str,
        params: Vec<(String, Ty)>,
        ret: Ty,
        builtin: &str,
        args: Vec<tyra_mir::Operand>,
    ) -> Program {
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        Program {
            functions: vec![Function {
                name: fn_name.into(),
                params,
                return_type: ret,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("r".into()),
                        func: builtin.into(),
                        args,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i4a_string_len_direct_i64() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("s".into(), Ty::String)],
            Ty::Int,
            "__string_len",
            vec![Operand::Var("s".into())],
        );
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call i64 @tyra_string_len"), "missing runtime call:\n{ir}");
    }

    #[test]
    fn i4a_string_contains_bool_from_i32() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("a".into(), Ty::String), ("b".into(), Ty::String)],
            Ty::Bool,
            "__string_contains",
            vec![Operand::Var("a".into()), Operand::Var("b".into())],
        );
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call i32 @tyra_string_contains"), "missing runtime call:\n{ir}");
        assert!(ir.contains("icmp ne i32"), "missing i32→i1 bool conversion:\n{ir}");
    }

    #[test]
    fn i4a_fs_errno_sext_to_i64() {
        let ctx = Context::create();
        let program = builtin_call_program("f", vec![], Ty::Int, "__fs_errno", vec![]);
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call i32 @tyra_fs_errno"), "missing runtime call:\n{ir}");
        assert!(ir.contains("sext i32"), "missing i32→i64 sext:\n{ir}");
    }

    #[test]
    fn i4a_float_abs_direct_double() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("x".into(), Ty::Float)],
            Ty::Float,
            "__float_abs",
            vec![Operand::Var("x".into())],
        );
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call double @tyra_float_abs"), "missing runtime call:\n{ir}");
    }

    #[test]
    fn i4a_log_info_void_call() {
        // fn f(m: String) -> Unit { __log_info(m) }  — void runtime call, no dest.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("m".into(), Ty::String)],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: None,
                        func: "__log_info".into(),
                        args: vec![Operand::Var("m".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call void @tyra_log_info"), "missing void runtime call:\n{ir}");
    }

    #[test]
    fn i4a_deferred_builtin_falls_back_to_unreachable() {
        // A still-deferred builtin (`__list_fold_int`, a higher-order list op —
        // I4f) is not in the supported set, so the function must fall back to a
        // single `unreachable` block. Coverage grows in later I4 sub-phases.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("m".into(), Ty::Int)],
                return_type: Ty::Int,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("r".into()),
                        func: "__list_fold_int".into(),
                        args: vec![Operand::Var("m".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
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
        let cg = build_module(&ctx, &program);
        assert!(cg.module.verify().is_ok());
        let ir = cg.module.print_to_string().to_string();
        assert!(ir.contains("unreachable"), "deferred builtin should fall back:\n{ir}");
        // A real body (loops etc.) would have >1 block; the fallback is exactly
        // one block holding only `unreachable`.
        let f = cg.module.get_function("f").unwrap();
        assert_eq!(
            f.count_basic_blocks(),
            1,
            "deferred builtin must not emit a real body:\n{ir}"
        );
    }

    // ---- I4d: control-flow / process sentinels (panic / sys.exit / sys.args) ----

    #[test]
    fn i4d_panic_emits_loc_msg_sentinel_and_exit101() {
        // panic(msg) with a real source loc → "panic at FILE:LINE", message,
        // sentinel (all to fd 2), then exit(101) + unreachable. The trailing
        // `Return` is dead code and must be skipped (no `ret` in the body).
        use tyra_mir::{Function, Instruction, MirStmt, Operand, SourceLoc};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("m".into(), Ty::String)],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::new(
                        SourceLoc { file_id: 0, line: 7, col: 3 },
                        Instruction::Call {
                            dest: None,
                            func: "panic".into(),
                            args: vec![Operand::Var("m".into())],
                        },
                    ),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec!["main.tyra".into()],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "panic body failed to verify:\n{ir}");
        // `ptr @.fmt.panic_loc` / `ptr @.src.0` appear only when used as a call
        // argument (the global *definitions* are `@... = private ...`), so these
        // assert the loc line is actually emitted, not merely declared.
        assert!(ir.contains("ptr @.fmt.panic_loc"), "panic-loc format must be used:\n{ir}");
        assert!(ir.contains("ptr @.src.0"), "source-file global must be passed:\n{ir}");
        assert!(ir.contains("@.fmt.str_ln"), "missing message+newline format:\n{ir}");
        assert!(ir.contains("@.str.panic_sentinel"), "missing panic sentinel:\n{ir}");
        assert!(ir.contains("call void @exit(i32 101)"), "panic must exit(101):\n{ir}");
        assert!(ir.contains("unreachable"), "panic must end in unreachable:\n{ir}");
        assert!(
            !ir.contains("ret "),
            "the dead trailing Return after panic must be skipped:\n{ir}"
        );
    }

    #[test]
    fn i4d_panic_without_loc_skips_file_line() {
        // A synthetic (dummy) loc omits the "panic at FILE:LINE" line but still
        // prints the message, sentinel, and exits.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("m".into(), Ty::String)],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: None,
                        func: "panic".into(),
                        args: vec![Operand::Var("m".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec!["main.tyra".into()],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "panic body failed to verify:\n{ir}");
        assert!(
            !ir.contains("ptr @.fmt.panic_loc"),
            "dummy loc must skip the FILE:LINE line:\n{ir}"
        );
        assert!(ir.contains("@.str.panic_sentinel"), "missing panic sentinel:\n{ir}");
        assert!(ir.contains("call void @exit(i32 101)"), "panic must exit(101):\n{ir}");
    }

    #[test]
    fn i4d_sys_exit_truncates_code_and_exits() {
        // sys.exit(3): truncate the Int (i64) code to i32, exit, unreachable.
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: None,
                        func: "sys__exit".into(),
                        args: vec![Operand::Const(Constant::Int(3))],
                    }),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "sys.exit body failed to verify:\n{ir}");
        assert!(ir.contains("call void @exit"), "sys.exit must call exit:\n{ir}");
        assert!(ir.contains("unreachable"), "sys.exit must end in unreachable:\n{ir}");
    }

    #[test]
    fn i4d_sys_args_builds_list_string() {
        // sys.args() materializes a List<String> from argc/argv via a copy loop.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![],
                return_type: Ty::Generic("List".into(), vec![Ty::String]),
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("a".into()),
                        func: "sys__args".into(),
                        args: vec![],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("a".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_string_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "sys.args body failed to verify:\n{ir}");
        assert!(ir.contains("@.tyra.argc"), "sys.args must load argc:\n{ir}");
        assert!(ir.contains("@.tyra.argv"), "sys.args must load argv:\n{ir}");
        assert!(ir.contains("@GC_malloc"), "sys.args must allocate the data array:\n{ir}");
    }

    // ---- I4e: boxed collections (Map/Set/LinkedMap/LinkedSet) ----

    /// `fn f(<params>) { <calls>; return <ret_var?> }` for collection tests.
    fn seq_program(
        params: Vec<(String, Ty)>,
        ret: Ty,
        calls: Vec<(Option<&str>, &str, Vec<tyra_mir::Operand>)>,
        ret_var: Option<&str>,
    ) -> Program {
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let mut body: Vec<MirStmt> = calls
            .into_iter()
            .map(|(d, f, a)| {
                MirStmt::synthetic(Instruction::Call {
                    dest: d.map(|s| s.to_string()),
                    func: f.to_string(),
                    args: a,
                })
            })
            .collect();
        body.push(MirStmt::synthetic(Instruction::Return {
            value: ret_var.map(|v| Operand::Var(v.to_string())),
        }));
        Program {
            functions: vec![Function {
                name: "f".into(),
                params,
                return_type: ret,
                body,
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i4e_map_int_int_new_insert_contains_len() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &seq_program(
                vec![("k".into(), Ty::Int), ("v".into(), Ty::Int)],
                Ty::Int,
                vec![
                    (Some("m0"), "__map_new__Int__Int", vec![]),
                    (
                        Some("m1"),
                        "__map_insert__Int__Int",
                        vec![Operand::Var("m0".into()), Operand::Var("k".into()), Operand::Var("v".into())],
                    ),
                    (
                        Some("has"),
                        "__map_contains__Int",
                        vec![Operand::Var("m1".into()), Operand::Var("k".into())],
                    ),
                    (Some("n"), "__map_len", vec![Operand::Var("m1".into())]),
                ],
                Some("n"),
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "map ops failed to verify:\n{ir}");
        // `new` threads the compiler-generated eq/hash fn pointers.
        assert!(
            ir.contains("@tyra_map_new(ptr @tyra_eq_Int, ptr @tyra_hash_Int)"),
            "map.new must pass eq/hash fn pointers:\n{ir}"
        );
        assert!(ir.contains("call ptr @tyra_map_insert"), "missing map.insert:\n{ir}");
        assert!(ir.contains("call i32 @tyra_map_contains"), "missing map.contains:\n{ir}");
        assert!(ir.contains("call i64 @tyra_map_len"), "missing map.len:\n{ir}");
        // eq/hash function bodies are emitted; Int hash uses the Knuth constant.
        assert!(ir.contains("@tyra_eq_Int("), "eq_Int must be defined:\n{ir}");
        assert!(ir.contains("-3932073806218323177"), "Int hash must use the Knuth multiply:\n{ir}");
        // Boxing allocates 8-byte cells.
        assert!(ir.contains("@GC_malloc"), "insert must box via GC_malloc:\n{ir}");
    }

    #[test]
    fn i4e_set_bool_boxes_via_zext() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &seq_program(
                vec![("b".into(), Ty::Bool)],
                Ty::Bool,
                vec![
                    (Some("s0"), "__set_new__Bool", vec![]),
                    (
                        Some("s1"),
                        "__set_insert__Bool",
                        vec![Operand::Var("s0".into()), Operand::Var("b".into())],
                    ),
                    (
                        Some("has"),
                        "__set_contains__Bool",
                        vec![Operand::Var("s1".into()), Operand::Var("b".into())],
                    ),
                ],
                Some("has"),
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "set ops failed to verify:\n{ir}");
        assert!(
            ir.contains("@tyra_set_new(ptr @tyra_eq_Bool, ptr @tyra_hash_Bool)"),
            "set.new must pass Bool eq/hash:\n{ir}"
        );
        assert!(ir.contains("call ptr @tyra_set_insert"), "missing set.insert:\n{ir}");
        // Bool boxing zero-extends the i1 to i8 before storing.
        assert!(ir.contains("zext i1"), "Bool box must zext i1->i8:\n{ir}");
    }

    #[test]
    fn i4e_linked_map_string_uses_contains_key() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &seq_program(
                vec![("k".into(), Ty::String), ("v".into(), Ty::Int)],
                Ty::Bool,
                vec![
                    (Some("m0"), "__linked_map_new__String__Int", vec![]),
                    (
                        Some("m1"),
                        "__linked_map_insert__String__Int",
                        vec![Operand::Var("m0".into()), Operand::Var("k".into()), Operand::Var("v".into())],
                    ),
                    (
                        Some("has"),
                        "__linked_map_contains__String",
                        vec![Operand::Var("m1".into()), Operand::Var("k".into())],
                    ),
                ],
                Some("has"),
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "linked_map ops failed to verify:\n{ir}");
        // contains routes to the `_contains_key` callee (the one naming divergence).
        assert!(
            ir.contains("call i32 @tyra_linked_map_contains_key"),
            "linked_map.contains must call tyra_linked_map_contains_key:\n{ir}"
        );
        // String eq/hash delegate to the cstr runtime helpers.
        assert!(ir.contains("@tyra_eq_String("), "eq_String must be defined:\n{ir}");
        assert!(ir.contains("@tyra_cstr_eq"), "String eq must delegate to tyra_cstr_eq:\n{ir}");
        assert!(ir.contains("@tyra_hash_cstr"), "String hash must delegate to tyra_hash_cstr:\n{ir}");
    }

    #[test]
    fn i4e_linked_set_int_new_insert_remove_len() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &seq_program(
                vec![("x".into(), Ty::Int)],
                Ty::Int,
                vec![
                    (Some("s0"), "__linked_set_new__Int", vec![]),
                    (
                        Some("s1"),
                        "__linked_set_insert__Int",
                        vec![Operand::Var("s0".into()), Operand::Var("x".into())],
                    ),
                    (
                        Some("s2"),
                        "__linked_set_remove__Int",
                        vec![Operand::Var("s1".into()), Operand::Var("x".into())],
                    ),
                    (Some("n"), "__linked_set_len", vec![Operand::Var("s2".into())]),
                ],
                Some("n"),
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "linked_set ops failed to verify:\n{ir}");
        assert!(ir.contains("call ptr @tyra_linked_set_new"), "missing linked_set.new:\n{ir}");
        assert!(ir.contains("call ptr @tyra_linked_set_insert"), "missing linked_set.insert:\n{ir}");
        assert!(ir.contains("call ptr @tyra_linked_set_remove"), "missing linked_set.remove:\n{ir}");
        assert!(ir.contains("call i64 @tyra_linked_set_len"), "missing linked_set.len:\n{ir}");
    }

    // ---- I4c: print family (type-scan-routed) ----

    /// Build `fn f(p: ty) -> Unit { <builtin>(p) }` for print-family tests.
    fn print_program(builtin: &str, arg_ty: Ty, structs: Vec<tyra_mir::StructDef>) -> Program {
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("p".into(), arg_ty)],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: None,
                        func: builtin.into(),
                        args: vec![Operand::Var("p".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: structs,
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i4c_println_string_uses_puts() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &print_program("println", Ty::String, vec![]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("call i32 @puts"), "println(String) should use puts:\n{ir}");
    }

    #[test]
    fn i4c_print_string_uses_printf_s() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &print_program("print", Ty::String, vec![]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("@printf"), "print(String) should use printf:\n{ir}");
        assert!(ir.contains("@.fmt.str"), "print(String) should use the %s format:\n{ir}");
    }

    #[test]
    fn i4c_print_int_uses_printf_ld() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &print_program("print", Ty::Int, vec![]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("@.fmt.int"), "print(Int) should use the %ld format:\n{ir}");
    }

    #[test]
    fn i4c_println_float_uses_float_ln() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &print_program("println", Ty::Float, vec![]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("@.fmt.float_ln"), "println(Float) should use %g\\n:\n{ir}");
    }

    #[test]
    fn i4c_println_bool_widens_to_i64() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &print_program("println", Ty::Bool, vec![]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("zext i1"), "println(Bool) should widen i1→i64:\n{ir}");
        assert!(ir.contains("@.fmt.int_ln"), "println(Bool) should print via %ld\\n:\n{ir}");
    }

    #[test]
    fn i4c_println_data_type_uses_puts_legacy_parity() {
        // A `data` type lowers to a ptr and the type scan puts it in
        // string_temps (type_scan.rs:154), so println(dataObject) routes to
        // `puts` — the runtime reads its bytes as a C string. This is a latent
        // legacy behavior; the inkwell backend reproduces it faithfully (parity,
        // not "fix print"). Documented here so the routing is intentional, not
        // accidental, and so the data-ptr path is covered.
        let data_foo = tyra_mir::StructDef {
            name: "Foo".into(),
            fields: vec![("x".into(), Ty::Int)],
            is_data: true,
            recursive_fields: vec![false],
        };
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &print_program("println", Ty::Named("Foo".into()), vec![data_foo]),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        // data ptr → %s/puts (legacy parity), NOT the %ld address path.
        assert!(
            ir.contains("call i32 @puts"),
            "println(data) must route to puts like legacy (data ∈ string_temps):\n{ir}"
        );
    }

    #[test]
    fn i4c_print_struct_arg_falls_back_to_unreachable() {
        // print(List<Int>) is not printable (no printf form for a struct value);
        // the gate must reject it so the function falls back to `unreachable`
        // rather than reaching emit_print (which would panic coercing a struct).
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &print_program("println", Ty::Generic("List".into(), vec![Ty::Int]), vec![list_int_def()]),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "module failed to verify:\n{ir}");
        assert!(ir.contains("unreachable"), "print(struct) should fall back:\n{ir}");
        assert!(!ir.contains("call i32 @puts"), "must not emit a print for a struct arg:\n{ir}");
    }

    // ---- I4b slice A: scalar __list_int_* builtins ----

    /// Build `fn f() -> ret { l = [10, 20, 30]; r = <builtin>(l, extra...); r }`.
    /// The list is a local SSA temp (no struct-param slot), exercising the
    /// builtin's struct-handle reads directly.
    fn list_int_builtin_program(
        builtin: &str,
        extra_args: Vec<tyra_mir::Operand>,
        ret: Ty,
        structs: Vec<tyra_mir::StructDef>,
    ) -> Program {
        use tyra_mir::{Constant, Function, Instruction, MirStmt, Operand};
        let mut call_args = vec![Operand::Var("l".into())];
        call_args.extend(extra_args);
        Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![],
                return_type: ret,
                body: vec![
                    MirStmt::synthetic(Instruction::ListInit {
                        dest: "l".into(),
                        elem_type: Ty::Int,
                        elements: vec![
                            Operand::Const(Constant::Int(10)),
                            Operand::Const(Constant::Int(20)),
                            Operand::Const(Constant::Int(30)),
                        ],
                    }),
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("r".into()),
                        func: builtin.into(),
                        args: call_args,
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: structs,
            source_files: vec![],
            lower_errors: vec![],
        }
    }

    #[test]
    fn i4b_list_int_sum_verifies_and_loops() {
        let ctx = Context::create();
        let cg = build_module(&ctx, &list_int_builtin_program("__list_int_sum", vec![], Ty::Int, vec![list_int_def()]));
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_sum failed to verify:\n{ir}");
        // Accumulator loop: a back-edge and an i64 add must be present.
        assert!(ir.contains("add i64"), "sum must accumulate with i64 add:\n{ir}");
    }

    #[test]
    fn i4b_list_int_contains_returns_bool() {
        use tyra_mir::{Constant, Operand};
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &list_int_builtin_program(
                "__list_int_contains",
                vec![Operand::Const(Constant::Int(20))],
                Ty::Bool,
                vec![list_int_def()],
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_contains failed to verify:\n{ir}");
        assert!(ir.contains("icmp eq i64"), "contains must compare elements:\n{ir}");
    }

    #[test]
    fn i4b_list_int_index_of_returns_option() {
        use tyra_mir::{Constant, Operand};
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &list_int_builtin_program(
                "__list_int_index_of",
                vec![Operand::Const(Constant::Int(20))],
                Ty::Generic("Option".into(), vec![Ty::Int]),
                vec![list_int_def(), option_int_def()],
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_index_of failed to verify:\n{ir}");
        assert!(ir.contains("insertvalue"), "index_of must build an Option struct:\n{ir}");
    }

    #[test]
    fn i4b_list_int_max_returns_option() {
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &list_int_builtin_program(
                "__list_int_max",
                vec![],
                Ty::Generic("Option".into(), vec![Ty::Int]),
                vec![list_int_def(), option_int_def()],
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_max failed to verify:\n{ir}");
        assert!(ir.contains("icmp sgt i64"), "max must use a signed-greater compare:\n{ir}");
    }

    #[test]
    fn i4b_list_int_min_returns_option() {
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &list_int_builtin_program(
                "__list_int_min",
                vec![],
                Ty::Generic("Option".into(), vec![Ty::Int]),
                vec![list_int_def(), option_int_def()],
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_min failed to verify:\n{ir}");
        assert!(ir.contains("icmp slt i64"), "min must use a signed-less compare:\n{ir}");
    }

    #[test]
    fn i4b_list_int_push_appends_and_verifies() {
        use tyra_mir::{Constant, Operand};
        let ctx = Context::create();
        let cg = build_module(
            &ctx,
            &list_int_builtin_program(
                "__list_int_push",
                vec![Operand::Const(Constant::Int(40))],
                Ty::Generic("List".into(), vec![Ty::Int]),
                vec![list_int_def()],
            ),
        );
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "list_int_push failed to verify:\n{ir}");
        // Delegates to the ListPush emitter: GC_malloc + memcpy prefix.
        assert!(ir.contains("@GC_malloc"), "push must allocate a new buffer:\n{ir}");
        assert!(ir.contains("memcpy"), "push must copy the prefix:\n{ir}");
    }

    // ---- I4b slice B: string builtins passing List<String> by reference ----

    /// `List<String>` (`{ ptr, i64 }`) struct def for split/join tests.
    fn list_string_def() -> tyra_mir::StructDef {
        tyra_mir::StructDef {
            name: "List__String".into(),
            fields: vec![("data".into(), Ty::String), ("len".into(), Ty::Int)],
            is_data: false,
            recursive_fields: vec![false, false],
        }
    }

    #[test]
    fn i4b_string_split_whitespace_out_param() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("s".into(), Ty::String)],
            Ty::Generic("List".into(), vec![Ty::String]),
            "__string_split_whitespace",
            vec![Operand::Var("s".into())],
        );
        let mut program = program;
        program.struct_defs = vec![list_string_def()];
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "split_whitespace failed to verify:\n{ir}");
        assert!(
            ir.contains("call void @tyra_string_split_whitespace"),
            "must call the void out-param runtime fn:\n{ir}"
        );
    }

    #[test]
    fn i4b_string_split_with_sep_out_param() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let mut program = builtin_call_program(
            "f",
            vec![("s".into(), Ty::String), ("sep".into(), Ty::String)],
            Ty::Generic("List".into(), vec![Ty::String]),
            "__string_split",
            vec![Operand::Var("s".into()), Operand::Var("sep".into())],
        );
        program.struct_defs = vec![list_string_def()];
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "split failed to verify:\n{ir}");
        assert!(ir.contains("call void @tyra_string_split"), "must call split runtime fn:\n{ir}");
    }

    #[test]
    fn i4b_string_join_passes_list_by_ref() {
        // fn f(s: String) -> String { t = split_whitespace(s); join(t, s) }
        // Chaining split→join exercises both producing and consuming a
        // List<String> value without needing string-constant setup.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![("s".into(), Ty::String)],
                return_type: Ty::String,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("t".into()),
                        func: "__string_split_whitespace".into(),
                        args: vec![Operand::Var("s".into())],
                    }),
                    MirStmt::synthetic(Instruction::Call {
                        dest: Some("r".into()),
                        func: "__string_join".into(),
                        args: vec![Operand::Var("t".into()), Operand::Var("s".into())],
                    }),
                    MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("r".into())),
                    }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![list_string_def()],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "join failed to verify:\n{ir}");
        assert!(ir.contains("call ptr @tyra_string_join"), "must call join runtime fn:\n{ir}");
        // The list value is stored into a slot before the by-ref call.
        assert!(ir.contains("store"), "join must store the list into a slot:\n{ir}");
    }

    #[test]
    fn i4b_string_replace_rides_simple_table() {
        // __string_replace returns String (not a list), so it uses the SIMPLE
        // table's plain build_call path: ptr×3 → ptr, Direct.
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("s".into(), Ty::String), ("from".into(), Ty::String), ("to".into(), Ty::String)],
            Ty::String,
            "__string_replace",
            vec![Operand::Var("s".into()), Operand::Var("from".into()), Operand::Var("to".into())],
        );
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "replace failed to verify:\n{ir}");
        assert!(ir.contains("call ptr @tyra_string_replace"), "must call replace runtime fn:\n{ir}");
    }

    // ---- I4b slice D: http server builtins (handle ptr↔int round-trip) ----

    #[test]
    fn i4b_http_server_new_ptrtoint() {
        let ctx = Context::create();
        let program = builtin_call_program("f", vec![], Ty::Int, "__http_server_new", vec![]);
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "server_new failed to verify:\n{ir}");
        assert!(ir.contains("call ptr @tyra_http_server_new"), "must call the runtime fn:\n{ir}");
        assert!(ir.contains("ptrtoint"), "handle must be stored as Int via ptrtoint:\n{ir}");
    }

    #[test]
    fn i4b_http_server_listen_inttoptr_sext() {
        use tyra_mir::Operand;
        let ctx = Context::create();
        let program = builtin_call_program(
            "f",
            vec![("srv".into(), Ty::Int), ("port".into(), Ty::Int)],
            Ty::Int,
            "__http_server_listen",
            vec![Operand::Var("srv".into()), Operand::Var("port".into())],
        );
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "server_listen failed to verify:\n{ir}");
        assert!(ir.contains("inttoptr"), "handle must be cast back to ptr:\n{ir}");
        assert!(ir.contains("call i32 @tyra_http_server_listen"), "must call the runtime fn:\n{ir}");
        assert!(ir.contains("sext i32"), "i32 result must sext to the Tyra Int:\n{ir}");
    }

    #[test]
    fn i4b_http_server_route_passes_handler_ptr() {
        // fn f(srv: Int, method: String, path: String, handler: Fn) {
        //   route(srv, method, path, handler)
        // }
        // The handler is a function-typed param (→ ptr), so it passes straight
        // through without needing ClosureBuild support.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let handler_ty = Ty::Fn(vec![], Box::new(Ty::Unit));
        let program = Program {
            functions: vec![Function {
                name: "f".into(),
                params: vec![
                    ("srv".into(), Ty::Int),
                    ("method".into(), Ty::String),
                    ("path".into(), Ty::String),
                    ("handler".into(), handler_ty),
                ],
                return_type: Ty::Unit,
                body: vec![
                    MirStmt::synthetic(Instruction::Call {
                        dest: None,
                        func: "__http_server_route".into(),
                        args: vec![
                            Operand::Var("srv".into()),
                            Operand::Var("method".into()),
                            Operand::Var("path".into()),
                            Operand::Var("handler".into()),
                        ],
                    }),
                    MirStmt::synthetic(Instruction::Return { value: None }),
                ],
                is_main: false,
                local_metas: vec![],
            }],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "server_route failed to verify:\n{ir}");
        assert!(ir.contains("inttoptr"), "handle must be cast back to ptr:\n{ir}");
        assert!(
            ir.contains("call void @tyra_http_server_route"),
            "must call the void runtime fn with four ptrs:\n{ir}"
        );
    }

    #[test]
    fn i4b_http_server_route_bare_top_level_handler() {
        // The REAL stdlib path: app.get("/foo", my_handler) lowers the handler
        // as a bare Operand::Var("my_handler") — a top-level function name that
        // is never a temp or a param (call.rs:1343, expr.rs:152). The gate must
        // admit it (a function name is always resolvable) and operand() must
        // resolve it to the function's global pointer (@my_handler), mirroring
        // the legacy `emit_call_args_typed`. Without this, `f` would fall back
        // to a single `unreachable` block and never emit the route call.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![
                // The handler itself (its signature is irrelevant to passing its
                // address; the runtime casts the ptr to fn(*Request)->*Response).
                Function {
                    name: "my_handler".into(),
                    params: vec![("req".into(), Ty::String)],
                    return_type: Ty::String,
                    body: vec![MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("req".into())),
                    })],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "f".into(),
                    params: vec![
                        ("srv".into(), Ty::Int),
                        ("method".into(), Ty::String),
                        ("path".into(), Ty::String),
                    ],
                    return_type: Ty::Unit,
                    body: vec![
                        MirStmt::synthetic(Instruction::Call {
                            dest: None,
                            func: "__http_server_route".into(),
                            args: vec![
                                Operand::Var("srv".into()),
                                Operand::Var("method".into()),
                                Operand::Var("path".into()),
                                // Bare top-level function name, NOT a param/temp.
                                Operand::Var("my_handler".into()),
                            ],
                        }),
                        MirStmt::synthetic(Instruction::Return { value: None }),
                    ],
                    is_main: false,
                    local_metas: vec![],
                },
            ],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "bare-handler route failed to verify:\n{ir}");
        // `f` must be emitted (not a fallback): the route call is present...
        assert!(
            ir.contains("call void @tyra_http_server_route"),
            "real handler path must emit the route call, not fall back to unreachable:\n{ir}"
        );
        // ...with the handler resolved to the function's global pointer.
        assert!(
            ir.contains("ptr @my_handler)"),
            "handler must be passed as the @my_handler global pointer:\n{ir}"
        );
    }

    #[test]
    fn i4b_let_bound_function_reference_resolves_to_global() {
        // `let h = my_handler; route(srv, method, path, h)` — the binding lowers
        // to Instruction::Copy { dest: "h", source: "my_handler" }. Both the gate
        // and the Copy emitter must treat a top-level function name as a value
        // (resolving to @my_handler), else `f` falls back to unreachable / panics.
        use tyra_mir::{Function, Instruction, MirStmt, Operand};
        let ctx = Context::create();
        let program = Program {
            functions: vec![
                Function {
                    name: "my_handler".into(),
                    params: vec![("req".into(), Ty::String)],
                    return_type: Ty::String,
                    body: vec![MirStmt::synthetic(Instruction::Return {
                        value: Some(Operand::Var("req".into())),
                    })],
                    is_main: false,
                    local_metas: vec![],
                },
                Function {
                    name: "f".into(),
                    params: vec![
                        ("srv".into(), Ty::Int),
                        ("method".into(), Ty::String),
                        ("path".into(), Ty::String),
                    ],
                    return_type: Ty::Unit,
                    body: vec![
                        // let h = my_handler
                        MirStmt::synthetic(Instruction::Copy {
                            dest: "h".into(),
                            source: "my_handler".into(),
                        }),
                        MirStmt::synthetic(Instruction::Call {
                            dest: None,
                            func: "__http_server_route".into(),
                            args: vec![
                                Operand::Var("srv".into()),
                                Operand::Var("method".into()),
                                Operand::Var("path".into()),
                                Operand::Var("h".into()),
                            ],
                        }),
                        MirStmt::synthetic(Instruction::Return { value: None }),
                    ],
                    is_main: false,
                    local_metas: vec![],
                },
            ],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
        let cg = build_module(&ctx, &program);
        let ir = cg.module.print_to_string().to_string();
        assert!(cg.module.verify().is_ok(), "let-bound handler route failed to verify:\n{ir}");
        assert!(
            ir.contains("call void @tyra_http_server_route"),
            "let-bound handler path must emit the route call, not fall back:\n{ir}"
        );
        assert!(
            ir.contains("ptr @my_handler)"),
            "the let-bound handler must resolve to the @my_handler global pointer:\n{ir}"
        );
    }
}
