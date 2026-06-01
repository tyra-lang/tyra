//! Inkwell I2a: core scalar and control-flow instruction emission.
//!
//! Ports the scalar / control-flow subset of the legacy `emit_instruction`
//! (instr_emit.rs) to the inkwell builder. The value-handle model removes the
//! text backend's per-temp type tables (`string_temps`/`float_temps`/…): a
//! `BasicValueEnum` carries its own LLVM type, so e.g. `icmp` width selection
//! is automatic from the operand handles.
//!
//! Scope (I2a): Const, BinOp (scalar arithmetic/compare, string eq, and/or),
//! Neg, Not, Copy, Return, Label, Jump, BranchIf, Phi (deferred resolution),
//! and Call to *user* functions. A function whose body contains any other
//! instruction (Alloca/Store/Load → I2b; StructInit/ADT/StringFormat/list →
//! I2c; builtin Call → I4) falls back to a single `unreachable` block so the
//! module still verifies, and coverage expands phase by phase.

use std::collections::HashSet;

use inkwell::types::BasicTypeEnum;
use inkwell::values::{AggregateValueEnum, BasicMetadataValueEnum, BasicValueEnum, PhiValue};
use inkwell::{FloatPredicate, IntPredicate};

use tyra_mir::{Constant, Function, Instruction, MirBinOp, Operand, Program};

use crate::inkwell_codegen::CodeGen;

