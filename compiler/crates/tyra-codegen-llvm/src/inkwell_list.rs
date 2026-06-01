//! Inkwell I3: List<T> instruction emission.
//!
//! Ports the legacy text backend's `list_codegen.rs` (ListInit / ListLen /
//! ListGet / ListGetSafe / ListPush) to the inkwell value-handle model. A
//! `List<T>` is a `{ ptr, i64 }` struct (`data`, `len`) per §11; the struct
//! value carries its own LLVM type, so reads (`ListLen`/`ListGet`/…) extract
//! straight off the operand handle — no `struct_temps` name lookup like the
//! text backend needed.
//!
//! Block-splitting: `ListGet` (bounds check → `exit(102)`, ADR-0012) and
//! `ListGetSafe` (Some/None) genuinely branch mid-instruction. `emit_instr`'s
//! per-instruction `cur_label` block-sync keeps deferred phi resolution
//! pointing at the real predecessor. `ListInit`/`ListPush` avoid splitting:
//! the GC_malloc OOM null-check is omitted (Boehm `GC_malloc` aborts internally
//! on OOM — same precedent as I2e StringFormat) and `ListPush` copies the
//! prefix with `llvm.memcpy` instead of an explicit loop.

use inkwell::IntPredicate;
use inkwell::types::{BasicType, StructType};
use inkwell::values::{AggregateValueEnum, BasicValueEnum};

use tyra_mir::Operand;
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

impl<'ctx> CodeGen<'ctx> {
    /// `dest = list_init(elem_type, [e0, e1, ...])` (§11). GC-allocates
    /// `count * sizeof(elem)` bytes, stores each element, and builds the
    /// `{ ptr, i64 }` struct. Empty list → `{ null, 0 }`.
    pub(crate) fn emit_list_init(&mut self, dest: &str, elem_type: &Ty, elements: &[Operand]) {
        let list_mono = Ty::Generic("List".into(), vec![elem_type.clone()]).monomorphized_name();
        let list_ty = self.struct_types[&list_mono];
        let i64t = self.ctx.i64_type();

        let data: BasicValueEnum<'ctx> = if elements.is_empty() {
            self.ptr().const_null().into()
        } else {
            let elem_bt = self.ty_to_basic_type(elem_type);
            let elem_size = elem_bt.size_of().expect("list element type must be sized");
            let count = i64t.const_int(elements.len() as u64, false);
            let total = self.builder.build_int_mul(count, elem_size, &format!("{dest}.tsz")).unwrap();
            let gc = self.module.get_function("GC_malloc").unwrap();
            let buf = self
                .builder
                .build_call(gc, &[total.into()], &format!("{dest}.ptr"))
                .unwrap()
                .try_as_basic_value()
                .basic()
                .unwrap()
                .into_pointer_value();
            for (i, elem) in elements.iter().enumerate() {
                let v = self.operand(elem);
                let idx = i64t.const_int(i as u64, false);
                let gep = unsafe {
                    self.builder
                        .build_gep(elem_bt, buf, &[idx], &format!("{dest}.gep.{i}"))
                        .unwrap()
                };
                self.builder.build_store(gep, v).unwrap();
            }
            buf.into()
        };

