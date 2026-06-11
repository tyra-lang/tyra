//! MIR instruction emission: scalars, control flow, memory, ADT, and builtins.
//!
//! Each `BasicValueEnum` carries its own LLVM type, so width selection for
//! `icmp`/`fcmp` and similar type-driven operations is automatic from the
//! operand handles. Functions whose bodies contain unsupported instructions
//! fall back to a single `unreachable` block so the module always verifies.

use std::collections::HashSet;

use inkwell::types::BasicTypeEnum;
use inkwell::values::{
    AggregateValueEnum, BasicMetadataValueEnum, BasicValueEnum, FunctionValue, PhiValue,
};
use inkwell::{FloatPredicate, IntPredicate};

use tyra_mir::{Constant, Function, Instruction, MirBinOp, Operand, Program};

use crate::inkwell_codegen::CodeGen;

impl<'ctx> CodeGen<'ctx> {
    /// I2: emit a body for every function. Fully-supported functions get real
    /// instructions; the rest get a single `unreachable` block (so the module
    /// verifies while instruction coverage grows phase by phase).
    pub(crate) fn emit_bodies(&mut self, program: &Program) {
        for (fi, f) in program.functions.iter().enumerate() {
            // I4c: per-function type scan (operand Tyra types for `print`
            // routing + the emittability gate's printability check). Computed
            // into a local first so the immutable borrows of struct_map/fn_sigs
            // end before the mutable store into self.scan.
            let scan = crate::type_scan::scan_function_types(f, &self.struct_map, &self.fn_sigs);
            self.scan = Some(scan);
            // I4i: reset the spawn-thunk id cursor to this function's base, so
            // each `Spawn` site references the id pre-assigned by program order
            // (robust against earlier functions that fell back to `unreachable`).
            self.spawn_cursor = self.spawn_bases.get(fi).copied().unwrap_or(0);
            // I6: clear any debug location left by the previous function so this
            // function's instructions never inherit an out-of-scope `!dbg`. An
            // emittable body resets it to its own subprogram below; a fallback
            // `unreachable` needs none.
            self.clear_debug_line();
            if self.is_i2_emittable(f) {
                self.emit_function_body(f);
            } else {
                let fv = self.fn_values[&f.name];
                let entry = self.ctx.append_basic_block(fv, "entry");
                self.builder.position_at_end(entry);
                self.builder.build_unreachable().unwrap();
            }
        }
    }

    /// Conservative static check: a function is I2a-emittable iff every
    /// instruction is in the supported set, every (non-phi) operand references
    /// a name defined earlier in body order (so it resolves at emission time),
    /// and every Phi has a first incoming that is a constant or already-defined
    /// value (so the phi's type is determinable when it is built).
    fn is_i2_emittable(&self, f: &Function) -> bool {
        // Body must end in a terminator so every block is terminated (real MIR
        // functions end with Return). An empty / non-terminated body (e.g. a
        // hand-built test stub) is not emittable and falls back to unreachable.
        match f.body.last().map(|s| &s.instr) {
            Some(
                Instruction::Return { .. }
                | Instruction::Jump { .. }
                | Instruction::BranchIf { .. },
            ) => {}
            _ => return false,
        }
        let mut seen: HashSet<&str> = HashSet::new();
        for (name, _) in &f.params {
            seen.insert(name.as_str());
        }
        for stmt in &f.body {
            let inst = &stmt.instr;
            // 1. supported instruction?
            let supported = match inst {
                Instruction::Const { .. }
                | Instruction::BinOp { .. }
                | Instruction::Neg { .. }
                | Instruction::Not { .. }
                | Instruction::Copy { .. }
                | Instruction::Return { .. }
                | Instruction::Label(_)
                | Instruction::Jump { .. }
                | Instruction::BranchIf { .. }
                | Instruction::Phi { .. }
                | Instruction::Alloca { .. }
                | Instruction::Store { .. }
                | Instruction::Load { .. }
                | Instruction::PtrLoad { .. }
                | Instruction::StructInit { .. }
                | Instruction::FieldGet { .. }
                | Instruction::FieldSet { .. }
                | Instruction::AdtInit { .. }
                | Instruction::AdtTag { .. }
                | Instruction::AdtPayload { .. }
                | Instruction::StringFormat { .. }
                | Instruction::ListInit { .. }
                | Instruction::ListLen { .. }
                | Instruction::ListGet { .. }
                | Instruction::ListGetSafe { .. }
                | Instruction::ListPush { .. }
                | Instruction::MapGetOption { .. }
                | Instruction::LinkedMapGetOption { .. }
                | Instruction::MapForEachCall { .. }
                | Instruction::SetForEachCall { .. }
                | Instruction::LinkedMapForEachCall { .. }
                | Instruction::LinkedSetForEachCall { .. }
                | Instruction::SortedMapGetOption { .. }
                | Instruction::SortedMapForEachCall { .. }
                | Instruction::SortedSetForEachCall { .. }
                | Instruction::IndirectCall { .. }
                // I4i concurrency: Await/JoinAll/Select dispatch to runtime
                // externs; their handle/list operands are definedness-checked
                // below. Spawn additionally needs the target function declared.
                | Instruction::Await { .. }
                | Instruction::JoinAll { .. }
                | Instruction::Select { .. } => true,
                Instruction::Spawn { func, .. } => self.fn_values.contains_key(func),
                // A closure can only be built for a function we have a value
                // for (mirrors the user-Call admission below).
                Instruction::ClosureBuild { fn_name, .. } => self.fn_values.contains_key(fn_name),
                Instruction::Call { func, args, .. } => {
                    self.fn_values.contains_key(func)
                        || self.module.get_function(func).is_some()
                        || (Self::is_supported_builtin(func)
                            && self.builtin_args_emittable(f, func, args))
                }
                // As of I4i the arms above cover every `Instruction` variant, so
                // this is currently unreachable — kept as a defensive default
                // (reject → `unreachable` fallback) for when tyra-mir adds a new
                // variant cross-crate.
                #[allow(unreachable_patterns)]
                _ => false,
            };
            if !supported {
                return false;
            }
            // 2. operand definedness.
            match inst {
                Instruction::Phi { branches, .. } => {
                    // First incoming must be resolvable at build time (for the
                    // phi type); later incomings may be defined anywhere.
                    if let Some((first, _)) = branches.first()
                        && !self.operand_resolvable_now(first, &seen)
                    {
                        return false;
                    }
                }
                _ => {
                    if !self.operands_of(inst).iter().all(|op| match op {
                        // A top-level function name is always resolvable (its
                        // global pointer exists regardless of local flow), so it
                        // counts as defined even without a `seen` entry — this is
                        // how a bare handler identifier (http route) is admitted.
                        Operand::Var(n) => {
                            seen.contains(n.as_str()) || self.fn_values.contains_key(n.as_str())
                        }
                        Operand::Const(_) => true,
                    }) {
                        return false;
                    }
                    // Slot/source names referenced by name (not Operand) must
                    // already be defined (an Alloca dest or a param).
                    let name_ref: Option<&str> = match inst {
                        Instruction::Copy { source, .. } => Some(source),
                        Instruction::Store { dest, .. } => Some(dest),
                        Instruction::Load { source, .. } => Some(source),
                        Instruction::PtrLoad { ptr, .. } => Some(ptr),
                        _ => None,
                    };
                    if let Some(n) = name_ref {
                        // `Copy::source` may name a top-level function (a
                        // `let`-bound function reference, resolved to its global
                        // pointer); `Store`/`Load`/`PtrLoad` names are always
                        // slots/pointers, so they still require a `seen` entry.
                        let is_copy_fn_ref = matches!(inst, Instruction::Copy { .. })
                            && self.fn_values.contains_key(n);
                        if !seen.contains(n) && !is_copy_fn_ref {
                            return false;
                        }
                    }
                }
            }
            // 3. record this instruction's definition.
            if let Some(dest) = instr_dest(inst) {
                seen.insert(dest);
            }
        }
        true
    }