impl<'ctx> CodeGen<'ctx> {
    /// I2: emit a body for every function. Fully-supported functions get real
    /// instructions; the rest get a single `unreachable` block (so the module
    /// verifies while instruction coverage grows phase by phase).
    pub(crate) fn emit_bodies(&mut self, program: &Program) {
        for f in &program.functions {
            // I4c: per-function type scan (operand Tyra types for `print`
            // routing + the emittability gate's printability check). Computed
            // into a local first so the immutable borrows of struct_map/fn_sigs
            // end before the mutable store into self.scan.
            let scan = crate::type_scan::scan_function_types(f, &self.struct_map, &self.fn_sigs);
            self.scan = Some(scan);
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
            Some(Instruction::Return { .. } | Instruction::Jump { .. } | Instruction::BranchIf { .. }) => {}
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
                | Instruction::ListPush { .. } => true,
                Instruction::Call { func, args, .. } => {
                    self.fn_values.contains_key(func)
                        || (Self::is_supported_builtin(func)
                            && self.builtin_args_emittable(f, func, args))
                }
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
                    if let Some((first, _)) = branches.first() {
                        if !self.operand_resolvable_now(first, &seen) {
                            return false;
                        }
                    }
                }
                _ => {
                    if !self.operands_of(inst).iter().all(|op| match op {
                        Operand::Var(n) => seen.contains(n.as_str()),
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
                        if !seen.contains(n) {
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
        if self.scan.as_ref().is_some_and(|s| s.struct_temps.contains_key(name)) {
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
            Operand::Var(n) => seen.contains(n.as_str()),
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
            Instruction::ListGet { list, index, .. } | Instruction::ListGetSafe { list, index, .. } => {
                vec![list, index]
            }
            Instruction::ListPush { list, elem, .. } => vec![list, elem],
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

        // Parameters: bind the SSA arg for direct (immutable) operand refs and
        // also create a `.addr` slot (matching the legacy backend) so mutation
        // (Store) and `Copy`-from-param read the mutable view.
        if !f.is_main {
            let i64t = self.ctx.i64_type();
            for (i, (name, ty)) in f.params.iter().enumerate() {
                let p = fv.get_nth_param(i as u32).unwrap();
                p.set_name(name);
                self.values.insert(name.clone(), p);
                let slot = self.builder.build_alloca(i64t, &format!("{name}.addr")).unwrap();
                self.builder.build_store(slot, p).unwrap();
                self.addr_slots.insert(name.clone(), slot);
                let bt = self.ty_to_basic_type(ty);
                self.slot_types.insert(name.clone(), bt);
            }
        }

        // Hoist every local alloca slot to the entry block (allocated once, not
        // per loop iteration). Slots are `alloca i64` (8 bytes covers every
        // scalar/ptr local an I2b-emittable function can hold); the load type is
        // tracked per-slot via `slot_types` (refined on Store).
        let i64t = self.ctx.i64_type();
        for stmt in &f.body {
            if let Instruction::Alloca { dest } = &stmt.instr {
                if !self.addr_slots.contains_key(dest) {
                    let slot = self.builder.build_alloca(i64t, dest).unwrap();
                    self.addr_slots.insert(dest.clone(), slot);
                    self.slot_types.insert(dest.clone(), i64t.into());
                }
            }
        }

        if f.is_main {
            // C entry: i32 @main(i32 %argc, ptr %argv). Initialize GC + runtime
            // and capture argc/argv for sys.args() (ADR-0007 / sys.args).
            self.call_runtime_void("GC_init", &[]);
            self.call_runtime_void("tyra_rt_init", &[]);
            let argc = fv.get_nth_param(0).unwrap();
            let argv = fv.get_nth_param(1).unwrap();
            let argc_g = self.module.get_global(".tyra.argc").unwrap().as_pointer_value();
            self.builder.build_store(argc_g, argc).unwrap();
            let argv_g = self.module.get_global(".tyra.argv").unwrap().as_pointer_value();
            self.builder.build_store(argv_g, argv).unwrap();
        }

        let mut pending: Vec<(PhiValue<'ctx>, Vec<(Operand, String)>)> = Vec::new();
        for stmt in &f.body {
            self.emit_instr(&stmt.instr, f, &mut pending);
            // Track the *exit* block of the current label (for phi predecessors)
            // without touching `blocks` (the jump-*target* table). For a
            // non-splitting instruction this is idempotent; for ListGet/
            // ListGetSafe (which branch mid-instruction) it advances to the
            // actual block the region's terminator will branch from.
            if let Some(label) = &self.cur_label {
                if let Some(bb) = self.builder.get_insert_block() {
                    self.pred_blocks.insert(label.clone(), bb);
                }
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
                    self.values[source]
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
            Instruction::StructInit { dest, type_name, fields } => {
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
                    let mut agg: AggregateValueEnum = st.get_undef().into();
                    for (i, fop) in fields.iter().enumerate() {
                        let v = self.operand(fop);
                        agg = self
                            .builder
                            .build_insert_value(agg, v, i as u32, &format!("{dest}.s{i}"))
                            .unwrap();
                    }
                    self.values.insert(dest.clone(), agg.into_struct_value().into());
                }
            }
            Instruction::FieldGet { dest, obj, type_name, field_index } => {
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
            Instruction::FieldSet { obj, type_name, field_index, value } => {
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
            Instruction::AdtInit { dest, type_name, tag, fields } => {
                let st = self.struct_types[type_name];
                let recursive = self.recursive_fields.get(type_name).cloned().unwrap_or_default();
                let num_fields = st.count_fields() as usize;
                // Field 0 is the i8 tag.
                let mut agg: AggregateValueEnum = st.get_undef().into();
                let tag_v = self.ctx.i8_type().const_int(*tag as u64, false);
                agg = self.builder.build_insert_value(agg, tag_v, 0, "adt.tag").unwrap();
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
                self.values.insert(dest.clone(), agg.into_struct_value().into());
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
            Instruction::AdtPayload { dest, obj, type_name, field_index } => {
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
                let v = self.emit_binop(*op, lhs, rhs, dest);
                self.values.insert(dest.clone(), v);
            }
            Instruction::Neg { dest, operand } => {
                let o = self.operand(operand);
                let v: BasicValueEnum = if o.is_float_value() {
                    self.builder.build_float_neg(o.into_float_value(), dest).unwrap().into()
                } else {
                    self.builder.build_int_neg(o.into_int_value(), dest).unwrap().into()
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
                self.builder.build_unconditional_branch(self.blocks[label]).unwrap();
            }
            Instruction::BranchIf { cond, true_label, false_label } => {
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
                if let Some(&callee) = self.fn_values.get(func) {
                    let argvals: Vec<BasicMetadataValueEnum<'ctx>> =
                        args.iter().map(|a| self.operand(a).into()).collect();
                    let cs = self
                        .builder
                        .build_call(callee, &argvals, dest.as_deref().unwrap_or(""))
                        .unwrap();
                    if let Some(d) = dest {
                        if let Some(rv) = cs.try_as_basic_value().basic() {
                            self.values.insert(d.clone(), rv);
                        }
                    }
                } else {
                    // Builtin (the gate admitted only supported ones, I4a+).
                    let handled = self.emit_builtin(dest, func, args);
                    debug_assert!(handled, "gate admitted unsupported builtin `{func}`");
                }
            }
            Instruction::ListInit { dest, elem_type, elements } => {
                self.emit_list_init(dest, elem_type, elements);
            }
            Instruction::ListLen { dest, list } => {
                self.emit_list_len(dest, list);
            }
            Instruction::ListGet { dest, list, index, elem_type } => {
                self.emit_list_get(dest, list, index, elem_type);
            }
            Instruction::ListGetSafe { dest, list, index, elem_type } => {
                self.emit_list_get_safe(dest, list, index, elem_type);
            }
            Instruction::ListPush { dest, list, elem, elem_type } => {
                self.emit_list_push(dest, list, elem, elem_type);
            }
            Instruction::StringFormat { dest, format_ref, args } => {
                // GC-allocate a 1024-byte buffer and snprintf into it. The
                // legacy backend adds a defensive GC_malloc null check + abort
                // branch; it is omitted here because Boehm GC_malloc never
                // returns null (its OOM handler aborts internally), and adding
                // the branch would split the current basic block — which would
                // break phi-predecessor bookkeeping. Observable behavior is
                // identical (abort on OOM either way).
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
                self.builder.build_call(snprintf, &call_args, "fmt").unwrap();
                self.values.insert(dest.clone(), buf.into());
            }
            // Not in I2 scope; is_i2_emittable guarantees we never reach here.
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
            MirBinOp::AddInt => b.build_int_add(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::SubInt => b.build_int_sub(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::MulInt => b.build_int_mul(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::DivInt => b.build_int_signed_div(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::RemInt => b.build_int_signed_rem(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::AddFloat => b.build_float_add(l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::SubFloat => b.build_float_sub(l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::MulFloat => b.build_float_mul(l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::DivFloat => b.build_float_div(l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            // Width (i1 vs i64) is taken from the operand handles automatically.
            MirBinOp::EqInt => b.build_int_compare(IntPredicate::EQ, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::NeqInt => b.build_int_compare(IntPredicate::NE, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::LtInt => b.build_int_compare(IntPredicate::SLT, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::LeInt => b.build_int_compare(IntPredicate::SLE, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::GtInt => b.build_int_compare(IntPredicate::SGT, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::GeInt => b.build_int_compare(IntPredicate::SGE, l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::LtFloat => b.build_float_compare(FloatPredicate::OLT, l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::LeFloat => b.build_float_compare(FloatPredicate::OLE, l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::GtFloat => b.build_float_compare(FloatPredicate::OGT, l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::GeFloat => b.build_float_compare(FloatPredicate::OGE, l.into_float_value(), r.into_float_value(), dest).unwrap().into(),
            MirBinOp::And => b.build_and(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
            MirBinOp::Or => b.build_or(l.into_int_value(), r.into_int_value(), dest).unwrap().into(),
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

    /// Resolve a MIR operand to its SSA value handle.
    pub(crate) fn operand(&self, op: &Operand) -> BasicValueEnum<'ctx> {
        match op {
            Operand::Const(c) => self.const_value(c),
            Operand::Var(name) => self.values[name],
        }
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
        | Instruction::ListPush { dest, .. } => Some(dest),
        Instruction::Call { dest, .. } => dest.as_deref(),
        _ => None,
    }
}

fn is_void_ret(f: &Function) -> bool {
    matches!(f.return_type, tyra_types::Ty::Unit | tyra_types::Ty::Never)
}

fn is_param(f: &Function, name: &str) -> bool {
    f.params.iter().any(|(n, _)| n == name)
}