        let len = i64t.const_int(elements.len() as u64, false);
        let v = self.build_list_struct(list_ty, data, len.into(), dest);
        self.values.insert(dest.to_string(), v);
    }

    /// `dest = list_len(list)` — extract field 1 (§11). The struct value carries
    /// its type, so no `elem_type` / struct-name resolution is required.
    pub(crate) fn emit_list_len(&mut self, dest: &str, list: &Operand) {
        let lv = self.operand(list).into_struct_value();
        let v = self.builder.build_extract_value(lv, 1, dest).unwrap();
        self.values.insert(dest.to_string(), v);
    }

    /// `dest = list_get(list, index, elem_type)` — panicking index access (§11).
    /// Out-of-bounds calls `exit(102)` (distinct from panic `exit(101)`, ADR-0012).
    pub(crate) fn emit_list_get(&mut self, dest: &str, list: &Operand, index: &Operand, elem_type: &Ty) {
        let lv = self.operand(list).into_struct_value();
        let data = self
            .builder
            .build_extract_value(lv, 0, &format!("{dest}.data"))
            .unwrap()
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(lv, 1, &format!("{dest}.len"))
            .unwrap()
            .into_int_value();
        let idx = self.operand(index).into_int_value();
        let inb = self
            .builder
            .build_int_compare(IntPredicate::ULT, idx, len, &format!("{dest}.inb"))
            .unwrap();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let ok = self.ctx.append_basic_block(fv, &format!("{dest}.ok"));
        let oob = self.ctx.append_basic_block(fv, &format!("{dest}.oob"));
        self.builder.build_conditional_branch(inb, ok, oob).unwrap();

        // Out-of-bounds: exit(102), unreachable.
        self.builder.position_at_end(oob);
        let exit = self.module.get_function("exit").unwrap();
        let code = self.ctx.i32_type().const_int(102, false);
        self.builder.build_call(exit, &[code.into()], "").unwrap();
        self.builder.build_unreachable().unwrap();

        // In-bounds: GEP + load.
        self.builder.position_at_end(ok);
        let elem_bt = self.ty_to_basic_type(elem_type);
        let gep = unsafe {
            self.builder
                .build_gep(elem_bt, data, &[idx], &format!("{dest}.gep"))
                .unwrap()
        };
        let v = self.builder.build_load(elem_bt, gep, dest).unwrap();
        self.values.insert(dest.to_string(), v);
    }

    /// `dest = list_get_safe(list, index, elem_type)` — returns `Option<T>` (§11):
    /// `Some(elem)` if in bounds, else `None`. Merges the two arms with a phi
    /// (no alloca slot), so it stays correct inside loops.
    pub(crate) fn emit_list_get_safe(&mut self, dest: &str, list: &Operand, index: &Operand, elem_type: &Ty) {
        let lv = self.operand(list).into_struct_value();
        let data = self
            .builder
            .build_extract_value(lv, 0, &format!("{dest}.data"))
            .unwrap()
            .into_pointer_value();
        let len = self
            .builder
            .build_extract_value(lv, 1, &format!("{dest}.len"))
            .unwrap()
            .into_int_value();
        let idx = self.operand(index).into_int_value();
        let inb = self
            .builder
            .build_int_compare(IntPredicate::ULT, idx, len, &format!("{dest}.inb"))
            .unwrap();

        let opt_mono = Ty::Generic("Option".into(), vec![elem_type.clone()]).monomorphized_name();
        let opt_ty = self.struct_types[&opt_mono];
        let elem_bt = self.ty_to_basic_type(elem_type);
        let i8t = self.ctx.i8_type();

        let fv = self.builder.get_insert_block().unwrap().get_parent().unwrap();
        let some_bb = self.ctx.append_basic_block(fv, &format!("{dest}.some"));
        let none_bb = self.ctx.append_basic_block(fv, &format!("{dest}.none"));
        let end_bb = self.ctx.append_basic_block(fv, &format!("{dest}.end"));
        self.builder.build_conditional_branch(inb, some_bb, none_bb).unwrap();

        // Some(elem): tag 0, payload = loaded element.
        self.builder.position_at_end(some_bb);
        let gep = unsafe {
            self.builder
                .build_gep(elem_bt, data, &[idx], &format!("{dest}.gep"))
                .unwrap()
        };
        let elem = self.builder.build_load(elem_bt, gep, &format!("{dest}.elem")).unwrap();
        let mut some_agg: AggregateValueEnum = opt_ty.get_undef().into();
        some_agg = self
            .builder
            .build_insert_value(some_agg, i8t.const_zero(), 0, &format!("{dest}.some.s0"))
            .unwrap();
        some_agg = self
            .builder
            .build_insert_value(some_agg, elem, 1, &format!("{dest}.some.s1"))
            .unwrap();
        let some_val = some_agg.into_struct_value();
        self.builder.build_unconditional_branch(end_bb).unwrap();
        let some_pred = self.builder.get_insert_block().unwrap();

        // None: tag 1, payload = typed zero.
        self.builder.position_at_end(none_bb);
        let zero = self.zero_of(elem_bt);
        let mut none_agg: AggregateValueEnum = opt_ty.get_undef().into();
        none_agg = self
            .builder
            .build_insert_value(none_agg, i8t.const_int(1, false), 0, &format!("{dest}.none.s0"))
            .unwrap();
        none_agg = self
            .builder
            .build_insert_value(none_agg, zero, 1, &format!("{dest}.none.s1"))
            .unwrap();
        let none_val = none_agg.into_struct_value();
        self.builder.build_unconditional_branch(end_bb).unwrap();
        let none_pred = self.builder.get_insert_block().unwrap();

        // Merge.
        self.builder.position_at_end(end_bb);
        let phi = self.builder.build_phi(opt_ty, dest).unwrap();
        phi.add_incoming(&[(&some_val, some_pred), (&none_val, none_pred)]);
        self.values.insert(dest.to_string(), phi.as_basic_value());
    }

    /// `dest = list_push(list, elem, elem_type)` — immutable append (§17.3.5).
    /// Allocates `(len+1) * sizeof(elem)`, `memcpy`s the prefix, and stores
    /// `elem` at the tail. The input list is never mutated.
    pub(crate) fn emit_list_push(&mut self, dest: &str, list: &Operand, elem: &Operand, elem_type: &Ty) {
        let lv = self.operand(list).into_struct_value();
        let list_ty = lv.get_type();
        let olddata = self
            .builder
            .build_extract_value(lv, 0, &format!("{dest}.olddata"))
            .unwrap()
            .into_pointer_value();
        let oldlen = self
            .builder
            .build_extract_value(lv, 1, &format!("{dest}.oldlen"))
            .unwrap()
            .into_int_value();

        let elem_bt = self.ty_to_basic_type(elem_type);
        let elem_size = elem_bt.size_of().expect("list element type must be sized");
        let i64t = self.ctx.i64_type();
        let newlen = self
            .builder
            .build_int_add(oldlen, i64t.const_int(1, false), &format!("{dest}.newlen"))
            .unwrap();
        let newsize = self.builder.build_int_mul(newlen, elem_size, &format!("{dest}.size")).unwrap();
        let gc = self.module.get_function("GC_malloc").unwrap();
        let newdata = self
            .builder
            .build_call(gc, &[newsize.into()], &format!("{dest}.newdata"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();

        // Copy the existing prefix. `llvm.memcpy` with len 0 is a well-defined
        // no-op (LangRef), so an empty source (null `olddata`) is safe. Align 1
        // is always valid; element stride may be 1 byte (Bool).
        let copysize = self.builder.build_int_mul(oldlen, elem_size, &format!("{dest}.copysz")).unwrap();
        self.builder
            .build_memcpy(newdata, 1, olddata, 1, copysize)
            .unwrap();

        // Store the new tail element at index oldlen.
        let tail_gep = unsafe {
            self.builder
                .build_gep(elem_bt, newdata, &[oldlen], &format!("{dest}.tailp"))
                .unwrap()
        };
        let elem_v = self.operand(elem);
        self.builder.build_store(tail_gep, elem_v).unwrap();

        let v = self.build_list_struct(list_ty, newdata.into(), newlen.into(), dest);
        self.values.insert(dest.to_string(), v);
    }

    /// Build a `{ data, len }` list struct value via the insertvalue chain.
    fn build_list_struct(
        &self,
        list_ty: StructType<'ctx>,
        data: BasicValueEnum<'ctx>,
        len: BasicValueEnum<'ctx>,
        dest: &str,
    ) -> BasicValueEnum<'ctx> {
        let mut agg: AggregateValueEnum = list_ty.get_undef().into();
        agg = self.builder.build_insert_value(agg, data, 0, &format!("{dest}.s0")).unwrap();
        agg = self.builder.build_insert_value(agg, len, 1, &format!("{dest}.s1")).unwrap();
        agg.into_struct_value().into()
    }
}