    /// Gate check for a supported builtin's arguments. `print`/`println`/… can
    /// only be emitted for printable scalar/String values; a struct-valued arg
    /// (List/Option/closure/value-struct) has no printf form, so the function
    /// must fall back to `unreachable` rather than reach `emit_print` (which
    /// would otherwise panic coercing a struct to int). Non-print builtins place
    /// no constraint here.
    fn builtin_args_emittable(&self, f: &Function, func: &str, args: &[Operand]) -> bool {
        if Self::is_print_builtin(func) {
            return args.iter().all(|a| !self.operand_is_struct(f, a));
        }
        true
    }

    /// Does the operand hold an LLVM struct value (vs a scalar/ptr)? A struct
    /// temp per the type scan, or a struct-typed parameter.
    fn operand_is_struct(&self, f: &Function, op: &Operand) -> bool {
        let Operand::Var(name) = op else { return false };
        if self
            .scan
            .as_ref()
            .is_some_and(|s| s.struct_temps.contains_key(name))
        {
            return true;
        }
        f.params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, ty)| self.ty_to_basic_type(ty).is_struct_type())
            .unwrap_or(false)
    }

    fn operand_resolvable_now(&self, op: &Operand, seen: &HashSet<&str>) -> bool {
        match op {
            Operand::Const(_) => true,
            Operand::Var(n) => seen.contains(n.as_str()) || self.fn_values.contains_key(n.as_str()),
        }
    }

    /// Operands directly referenced by an instruction (excludes Copy::source,
    /// which is a `String`, not an `Operand` — handled by the caller).
    fn operands_of<'a>(&self, inst: &'a Instruction) -> Vec<&'a Operand> {
        match inst {
            Instruction::BinOp { lhs, rhs, .. } => vec![lhs, rhs],
            Instruction::Neg { operand, .. } | Instruction::Not { operand, .. } => vec![operand],
            Instruction::Return { value: Some(v) } => vec![v],
            Instruction::BranchIf { cond, .. } => vec![cond],
            Instruction::Store { value, .. } => vec![value],
            Instruction::Call { args, .. } => args.iter().collect(),
            Instruction::StructInit { fields, .. } => fields.iter().collect(),
            Instruction::FieldGet { obj, .. } => vec![obj],
            Instruction::FieldSet { obj, value, .. } => vec![obj, value],
            Instruction::AdtInit { fields, .. } => fields.iter().collect(),
            Instruction::AdtTag { obj, .. } | Instruction::AdtPayload { obj, .. } => vec![obj],
            Instruction::StringFormat { args, .. } => args.iter().collect(),
            Instruction::ListInit { elements, .. } => elements.iter().collect(),
            Instruction::ListLen { list, .. } => vec![list],
            Instruction::ListGet { list, index, .. }
            | Instruction::ListGetSafe { list, index, .. } => {
                vec![list, index]
            }
            Instruction::ListPush { list, elem, .. } => vec![list, elem],
            Instruction::MapGetOption { handle, key, .. }
            | Instruction::LinkedMapGetOption { handle, key, .. }
            | Instruction::SortedMapGetOption { handle, key, .. } => vec![handle, key],
            Instruction::MapForEachCall { handle, fat_ptr }
            | Instruction::SetForEachCall { handle, fat_ptr }
            | Instruction::LinkedMapForEachCall { handle, fat_ptr }
            | Instruction::LinkedSetForEachCall { handle, fat_ptr }
            | Instruction::SortedMapForEachCall { handle, fat_ptr }
            | Instruction::SortedSetForEachCall { handle, fat_ptr } => vec![handle, fat_ptr],
            // I4i concurrency. Spawn's `func` is a top-level function (resolvable
            // via fn_values, like ClosureBuild), so only its args are operands.
            Instruction::Spawn { args, .. } => args.iter().collect(),
            Instruction::Await { task, .. } => vec![task],
            Instruction::JoinAll { list, .. } | Instruction::Select { list, .. } => vec![list],
            // ClosureBuild's `fn_name` is a top-level function (always
            // resolvable), so only the captured env fields are operands.
            Instruction::ClosureBuild { env_fields, .. } => env_fields.iter().collect(),
            Instruction::IndirectCall { fat_ptr, args, .. } => {
                let mut v = vec![fat_ptr];
                v.extend(args.iter());
                v
            }
            _ => vec![],
        }
    }

    fn emit_function_body(&mut self, f: &Function) {
        self.values.clear();
        self.blocks.clear();
        self.pred_blocks.clear();
        self.addr_slots.clear();
        self.slot_types.clear();
        self.cur_label = None;
        let fv = self.fn_values[&f.name];

        // Entry block, then pre-create every labeled block so forward jumps and
        // phi predecessors resolve regardless of emission order.
        let entry = self.ctx.append_basic_block(fv, "tyra.entry");
        for stmt in &f.body {
            if let Instruction::Label(name) = &stmt.instr {
                let bb = self.ctx.append_basic_block(fv, name);
                self.blocks.insert(name.clone(), bb);
            }
        }
        self.builder.position_at_end(entry);

        // I6: this function's subprogram (None without debug info). Set an
        // entry-line location now so every prologue instruction (param allocas,
        // main init, alloca hoists) carries an in-scope `!dbg`; the body loop
        // refines it per source statement.
        let sp = self.di_subprogram(&f.name);
        if let Some(sp) = sp {
            let entry_line = f
                .body
                .iter()
                .find(|s| !s.loc.is_dummy())
                .map(|s| s.loc.line)
                .unwrap_or(1);
            self.set_debug_line(sp, entry_line);
        }

        // Parameters: bind the SSA arg for direct (immutable) operand refs and
        // also create a `.addr` alloca so mutation (Store) and `Copy`-from-param
        // read the mutable view.
        // IMPORTANT: use the parameter's actual LLVM type for the alloca, not
        // a fixed `alloca i64`.  Struct-typed params (Option, Result, List,
        // collection wrappers, ADT values) are wider than 8 bytes; allocating
        // only 8 bytes then storing the full struct overflows into adjacent
        // stack slots and silently corrupts later loads.
        if !f.is_main {
            for (i, (name, ty)) in f.params.iter().enumerate() {
                let p = fv.get_nth_param(i as u32).unwrap();
                p.set_name(name);
                self.values.insert(name.clone(), p);
                let bt = self.ty_to_basic_type(ty);
                let slot = self
                    .builder
                    .build_alloca(bt, &format!("{name}.addr"))
                    .unwrap();
                self.builder.build_store(slot, p).unwrap();
                self.addr_slots.insert(name.clone(), slot);
                self.slot_types.insert(name.clone(), bt);
            }
        }

        // Hoist every local alloca slot to the entry block (allocated once, not
        // per loop iteration). Use the type from the scan's alloca_llvm_types
        // (derived from the first Store into each slot) so struct-typed locals
        // (Option/Result/List/Map wrappers, ADT values) get correctly-sized
        // slots. Scalar/unknown locals fall back to i64.
        let alloca_type_strs: std::collections::HashMap<String, String> = self
            .scan
            .as_ref()
            .map(|s| s.alloca_llvm_types.clone())
            .unwrap_or_default();
        let i64t = self.ctx.i64_type();
        for stmt in &f.body {
            if let Instruction::Alloca { dest } = &stmt.instr
                && !self.addr_slots.contains_key(dest)
            {
                let bt = alloca_type_strs
                    .get(dest.as_str())
                    .map(|s| self.basic_type_from_scan_str(s))
                    .unwrap_or_else(|| i64t.into());
                let slot = self.builder.build_alloca(bt, dest).unwrap();
                self.addr_slots.insert(dest.clone(), slot);
                self.slot_types.insert(dest.clone(), bt);
            }
        }

        if f.is_main {
            // C entry: i32 @main(i32 %argc, ptr %argv). Initialize GC + runtime
            // and capture argc/argv for sys.args() (ADR-0007 / sys.args).
            self.call_runtime_void("GC_init", &[]);
            self.call_runtime_void("tyra_rt_init", &[]);
            let argc = fv.get_nth_param(0).unwrap();
            let argv = fv.get_nth_param(1).unwrap();
            argc.set_name("argc");
            argv.set_name("argv");
            let argc_g = self
                .module
                .get_global(".tyra.argc")
                .unwrap()
                .as_pointer_value();
            self.builder.build_store(argc_g, argc).unwrap();
            let argv_g = self
                .module
                .get_global(".tyra.argv")
                .unwrap()
                .as_pointer_value();
            self.builder.build_store(argv_g, argv).unwrap();
            // I5: register the counter array with the runtime atexit flusher
            // before any user code runs (no-op without coverage).
            self.emit_cov_init_call();
        }

        // I5: the function's entry-block counter, in `tyra.entry` after init /
        // alloca hoisting and before the first real instruction (no-op without
        // coverage). Mirrors the legacy entry increment.
        self.emit_cov_entry(f);

        // I6b: bind each named local's alloca slot to its DILocalVariable via
        // llvm.dbg.declare, appended to the entry block after alloca hoisting
        // (no-op without debug info).
        if let Some(sp) = sp {
            self.emit_local_var_decls(f, sp, entry);
        }

        let mut pending: Vec<(PhiValue<'ctx>, Vec<(Operand, String)>)> = Vec::new();
        for (si, stmt) in f.body.iter().enumerate() {
            // Dead code after an in-block terminator: `panic`/`sys.exit` emit
            // `unreachable` mid-block, so the lowering-appended trailing `Return`
            // (lower/mod.rs always closes a body with one) is unreachable. A
            // genuine terminator (Jump/BranchIf/Return) is always followed by a
            // `Label` which repositions the builder, so only such dead tails are
            // skipped here — existing emittable functions are unaffected.
            if !matches!(stmt.instr, Instruction::Label(_))
                && self
                    .builder
                    .get_insert_block()
                    .and_then(|b| b.get_terminator())
                    .is_some()
            {
                continue;
            }
            self.cur_loc = stmt.loc;
            // I6: refine the debug location to this statement's line (column 1),
            // so the instructions it emits get its `!dbg`. A dummy-loc stmt
            // keeps the previous in-scope location (still valid).
            if let Some(sp) = sp
                && !stmt.loc.is_dummy()
            {
                self.set_debug_line(sp, stmt.loc.line);
            }
            self.emit_instr(&stmt.instr, f, &mut pending);
            // I5: a labeled basic block gets its own counter, incremented right
            // after the builder repositions onto it. Use the label's loc, or the
            // next non-dummy stmt's loc when the label itself is synthetic
            // (no-op without coverage). Mirrors the legacy per-label increment.
            if matches!(stmt.instr, Instruction::Label(_)) {
                let loc = if !stmt.loc.is_dummy() {
                    stmt.loc
                } else {
                    f.body[si + 1..]
                        .iter()
                        .find(|s| !s.loc.is_dummy())
                        .map(|s| s.loc)
                        .unwrap_or_else(tyra_mir::SourceLoc::dummy)
                };
                self.emit_cov_increment(loc);
            }
            // Track the *exit* block of the current label (for phi predecessors)
            // without touching `blocks` (the jump-*target* table). For a
            // non-splitting instruction this is idempotent; for ListGet/
            // ListGetSafe (which branch mid-instruction) it advances to the
            // actual block the region's terminator will branch from.
            if let Some(label) = &self.cur_label
                && let Some(bb) = self.builder.get_insert_block()
            {
                self.pred_blocks.insert(label.clone(), bb);
            }
        }

        // Resolve phi incomings now that every value and block exists. Use the
        // label's *exit* block (`pred_blocks`) — the real predecessor — falling
        // back to the entry block for any label whose region was never split.
        for (phi, branches) in pending {
            for (op, label) in &branches {
                let v = self.operand(op);
                let bb = self
                    .pred_blocks
                    .get(label)
                    .copied()
                    .unwrap_or_else(|| self.blocks[label]);
                phi.add_incoming(&[(&v, bb)]);
            }
        }
    }

    fn emit_instr(
        &mut self,
        inst: &Instruction,
        f: &Function,
        pending: &mut Vec<(PhiValue<'ctx>, Vec<(Operand, String)>)>,
    ) {
        match inst {
            Instruction::Const { dest, value } => {
                let v = self.const_value(value);
                self.values.insert(dest.clone(), v);
            }
            Instruction::Copy { dest, source } => {
                // From a param: load the mutable `.addr` view (matches legacy,
                // so post-mutation reads see the new value). From an SSA temp:
                // alias the value handle.
                let v = if is_param(f, source) {
                    let ty = self.slot_types[source];
                    let slot = self.addr_slots[source];
                    self.builder.build_load(ty, slot, dest).unwrap()
                } else {
                    // An SSA temp, or a `let`-bound top-level function reference
                    // (`let h = my_handler`) that resolves to the fn global ptr.
                    self.value_by_name(source)
                };
                self.values.insert(dest.clone(), v);
            }
            // Slots are pre-allocated and hoisted to the entry block.
            Instruction::Alloca { .. } => {}
            Instruction::Store { dest, value } => {
                let v = self.operand(value);
                let slot = self.addr_slots[dest];
                self.builder.build_store(slot, v).unwrap();
                // Refine the slot's load type to the stored value's type. LLVM
                // requires store/load type annotations to match the value, not
                // the (i64) alloca declaration (opaque pointers).
                self.slot_types.insert(dest.clone(), v.get_type());
            }
            Instruction::Load { dest, source } => {
                let ty = self
                    .slot_types
                    .get(source)
                    .copied()
                    .unwrap_or_else(|| self.ctx.i64_type().into());
                let slot = self.addr_slots[source];
                let v = self.builder.build_load(ty, slot, dest).unwrap();
                self.values.insert(dest.clone(), v);
            }
            Instruction::PtrLoad { dest, ptr, ty } => {
                let bt = self.ty_to_basic_type(ty);
                let p = self.values[ptr].into_pointer_value();
                let v = self.builder.build_load(bt, p, dest).unwrap();
                self.values.insert(dest.clone(), v);
            }
            Instruction::StructInit {
                dest,
                type_name,
                fields,
            } => {
                let st = self.struct_types[type_name];
                if self.data_types.contains(type_name) {
                    // data type (§8.6): heap-allocate, then store each field.
                    let size = st.size_of().expect("data struct must be sized");
                    let gc = self.module.get_function("GC_malloc").unwrap();
                    let raw = self
                        .builder
                        .build_call(gc, &[size.into()], dest)
                        .unwrap()
                        .try_as_basic_value()
                        .basic()
                        .unwrap()
                        .into_pointer_value();
                    for (i, fop) in fields.iter().enumerate() {
                        let v = self.operand(fop);
                        let gep = self
                            .builder
                            .build_struct_gep(st, raw, i as u32, &format!("{dest}.f{i}"))
                            .unwrap();
                        self.builder.build_store(gep, v).unwrap();
                    }
                    self.values.insert(dest.clone(), raw.into());
                } else {
                    // value type: insertvalue chain from undef.
                    // The last insertvalue is named `dest` so extractvalue/BinOp
                    // references use the same name as the MIR temp (%_t2 not %_t2.s1).
                    let n = fields.len();
                    let mut agg: AggregateValueEnum = st.get_undef().into();
                    for (i, fop) in fields.iter().enumerate() {
                        let v = self.operand(fop);
                        let name = if i + 1 == n {
                            dest.as_str()
                        } else {
                            &format!("{dest}.s{i}")
                        };
                        agg = self
                            .builder
                            .build_insert_value(agg, v, i as u32, name)
                            .unwrap();
                    }
                    self.values
                        .insert(dest.clone(), agg.into_struct_value().into());
                }
            }
            Instruction::FieldGet {
                dest,
                obj,
                type_name,
                field_index,
            } => {
                let st = self.struct_types[type_name];
                let o = self.operand(obj);
                if self.data_types.contains(type_name) {
                    let ptr = o.into_pointer_value();
                    let gep = self
                        .builder
                        .build_struct_gep(st, ptr, *field_index, &format!("{dest}.gep"))
                        .unwrap();
                    let fty = st.get_field_type_at_index(*field_index).unwrap();
                    let v = self.builder.build_load(fty, gep, dest).unwrap();
                    self.values.insert(dest.clone(), v);
                } else {
                    let v = self
                        .builder
                        .build_extract_value(o.into_struct_value(), *field_index, dest)
                        .unwrap();
                    self.values.insert(dest.clone(), v);
                }
            }
            Instruction::FieldSet {
                obj,
                type_name,
                field_index,
                value,
            } => {
                // In-place data-type field mutation (§8.6): GEP + store.
                let st = self.struct_types[type_name];
                let ptr = self.operand(obj).into_pointer_value();
                let v = self.operand(value);
                let gep = self
                    .builder
                    .build_struct_gep(st, ptr, *field_index, "fset")
                    .unwrap();
                self.builder.build_store(gep, v).unwrap();
            }
            Instruction::AdtInit {
                dest,
                type_name,
                tag,
                fields,
            } => {
                let st = self.struct_types[type_name];
                let recursive = self
                    .recursive_fields
                    .get(type_name)
                    .cloned()
                    .unwrap_or_default();
                let num_fields = st.count_fields() as usize;
                // Field 0 is the i8 tag.
                let mut agg: AggregateValueEnum = st.get_undef().into();
                let tag_v = self.ctx.i8_type().const_int(*tag as u64, false);
                agg = self
                    .builder
                    .build_insert_value(agg, tag_v, 0, "adt.tag")
                    .unwrap();
                // Payload fields: AdtInit.fields excludes the tag, so field
                // struct-index `fi` maps to fields[fi - 1].
                for fi in 1..num_fields {
                    let fty = st.get_field_type_at_index(fi as u32).unwrap();
                    let is_rec = recursive.get(fi).copied().unwrap_or(false);
                    let field_op = fields.get(fi - 1);
                    let v: BasicValueEnum = if is_rec {
                        // Recursive self-reference: boxed GC-heap ptr. A zero
                        // placeholder (inactive variant) becomes null.
                        match field_op {
                            Some(Operand::Const(Constant::Int(0))) | None => {
                                self.ptr().const_null().into()
                            }
                            Some(op) => {
                                let inner = self.operand(op);
                                let size = st.size_of().expect("sized ADT");
                                let gc = self.module.get_function("GC_malloc").unwrap();
                                let box_ptr = self
                                    .builder
                                    .build_call(gc, &[size.into()], "adt.box")
                                    .unwrap()
                                    .try_as_basic_value()
                                    .basic()
                                    .unwrap()
                                    .into_pointer_value();
                                self.builder.build_store(box_ptr, inner).unwrap();
                                box_ptr.into()
                            }
                        }
                    } else {
                        match field_op {
                            // MIR fills inactive variant fields with Int(0)
                            // regardless of the field's real type (incl. i1
                            // Bool, i8, ptr, double, struct). The value-handle
                            // backend keeps the real type, so always materialize
                            // the field-typed zero — this also equals the real
                            // value for an active integer 0.
                            Some(Operand::Const(Constant::Int(0))) => self.zero_of(fty),
                            Some(op) => self.operand(op),
                            None => self.zero_of(fty),
                        }
                    };
                    agg = self
                        .builder
                        .build_insert_value(agg, v, fi as u32, &format!("adt.f{fi}"))
                        .unwrap();
                }
                self.values
                    .insert(dest.clone(), agg.into_struct_value().into());
            }
            Instruction::AdtTag { dest, obj, .. } => {
                let o = self.operand(obj).into_struct_value();
                let tag_i8 = self
                    .builder
                    .build_extract_value(o, 0, &format!("{dest}.i8"))
                    .unwrap()
                    .into_int_value();
                let v = self
                    .builder
                    .build_int_z_extend(tag_i8, self.ctx.i64_type(), dest)
                    .unwrap();
                self.values.insert(dest.clone(), v.into());
            }
            Instruction::AdtPayload {
                dest,
                obj,
                type_name,
                field_index,
            } => {
                let st = self.struct_types[type_name];
                let o = self.operand(obj).into_struct_value();
                let extracted = self
                    .builder
                    .build_extract_value(o, *field_index, dest)
                    .unwrap();
                let idx = *field_index as usize;
                let is_rec = self
                    .recursive_fields
                    .get(type_name)
                    .and_then(|r| r.get(idx).copied())
                    .unwrap_or(false);
                if is_rec {
                    // Boxed self-reference: load the referenced ADT struct back.
                    let v = self
                        .builder
                        .build_load(st, extracted.into_pointer_value(), dest)
                        .unwrap();
                    self.values.insert(dest.clone(), v);
                } else {
                    self.values.insert(dest.clone(), extracted);
                }
            }
            Instruction::BinOp { dest, op, lhs, rhs } => {
                // EqInt/NeqInt with struct operands: field-by-field ADT comparison.
                if matches!(op, MirBinOp::EqInt | MirBinOp::NeqInt) {
                    let stype = self
                        .scan
                        .as_ref()
                        .and_then(|s| {
                            let ln = if let Operand::Var(n) = lhs {
                                s.struct_temps.get(n.as_str())
                            } else {
                                None
                            };
                            let rn = if let Operand::Var(n) = rhs {
                                s.struct_temps.get(n.as_str())
                            } else {
                                None
                            };
                            ln.or(rn)
                        })
                        .cloned();
                    if let Some(stype) = stype {
                        let fv = self
                            .builder
                            .get_insert_block()
                            .unwrap()
                            .get_parent()
                            .unwrap();
                        let v = self.emit_adt_compare(*op, lhs, rhs, dest, &stype, fv);
                        self.values.insert(dest.clone(), v);
                        return;
                    }
                }
                let v = self.emit_binop(*op, lhs, rhs, dest);
                self.values.insert(dest.clone(), v);
            }
            Instruction::Neg { dest, operand } => {
                let o = self.operand(operand);
                let v: BasicValueEnum = if o.is_float_value() {
                    self.builder
                        .build_float_neg(o.into_float_value(), dest)
                        .unwrap()
                        .into()
                } else {
                    self.builder
                        .build_int_neg(o.into_int_value(), dest)
                        .unwrap()
                        .into()
                };
                self.values.insert(dest.clone(), v);
            }
            Instruction::Not { dest, operand } => {
                let o = self.operand(operand).into_int_value();
                let v = self.builder.build_not(o, dest).unwrap();
                self.values.insert(dest.clone(), v.into());
            }
            Instruction::Return { value } => {
                if f.is_main {
                    let zero = self.ctx.i32_type().const_zero();
                    self.builder.build_return(Some(&zero)).unwrap();
                } else {
                    match value {
                        Some(v) if !is_void_ret(f) => {
                            let rv = self.operand(v);
                            self.builder.build_return(Some(&rv)).unwrap();
                        }
                        _ => {
                            self.builder.build_return(None).unwrap();
                        }
                    }
                }
            }
            Instruction::Label(name) => {
                self.builder.position_at_end(self.blocks[name]);
                self.cur_label = Some(name.clone());
            }
            Instruction::Jump { label } => {
                self.builder
                    .build_unconditional_branch(self.blocks[label])
                    .unwrap();
            }
            Instruction::BranchIf {
                cond,
                true_label,
                false_label,
            } => {
                let c = self.operand(cond).into_int_value();
                let t = self.blocks[true_label];
                let e = self.blocks[false_label];
                self.builder.build_conditional_branch(c, t, e).unwrap();
            }
            Instruction::Phi { dest, branches } => {
                let first = self.operand(&branches[0].0);
                let phi = self.builder.build_phi(first.get_type(), dest).unwrap();
                self.values.insert(dest.clone(), phi.as_basic_value());
                pending.push((phi, branches.clone()));
            }
            Instruction::Call { dest, func, args } => {
                let callee_opt = self
                    .fn_values
                    .get(func)
                    .copied()
                    .or_else(|| self.module.get_function(func));
                if let Some(callee) = callee_opt {
                    let argvals: Vec<BasicMetadataValueEnum<'ctx>> =
                        args.iter().map(|a| self.operand(a).into()).collect();
                    let cs = self
                        .builder
                        .build_call(callee, &argvals, dest.as_deref().unwrap_or(""))
                        .unwrap();
                    if let Some(d) = dest
                        && let Some(rv) = cs.try_as_basic_value().basic()
                    {
                        self.values.insert(d.clone(), rv);
                    }
                } else {
                    // Builtin (the gate admitted only supported ones, I4a+).
                    let handled = self.emit_builtin(dest, func, args);
                    debug_assert!(handled, "gate admitted unsupported builtin `{func}`");
                }
            }
            Instruction::ListInit {
                dest,
                elem_type,
                elements,
            } => {
                self.emit_list_init(dest, elem_type, elements);
            }
            Instruction::ListLen { dest, list } => {
                self.emit_list_len(dest, list);
            }
            Instruction::ListGet {
                dest,
                list,
                index,
                elem_type,
            } => {
                self.emit_list_get(dest, list, index, elem_type);
            }
            Instruction::ListGetSafe {
                dest,
                list,
                index,
                elem_type,
            } => {
                self.emit_list_get_safe(dest, list, index, elem_type);
            }
            Instruction::ListPush {
                dest,
                list,
                elem,
                elem_type,
            } => {
                self.emit_list_push(dest, list, elem, elem_type);
            }
            Instruction::MapGetOption {
                dest,
                handle,
                key,
                key_ty,
                val_ty,
            } => {
                self.emit_map_get_option(dest, handle, key, key_ty, val_ty, "tyra_map_get");
            }
            Instruction::LinkedMapGetOption {
                dest,
                handle,
                key,
                key_ty,
                val_ty,
            } => {
                self.emit_map_get_option(dest, handle, key, key_ty, val_ty, "tyra_linked_map_get");
            }
            Instruction::ClosureBuild {
                dest,
                fn_name,
                env_fields,
                env_struct_name,
                ..
            } => {
                self.emit_closure_build(dest, fn_name, env_fields, env_struct_name);
            }
            Instruction::IndirectCall {
                dest,
                fat_ptr,
                args,
                param_types,
                return_type,
            } => {
                self.emit_indirect_call(dest, fat_ptr, args, param_types, return_type);
            }
            Instruction::MapForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_map_for_each", "__mfe");
            }
            Instruction::SetForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_set_for_each", "__sfe");
            }
            Instruction::LinkedMapForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_linked_map_for_each", "__lmfe");
            }
            Instruction::LinkedSetForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_linked_set_for_each", "__lsfe");
            }
            Instruction::SortedMapGetOption {
                dest,
                handle,
                key,
                key_ty,
                val_ty,
            } => {
                self.emit_map_get_option(
                    dest,
                    handle,
                    key,
                    key_ty,
                    val_ty,
                    "tyra_sorted_map_get",
                );
            }
            Instruction::SortedMapForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_sorted_map_for_each", "__smfe");
            }
            Instruction::SortedSetForEachCall { handle, fat_ptr } => {
                self.emit_for_each(handle, fat_ptr, "tyra_sorted_set_for_each", "__ssfe");
            }
            Instruction::Spawn {
                dest,
                func,
                args,
                arg_types,
                result_type,
            } => {
                self.emit_spawn(dest, func, args, arg_types, result_type);
            }
            Instruction::Await {
                dest,
                task,
                result_type,
            } => {
                self.emit_await(dest, task, result_type);
            }
            Instruction::JoinAll {
                dest,
                list,
                elem_type,
            } => {
                self.emit_join_all(dest, list, elem_type);
            }
            Instruction::Select {
                dest,
                list,
                elem_type,
            } => {
                self.emit_select(dest, list, elem_type);
            }
            Instruction::StringFormat {
                dest,
                format_ref,
                args,
            } => {
                // GC-allocate a 1024-byte buffer and snprintf into it. No
                // null-check branch: Boehm GC_malloc never returns null (its OOM
                // handler aborts internally), and adding the branch would split
                // the basic block and break phi-predecessor bookkeeping.
                let i64t = self.ctx.i64_type();
                let size = i64t.const_int(1024, false);
                let gc = self.module.get_function("GC_malloc").unwrap();
                let buf = self
                    .builder
                    .build_call(gc, &[size.into()], dest)
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap()
                    .into_pointer_value();
                let fmt = self
                    .module
                    .get_global(&format!(".str.{format_ref}"))
                    .expect("format string global (I1)")
                    .as_pointer_value();
                let mut call_args: Vec<BasicMetadataValueEnum<'ctx>> =
                    vec![buf.into(), size.into(), fmt.into()];
                for arg in args {
                    let v = self.operand(arg);
                    // i1 (bool) must be widened to i64 for printf-family varargs.
                    if v.is_int_value() && v.into_int_value().get_type().get_bit_width() == 1 {
                        let w = self
                            .builder
                            .build_int_z_extend(v.into_int_value(), i64t, "fmt.b")
                            .unwrap();
                        call_args.push(w.into());
                    } else {
                        call_args.push(v.into());
                    }
                }
                let snprintf = self.module.get_function("snprintf").unwrap();
                self.builder
                    .build_call(snprintf, &call_args, "fmt")
                    .unwrap();
                self.values.insert(dest.clone(), buf.into());
            }
            // As of I4i every `Instruction` variant has a dispatch arm above, so
            // this is currently unreachable. Kept as the gate↔dispatch coherence
            // guard (and the cross-crate safety net for a future tyra-mir
            // variant): the gate must never admit an instruction this can't emit.
            #[allow(unreachable_patterns)]
            other => unreachable!("emit_instr called on unsupported instruction: {other:?}"),
        }
    }

    fn emit_binop(
        &self,
        op: MirBinOp,
        lhs: &Operand,
        rhs: &Operand,
        dest: &str,
    ) -> BasicValueEnum<'ctx> {
        let l = self.operand(lhs);
        let r = self.operand(rhs);
        let b = &self.builder;
        match op {
            MirBinOp::AddInt => b
                .build_int_add(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::SubInt => b
                .build_int_sub(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::MulInt => b
                .build_int_mul(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::DivInt => b
                .build_int_signed_div(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::RemInt => b
                .build_int_signed_rem(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::AddFloat => b
                .build_float_add(l.into_float_value(), r.into_float_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::SubFloat => b
                .build_float_sub(l.into_float_value(), r.into_float_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::MulFloat => b
                .build_float_mul(l.into_float_value(), r.into_float_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::DivFloat => b
                .build_float_div(l.into_float_value(), r.into_float_value(), dest)
                .unwrap()
                .into(),
            // Width (i1 vs i64) is taken from the operand handles automatically.
            MirBinOp::EqInt => b
                .build_int_compare(
                    IntPredicate::EQ,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::NeqInt => b
                .build_int_compare(
                    IntPredicate::NE,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::LtInt => b
                .build_int_compare(
                    IntPredicate::SLT,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::LeInt => b
                .build_int_compare(
                    IntPredicate::SLE,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::GtInt => b
                .build_int_compare(
                    IntPredicate::SGT,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::GeInt => b
                .build_int_compare(
                    IntPredicate::SGE,
                    l.into_int_value(),
                    r.into_int_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::LtFloat => b
                .build_float_compare(
                    FloatPredicate::OLT,
                    l.into_float_value(),
                    r.into_float_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::LeFloat => b
                .build_float_compare(
                    FloatPredicate::OLE,
                    l.into_float_value(),
                    r.into_float_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::GtFloat => b
                .build_float_compare(
                    FloatPredicate::OGT,
                    l.into_float_value(),
                    r.into_float_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::GeFloat => b
                .build_float_compare(
                    FloatPredicate::OGE,
                    l.into_float_value(),
                    r.into_float_value(),
                    dest,
                )
                .unwrap()
                .into(),
            MirBinOp::And => b
                .build_and(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::Or => b
                .build_or(l.into_int_value(), r.into_int_value(), dest)
                .unwrap()
                .into(),
            MirBinOp::EqString | MirBinOp::NeqString => {
                let strcmp = self.module.get_function("strcmp").unwrap();
                let cs = b.build_call(strcmp, &[l.into(), r.into()], "scmp").unwrap();
                let ci = cs.try_as_basic_value().basic().unwrap().into_int_value();
                let zero = self.ctx.i32_type().const_zero();
                let pred = if matches!(op, MirBinOp::EqString) {
                    IntPredicate::EQ
                } else {
                    IntPredicate::NE
                };
                b.build_int_compare(pred, ci, zero, dest).unwrap().into()
            }
        }
    }

    /// Field-by-field structural comparison for Option/Result ADT values
    /// (EqInt/NeqInt with struct operands). Creates null-safe strcmp blocks for
    /// String fields.
    fn emit_adt_compare(
        &mut self,
        op: MirBinOp,
        lhs: &Operand,
        rhs: &Operand,
        dest: &str,
        stype: &str,
        fv: FunctionValue<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let info = &self.struct_map[stype];
        let lv = self.operand(lhs).into_struct_value();
        let rv = self.operand(rhs).into_struct_value();
        let num_fields = info.field_types.len();
        let i1t = self.ctx.bool_type();

        // Classify each field.
        enum FieldKind {
            Scalar,
            StrPtr,
            Unsupported,
        }
        let field_kinds: Vec<FieldKind> = (0..num_fields)
            .map(|fi| {
                if fi == 0 {
                    FieldKind::Scalar // tag is always i8/i1/i64
                } else if !info.recursive_fields.get(fi).copied().unwrap_or(false)
                    && info.field_types.get(fi) == Some(&tyra_types::Ty::String)
                {
                    FieldKind::StrPtr
                } else {
                    match lv.get_type().get_field_type_at_index(fi as u32) {
                        Some(t) if t.is_int_type() || t.is_float_type() => FieldKind::Scalar,
                        _ => FieldKind::Unsupported,
                    }
                }
            })
            .collect();

        let all_supported = field_kinds
            .iter()
            .all(|k| !matches!(k, FieldKind::Unsupported));
        if !all_supported {
            // Fallback: tag-only comparison (unsupported payload — leave as explicit TODO).
            let lt = self
                .builder
                .build_extract_value(lv, 0, &format!("{dest}.lt"))
                .unwrap();
            let rt = self
                .builder
                .build_extract_value(rv, 0, &format!("{dest}.rt"))
                .unwrap();
            let eq = self
                .builder
                .build_int_compare(
                    IntPredicate::EQ,
                    lt.into_int_value(),
                    rt.into_int_value(),
                    dest,
                )
                .unwrap();
            return if matches!(op, MirBinOp::NeqInt) {
                self.builder
                    .build_xor(eq, i1t.const_all_ones(), &format!("{dest}.ne"))
                    .unwrap()
                    .into()
            } else {
                eq.into()
            };
        }

        let strcmp = self.module.get_function("strcmp").unwrap();
        let mut acc: Option<BasicValueEnum<'ctx>> = None;
        for (fi, kind) in field_kinds.iter().enumerate() {
            let lf = self
                .builder
                .build_extract_value(lv, fi as u32, &format!("{dest}.l{fi}"))
                .unwrap();
            let rf = self
                .builder
                .build_extract_value(rv, fi as u32, &format!("{dest}.r{fi}"))
                .unwrap();
            let cmp: BasicValueEnum<'ctx> = match kind {
                FieldKind::StrPtr => {
                    // Null guard: AdtInit zero-fills inactive variant String fields.
                    let lp = lf.into_pointer_value();
                    let rp = rf.into_pointer_value();
                    let null = self.ptr().const_null();
                    let ln = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, lp, null, &format!("{dest}.ln{fi}"))
                        .unwrap();
                    let rn = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, rp, null, &format!("{dest}.rn{fi}"))
                        .unwrap();
                    let any_null = self
                        .builder
                        .build_or(ln, rn, &format!("{dest}.anyn{fi}"))
                        .unwrap();
                    let bb_snull = self
                        .ctx
                        .append_basic_block(fv, &format!("{dest}.snull{fi}"));
                    let bb_scmp = self.ctx.append_basic_block(fv, &format!("{dest}.scmp{fi}"));
                    let bb_sdone = self
                        .ctx
                        .append_basic_block(fv, &format!("{dest}.sdone{fi}"));
                    self.builder
                        .build_conditional_branch(any_null, bb_snull, bb_scmp)
                        .unwrap();
                    // snull: pointer equality (both null → equal, one null → not)
                    self.builder.position_at_end(bb_snull);
                    let pe = self
                        .builder
                        .build_int_compare(IntPredicate::EQ, lp, rp, &format!("{dest}.pe{fi}"))
                        .unwrap();
                    self.builder.build_unconditional_branch(bb_sdone).unwrap();
                    // scmp: strcmp
                    self.builder.position_at_end(bb_scmp);
                    let sc = self
                        .builder
                        .build_call(strcmp, &[lp.into(), rp.into()], &format!("{dest}.sc{fi}"))
                        .unwrap();
                    let si = sc.try_as_basic_value().basic().unwrap().into_int_value();
                    let se = self
                        .builder
                        .build_int_compare(
                            IntPredicate::EQ,
                            si,
                            self.ctx.i32_type().const_zero(),
                            &format!("{dest}.se{fi}"),
                        )
                        .unwrap();
                    self.builder.build_unconditional_branch(bb_sdone).unwrap();
                    // sdone: phi
                    self.builder.position_at_end(bb_sdone);
                    let phi = self
                        .builder
                        .build_phi(i1t, &format!("{dest}.e{fi}"))
                        .unwrap();
                    phi.add_incoming(&[(&pe, bb_snull), (&se, bb_scmp)]);
                    phi.as_basic_value()
                }
                FieldKind::Scalar => {
                    let lfi = lf.into_int_value();
                    let rfi = rf.into_int_value();
                    self.builder
                        .build_int_compare(IntPredicate::EQ, lfi, rfi, &format!("{dest}.f{fi}"))
                        .unwrap()
                        .into()
                }
                FieldKind::Unsupported => unreachable!(),
            };
            acc = Some(match acc {
                None => cmp,
                Some(prev) => self
                    .builder
                    .build_and(
                        prev.into_int_value(),
                        cmp.into_int_value(),
                        &format!("{dest}.a{fi}"),
                    )
                    .unwrap()
                    .into(),
            });
        }
        let eq = acc.unwrap_or_else(|| i1t.const_int(1, false).into());
        if matches!(op, MirBinOp::NeqInt) {
            self.builder
                .build_xor(
                    eq.into_int_value(),
                    i1t.const_all_ones(),
                    &format!("{dest}.ne"),
                )
                .unwrap()
                .into()
        } else {
            eq
        }
    }

    /// Resolve a MIR operand to its SSA value handle.
    pub(crate) fn operand(&self, op: &Operand) -> BasicValueEnum<'ctx> {
        match op {
            Operand::Const(c) => self.const_value(c),
            Operand::Var(name) => self.value_by_name(name),
        }
    }

    /// Resolve a value-producing name to its LLVM handle: a live SSA value if
    /// one exists, else a top-level function's global pointer. The latter lets a
    /// function reference used as a *value* — passed bare to a call, or first
    /// `let`-bound (`Copy`) then used — resolve to `@name`, mirroring the legacy
    /// backend (instr_emit::emit_call_args_typed). The stdlib types such slots
    /// as a ptr (e.g. an http route handler).
    pub(crate) fn value_by_name(&self, name: &str) -> BasicValueEnum<'ctx> {
        self.values.get(name).copied().unwrap_or_else(|| {
            self.fn_values
                .get(name)
                .unwrap_or_else(|| panic!("unbound operand `{name}`"))
                .as_global_value()
                .as_pointer_value()
                .into()
        })
    }

    fn const_value(&self, c: &Constant) -> BasicValueEnum<'ctx> {
        match c {
            Constant::Int(n) => self.ctx.i64_type().const_int(*n as u64, true).into(),
            Constant::Float(f) => self.ctx.f64_type().const_float(*f).into(),
            Constant::Bool(b) => self.ctx.bool_type().const_int(*b as u64, false).into(),
            Constant::StringRef(idx) => self
                .module
                .get_global(&format!(".str.{idx}"))
                .expect("string constant global must be declared (I1)")
                .as_pointer_value()
                .into(),
            Constant::Unit => self.ctx.i64_type().const_zero().into(),
        }
    }

    /// Zero/null value of a basic type (for inactive ADT variant fields).
    pub(crate) fn zero_of(&self, ty: BasicTypeEnum<'ctx>) -> BasicValueEnum<'ctx> {
        match ty {
            BasicTypeEnum::IntType(t) => t.const_zero().into(),
            BasicTypeEnum::FloatType(t) => t.const_zero().into(),
            BasicTypeEnum::PointerType(t) => t.const_null().into(),
            BasicTypeEnum::StructType(t) => t.const_zero().into(),
            BasicTypeEnum::ArrayType(t) => t.const_zero().into(),
            BasicTypeEnum::VectorType(t) => t.const_zero().into(),
            BasicTypeEnum::ScalableVectorType(t) => t.const_zero().into(),
        }
    }

    /// Call a no-return-value runtime function declared in I1.
    fn call_runtime_void(&self, name: &str, args: &[BasicMetadataValueEnum<'ctx>]) {
        let f = self
            .module
            .get_function(name)
            .unwrap_or_else(|| panic!("runtime extern `{name}` must be declared (I1)"));
        self.builder.build_call(f, args, "").unwrap();
    }
}

/// The dest name an instruction defines, if any (for the emittability scan).
fn instr_dest(inst: &Instruction) -> Option<&str> {
    match inst {
        Instruction::Const { dest, .. }
        | Instruction::BinOp { dest, .. }
        | Instruction::Neg { dest, .. }
        | Instruction::Not { dest, .. }
        | Instruction::Copy { dest, .. }
        | Instruction::Phi { dest, .. }
        | Instruction::Alloca { dest }
        | Instruction::Load { dest, .. }
        | Instruction::PtrLoad { dest, .. }
        | Instruction::StructInit { dest, .. }
        | Instruction::FieldGet { dest, .. }
        | Instruction::AdtInit { dest, .. }
        | Instruction::AdtTag { dest, .. }
        | Instruction::AdtPayload { dest, .. }
        | Instruction::StringFormat { dest, .. }
        | Instruction::ListInit { dest, .. }
        | Instruction::ListLen { dest, .. }
        | Instruction::ListGet { dest, .. }
        | Instruction::ListGetSafe { dest, .. }
        | Instruction::ListPush { dest, .. }
        | Instruction::MapGetOption { dest, .. }
        | Instruction::LinkedMapGetOption { dest, .. }
        | Instruction::SortedMapGetOption { dest, .. }
        | Instruction::ClosureBuild { dest, .. }
        | Instruction::Spawn { dest, .. }
        | Instruction::Await { dest, .. }
        | Instruction::JoinAll { dest, .. }
        | Instruction::Select { dest, .. } => Some(dest),
        Instruction::Call { dest, .. } | Instruction::IndirectCall { dest, .. } => dest.as_deref(),
        _ => None,
    }
}

fn is_void_ret(f: &Function) -> bool {
    matches!(f.return_type, tyra_types::Ty::Unit | tyra_types::Ty::Never)
}

fn is_param(f: &Function, name: &str) -> bool {
    f.params.iter().any(|(n, _)| n == name)
}
