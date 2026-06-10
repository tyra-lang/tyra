//! Inkwell I4e: boxed-collection builtins —
//! `Map`/`Set`/`LinkedMap`/`LinkedSet`/`SortedMap`/`SortedSet`.
//!
//! Hash families (Map/Set/LinkedMap/LinkedSet) pass two fn-ptrs to `*_new`:
//!   `tyra_eq_<K>(ptr, ptr) -> i32` and `tyra_hash_<K>(ptr) -> i64`.
//!
//! Sorted families (SortedMap/SortedSet, ADR-0024) pass one fn-ptr to `*_new`:
//!   `tyra_cmp_<K>(ptr, ptr) -> i32` (three-way comparison).
//!
//! All families box keys/values the same way: `GC_malloc(8)` + typed store.
//!
//! Monomorphized builtin names:
//!   `__sorted_map_new__K__V`, `__sorted_map_insert__K__V`, etc.
//!   `__sorted_set_new__T`, `__sorted_set_insert__T`, etc.

use inkwell::IntPredicate;
use inkwell::module::Linkage;
use inkwell::values::{AggregateValueEnum, CallSiteValue, FunctionValue, PointerValue};

use tyra_mir::{Instruction, Operand, Program};
use tyra_types::Ty;

use crate::inkwell_codegen::CodeGen;

/// Per-family runtime callees + whether the family carries a value (key-value
/// map vs single-element set).
struct CollFamily {
    /// The infix in the builtin name after `__` (e.g. `map`, `linked_set`).
    tag: &'static str,
    new: &'static str,
    insert: &'static str,
    remove: &'static str,
    contains: &'static str,
    len: &'static str,
    /// `true` for `Map`/`LinkedMap`/`SortedMap` (insert takes key + value).
    kv: bool,
    /// `true` for sorted families: `*_new` takes one `cmp_fn` ptr, not two (eq+hash).
    cmp_only: bool,
}

const MAP: CollFamily = CollFamily {
    tag: "map",
    new: "tyra_map_new",
    insert: "tyra_map_insert",
    remove: "tyra_map_remove",
    contains: "tyra_map_contains",
    len: "tyra_map_len",
    kv: true,
    cmp_only: false,
};
const SET: CollFamily = CollFamily {
    tag: "set",
    new: "tyra_set_new",
    insert: "tyra_set_insert",
    remove: "tyra_set_remove",
    contains: "tyra_set_contains",
    len: "tyra_set_len",
    kv: false,
    cmp_only: false,
};
const LINKED_MAP: CollFamily = CollFamily {
    tag: "linked_map",
    new: "tyra_linked_map_new",
    insert: "tyra_linked_map_insert",
    remove: "tyra_linked_map_remove",
    contains: "tyra_linked_map_contains_key",
    len: "tyra_linked_map_len",
    kv: true,
    cmp_only: false,
};
const LINKED_SET: CollFamily = CollFamily {
    tag: "linked_set",
    new: "tyra_linked_set_new",
    insert: "tyra_linked_set_insert",
    remove: "tyra_linked_set_remove",
    contains: "tyra_linked_set_contains",
    len: "tyra_linked_set_len",
    kv: false,
    cmp_only: false,
};
const SORTED_MAP: CollFamily = CollFamily {
    tag: "sorted_map",
    new: "tyra_sorted_map_new",
    insert: "tyra_sorted_map_insert",
    remove: "tyra_sorted_map_remove",
    contains: "tyra_sorted_map_contains_key",
    len: "tyra_sorted_map_len",
    kv: true,
    cmp_only: true,
};
const SORTED_SET: CollFamily = CollFamily {
    tag: "sorted_set",
    new: "tyra_sorted_set_new",
    insert: "tyra_sorted_set_insert",
    remove: "tyra_sorted_set_remove",
    contains: "tyra_sorted_set_contains",
    len: "tyra_sorted_set_len",
    kv: false,
    cmp_only: true,
};

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a boxed-collection builtin (any family/op)?
    pub(crate) fn is_collection_builtin(name: &str) -> bool {
        matches!(
            name,
            "__map_len"
                | "__set_len"
                | "__linked_map_len"
                | "__linked_set_len"
                | "__sorted_map_len"
                | "__sorted_set_len"
        ) || [
            "__map_new__",
            "__map_insert__",
            "__map_remove__",
            "__map_contains__",
            "__set_new__",
            "__set_insert__",
            "__set_remove__",
            "__set_contains__",
            "__linked_map_new__",
            "__linked_map_insert__",
            "__linked_map_remove__",
            "__linked_map_contains__",
            "__linked_set_new__",
            "__linked_set_insert__",
            "__linked_set_remove__",
            "__linked_set_contains__",
            "__sorted_map_new__",
            "__sorted_map_insert__",
            "__sorted_map_remove__",
            "__sorted_map_contains__",
            "__sorted_set_new__",
            "__sorted_set_insert__",
            "__sorted_set_remove__",
            "__sorted_set_contains__",
        ]
        .iter()
        .any(|p| name.starts_with(p))
    }

    /// Resolve the family of a collection builtin. Longer prefixes checked first
    /// so `sorted_map` doesn't accidentally match `map`.
    fn collection_family(fname: &str) -> Option<&'static CollFamily> {
        if fname.starts_with("__sorted_map_") {
            Some(&SORTED_MAP)
        } else if fname.starts_with("__sorted_set_") {
            Some(&SORTED_SET)
        } else if fname.starts_with("__linked_map_") {
            Some(&LINKED_MAP)
        } else if fname.starts_with("__linked_set_") {
            Some(&LINKED_SET)
        } else if fname.starts_with("__map_") {
            Some(&MAP)
        } else if fname.starts_with("__set_") {
            Some(&SET)
        } else {
            None
        }
    }

    /// Emit a boxed-collection builtin call. Returns `false` if `fname` is not a
    /// collection builtin (caller falls through to the next dispatcher).
    pub(crate) fn emit_collection_builtin(
        &mut self,
        dest: &Option<String>,
        fname: &str,
        args: &[Operand],
    ) -> bool {
        let Some(fam) = Self::collection_family(fname) else {
            return false;
        };
        // Strip `__<tag>_` to get e.g. "new__Int__Int" / "contains__Int" / "len".
        let body = fname
            .strip_prefix("__")
            .and_then(|s| s.strip_prefix(fam.tag))
            .and_then(|s| s.strip_prefix('_'))
            .unwrap_or("");
        let (op, rest) = body.split_once("__").unwrap_or((body, ""));

        match op {
            "new" => {
                // rest = "K__V" (map) or "T" (set); only the key/elem type drives eq/hash/cmp.
                let k = rest.split("__").next().unwrap_or("String");
                let f = self.runtime_fn(fam.new);
                let cs = if fam.cmp_only {
                    // Sorted families: pass one cmp_fn pointer.
                    let cmp = self.cmp_fn(k);
                    self.builder
                        .build_call(
                            f,
                            &[cmp.as_global_value().as_pointer_value().into()],
                            dest.as_deref().unwrap_or(""),
                        )
                        .unwrap()
                } else {
                    // Hash families: pass eq + hash pointers.
                    let (eq, hash) = self.eq_hash_fns(k);
                    self.builder
                        .build_call(
                            f,
                            &[
                                eq.as_global_value().as_pointer_value().into(),
                                hash.as_global_value().as_pointer_value().into(),
                            ],
                            dest.as_deref().unwrap_or(""),
                        )
                        .unwrap()
                };
                self.store_call_result(dest, cs);
            }
            "insert" if fam.kv => {
                // rest = "K__V". Box key and value, then insert(coll, kbox, vbox).
                let (k, v) = rest.split_once("__").unwrap_or((rest, "Int"));
                let coll = self.collection_ptr(&args[0]);
                let kbox = self.box_arg(&args[1], k);
                let vbox = self.box_arg(&args[2], v);
                let f = self.runtime_fn(fam.insert);
                let cs = self
                    .builder
                    .build_call(
                        f,
                        &[coll.into(), kbox.into(), vbox.into()],
                        dest.as_deref().unwrap_or(""),
                    )
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "insert" => {
                // Set: rest = "T". Box the element, then insert(coll, ebox).
                let coll = self.collection_ptr(&args[0]);
                let ebox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.insert);
                let cs = self
                    .builder
                    .build_call(
                        f,
                        &[coll.into(), ebox.into()],
                        dest.as_deref().unwrap_or(""),
                    )
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "remove" => {
                // rest = "K"/"T". Box the key/elem, then remove(coll, kbox).
                let coll = self.collection_ptr(&args[0]);
                let kbox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.remove);
                let cs = self
                    .builder
                    .build_call(
                        f,
                        &[coll.into(), kbox.into()],
                        dest.as_deref().unwrap_or(""),
                    )
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "contains" => {
                // rest = "K"/"T". Box the key/elem; the runtime returns i32 → i1.
                let coll = self.collection_ptr(&args[0]);
                let kbox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.contains);
                let raw = dest
                    .as_deref()
                    .map(|d| format!("{d}.i32"))
                    .unwrap_or_default();
                let cs = self
                    .builder
                    .build_call(f, &[coll.into(), kbox.into()], &raw)
                    .unwrap();
                if let Some(d) = dest {
                    let i = cs.try_as_basic_value().basic().unwrap().into_int_value();
                    let zero = self.ctx.i32_type().const_zero();
                    let b = self
                        .builder
                        .build_int_compare(IntPredicate::NE, i, zero, d)
                        .unwrap();
                    self.values.insert(d.clone(), b.into());
                }
            }
            "len" => {
                let coll = self.collection_ptr(&args[0]);
                let f = self.runtime_fn(fam.len);
                let cs = self
                    .builder
                    .build_call(f, &[coll.into()], dest.as_deref().unwrap_or(""))
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            _ => return false,
        }
        true
    }

    /// Emit `Map`/`LinkedMap` value retrieval — the `MapGetOption` /
    /// `LinkedMapGetOption` MIR instructions (ADR-0015 / ADR-0019). `getter` is
    /// the runtime callee (`tyra_map_get` / `tyra_linked_map_get`), both
    /// `fn(ptr coll, ptr keybox) -> ptr valuebox` returning null when absent.
    ///
    /// Shape (single basic block via `select` rather than branches):
    ///   1. box the key (same boxing as insert/contains);
    ///   2. `vbox = getter(coll, kbox)`; `present = vbox != null`;
    ///   3. `safe = present ? vbox : @.tyra_zero_slot` (null-safe load source);
    ///   4. load the value through `safe` (`Bool` loads i8 then truncates to
    ///      i1, matching the i8 box store);
    ///   5. build `Option<V> { tag: present ? 0 : 1, value: present ? loaded : 0 }`.
    pub(crate) fn emit_map_get_option(
        &mut self,
        dest: &str,
        handle: &Operand,
        key: &Operand,
        key_ty: &Ty,
        val_ty: &Ty,
        getter: &str,
    ) {
        let coll = self.collection_ptr(handle);
        let kbox = self.box_arg(key, &key_ty.monomorphized_name());

        let get = self.runtime_fn(getter);
        let vbox = self
            .builder
            .build_call(get, &[coll.into(), kbox.into()], &format!("{dest}.vbox"))
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        let null = self.ptr().const_null();
        let present = self
            .builder
            .build_int_compare(IntPredicate::NE, vbox, null, &format!("{dest}.present"))
            .unwrap();

        // Null-safe load source: never dereference null in the not-found branch.
        let zero_slot = self
            .module
            .get_global(".tyra_zero_slot")
            .expect("zero slot global (I1)")
            .as_pointer_value();
        let safe = self
            .builder
            .build_select(present, vbox, zero_slot, &format!("{dest}.safe"))
            .unwrap()
            .into_pointer_value();

        // Load the value with its payload type. `Bool` is boxed as i8 (zext of
        // i1), so load i8 and truncate back to the i1 Option payload.
        let payload_bt = self.ty_to_basic_type(val_ty);
        let raw = if matches!(val_ty, Ty::Bool) {
            let i8t = self.ctx.i8_type();
            let byte = self
                .builder
                .build_load(i8t, safe, &format!("{dest}.raw8"))
                .unwrap()
                .into_int_value();
            self.builder
                .build_int_truncate(byte, self.ctx.bool_type(), &format!("{dest}.rawval"))
                .unwrap()
                .into()
        } else {
            self.builder
                .build_load(payload_bt, safe, &format!("{dest}.rawval"))
                .unwrap()
        };

        let i8t = self.ctx.i8_type();
        let tag = self
            .builder
            .build_select(
                present,
                i8t.const_zero(),
                i8t.const_int(1, false),
                &format!("{dest}.tag"),
            )
            .unwrap();
        let zero = self.zero_of(payload_bt);
        let val = self
            .builder
            .build_select(present, raw, zero, &format!("{dest}.val"))
            .unwrap();

        // Build Option<V> = { tag: i8, value: payload }.
        let opt_mono = Ty::Generic("Option".into(), vec![val_ty.clone()]).monomorphized_name();
        let opt_ty = self.struct_types[&opt_mono];
        let mut agg: AggregateValueEnum = opt_ty.get_undef().into();
        agg = self
            .builder
            .build_insert_value(agg, tag, 0, &format!("{dest}.s0"))
            .unwrap();
        agg = self.builder.build_insert_value(agg, val, 1, dest).unwrap();
        self.values
            .insert(dest.to_string(), agg.into_struct_value().into());
    }

    /// Store a collection call's basic return value (a `ptr` for
    /// new/insert/remove, an i64 for len) under `dest`, if present.
    fn store_call_result(&mut self, dest: &Option<String>, cs: CallSiteValue<'ctx>) {
        if let (Some(d), Some(rv)) = (dest, cs.try_as_basic_value().basic()) {
            self.values.insert(d.clone(), rv);
        }
    }

    /// Box an argument value: `GC_malloc(8)` + a typed store. `Int` stores i64,
    /// `Bool` zero-extends i1→i8, everything else (`String`, data ptrs) stores
    /// the ptr.
    fn box_arg(&mut self, op: &Operand, ty_name: &str) -> PointerValue<'ctx> {
        let val = self.operand(op);
        let i64t = self.ctx.i64_type();
        let gc = self.runtime_fn("GC_malloc");
        let boxp = self
            .builder
            .build_call(gc, &[i64t.const_int(8, false).into()], "box")
            .unwrap()
            .try_as_basic_value()
            .basic()
            .unwrap()
            .into_pointer_value();
        match ty_name {
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let w = self
                    .builder
                    .build_int_z_extend(val.into_int_value(), i8t, "box.i8")
                    .unwrap();
                self.builder.build_store(boxp, w).unwrap();
            }
            // Int (i64) and String / data ptrs store the value directly.
            _ => {
                self.builder.build_store(boxp, val).unwrap();
            }
        }
        boxp
    }

    fn runtime_fn(&self, name: &str) -> FunctionValue<'ctx> {
        self.module
            .get_function(name)
            .unwrap_or_else(|| panic!("runtime extern `{name}` must be declared (I1)"))
    }

    /// Emit the compiler-generated `@tyra_eq_<K>` / `@tyra_hash_<K>` functions
    /// for every key/element type used by a collection `*_new`/`*_contains`
    /// builtin in the program. Every collection is created by a `*_new` call (whose
    /// K is collected), so this covers every type that any reachable
    /// `get`/`insert` could need. Idempotent per type.
    pub(crate) fn emit_collection_eq_hash(&mut self, program: &Program) {
        let mut hash_keys: Vec<String> = Vec::new();
        let mut cmp_keys: Vec<String> = Vec::new();
        let mut seen_hash = std::collections::HashSet::new();
        let mut seen_cmp = std::collections::HashSet::new();
        for f in &program.functions {
            for stmt in &f.body {
                match &stmt.instr {
                    Instruction::Call { func, .. } => {
                        if let Some(k) = key_type_of_call(func) {
                            if is_sorted_call(func) {
                                if seen_cmp.insert(k.to_string()) {
                                    cmp_keys.push(k.to_string());
                                }
                            } else if seen_hash.insert(k.to_string()) {
                                hash_keys.push(k.to_string());
                            }
                        }
                    }
                    Instruction::MapGetOption { key_ty, .. }
                    | Instruction::LinkedMapGetOption { key_ty, .. } => {
                        let k = key_ty.monomorphized_name();
                        if seen_hash.insert(k.clone()) {
                            hash_keys.push(k);
                        }
                    }
                    Instruction::SortedMapGetOption { key_ty, .. } => {
                        let k = key_ty.monomorphized_name();
                        if seen_cmp.insert(k.clone()) {
                            cmp_keys.push(k);
                        }
                    }
                    _ => {}
                }
            }
        }
        for k in hash_keys {
            self.emit_eq_hash_fns(&k);
        }
        for k in cmp_keys {
            self.emit_cmp_fn(&k);
        }
    }

    /// Get (creating on first use) the `(eq, hash)` function pair for key type `k`.
    fn eq_hash_fns(&mut self, k: &str) -> (FunctionValue<'ctx>, FunctionValue<'ctx>) {
        self.emit_eq_hash_fns(k);
        (
            self.module.get_function(&format!("tyra_eq_{k}")).unwrap(),
            self.module.get_function(&format!("tyra_hash_{k}")).unwrap(),
        )
    }

    /// Get (creating on first use) the `tyra_cmp_<K>` function for sorted collections.
    fn cmp_fn(&mut self, k: &str) -> FunctionValue<'ctx> {
        self.emit_cmp_fn(k);
        self.module
            .get_function(&format!("tyra_cmp_{k}"))
            .unwrap_or_else(|| panic!("tyra_cmp_{k} should have been emitted"))
    }

    /// Emit `@tyra_cmp_<K>` (`fn(ptr, ptr) -> i32`): three-way comparison for
    /// sorted collection keys. No-op if already emitted or if `k` is not a
    /// supported primitive (the checker rejects Float keys before codegen).
    fn emit_cmp_fn(&mut self, k: &str) {
        let cmp_name = format!("tyra_cmp_{k}");
        if self.module.get_function(&cmp_name).is_some() {
            return;
        }
        if !matches!(k, "Int" | "Bool" | "String") {
            return;
        }
        let ptr = self.ptr();
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();
        let cmp = self.module.add_function(
            &cmp_name,
            i32t.fn_type(&[ptr.into(), ptr.into()], false),
            Some(Linkage::Private),
        );
        let entry = self.ctx.append_basic_block(cmp, "entry");
        let a = cmp.get_nth_param(0).unwrap().into_pointer_value();
        let b = cmp.get_nth_param(1).unwrap().into_pointer_value();
        self.builder.position_at_end(entry);
        match k {
            "Int" => {
                let va = self
                    .builder
                    .build_load(i64t, a, "va")
                    .unwrap()
                    .into_int_value();
                let vb = self
                    .builder
                    .build_load(i64t, b, "vb")
                    .unwrap()
                    .into_int_value();
                // (a < b) ? -1 : (a > b) ? 1 : 0
                let lt = self
                    .builder
                    .build_int_compare(IntPredicate::SLT, va, vb, "lt")
                    .unwrap();
                let gt = self
                    .builder
                    .build_int_compare(IntPredicate::SGT, va, vb, "gt")
                    .unwrap();
                let neg1 = i32t.const_int((-1i32) as u64, false);
                let one = i32t.const_int(1, false);
                let zero = i32t.const_zero();
                let r1 = self
                    .builder
                    .build_select(gt, one, zero, "r1")
                    .unwrap()
                    .into_int_value();
                let r = self
                    .builder
                    .build_select(lt, neg1, r1, "r")
                    .unwrap()
                    .into_int_value();
                self.builder.build_return(Some(&r)).unwrap();
            }
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let va = self
                    .builder
                    .build_load(i8t, a, "va")
                    .unwrap()
                    .into_int_value();
                let vb = self
                    .builder
                    .build_load(i8t, b, "vb")
                    .unwrap()
                    .into_int_value();
                let lt = self
                    .builder
                    .build_int_compare(IntPredicate::ULT, va, vb, "lt")
                    .unwrap();
                let gt = self
                    .builder
                    .build_int_compare(IntPredicate::UGT, va, vb, "gt")
                    .unwrap();
                let neg1 = i32t.const_int((-1i32) as u64, false);
                let one = i32t.const_int(1, false);
                let zero = i32t.const_zero();
                let r1 = self
                    .builder
                    .build_select(gt, one, zero, "r1")
                    .unwrap()
                    .into_int_value();
                let r = self
                    .builder
                    .build_select(lt, neg1, r1, "r")
                    .unwrap()
                    .into_int_value();
                self.builder.build_return(Some(&r)).unwrap();
            }
            _ => {
                // String: deref to the C string ptr, delegate to tyra_cstr_cmp.
                let sa = self
                    .builder
                    .build_load(ptr, a, "sa")
                    .unwrap()
                    .into_pointer_value();
                let sb = self
                    .builder
                    .build_load(ptr, b, "sb")
                    .unwrap()
                    .into_pointer_value();
                let cstr_cmp = self.runtime_fn("tyra_cstr_cmp");
                let r = self
                    .builder
                    .build_call(cstr_cmp, &[sa.into(), sb.into()], "r")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
        }
    }

    /// Emit `@tyra_eq_<k>` (`fn(ptr, ptr) -> i32`) and `@tyra_hash_<k>`
    /// (`fn(ptr) -> i64`) as private functions. No-op if already emitted or if
    /// `k` is not a primitive (the type checker rejects non-primitive keys before
    /// codegen, so the latter never occurs for valid programs).
    fn emit_eq_hash_fns(&mut self, k: &str) {
        let eq_name = format!("tyra_eq_{k}");
        if self.module.get_function(&eq_name).is_some() {
            return;
        }
        if !matches!(k, "Int" | "Bool" | "String") {
            return;
        }
        let ptr = self.ptr();
        let i32t = self.ctx.i32_type();
        let i64t = self.ctx.i64_type();

        // Declare both functions before emitting bodies.
        let eq = self.module.add_function(
            &eq_name,
            i32t.fn_type(&[ptr.into(), ptr.into()], false),
            Some(Linkage::Private),
        );
        let eq_entry = self.ctx.append_basic_block(eq, "entry");
        let hash_name = format!("tyra_hash_{k}");
        let hash = self.module.add_function(
            &hash_name,
            i64t.fn_type(&[ptr.into()], false),
            Some(Linkage::Private),
        );
        let hash_entry = self.ctx.append_basic_block(hash, "entry");

        // eq body: load both boxes, compare, zext to i32.
        let a = eq.get_nth_param(0).unwrap().into_pointer_value();
        let b = eq.get_nth_param(1).unwrap().into_pointer_value();
        self.builder.position_at_end(eq_entry);
        match k {
            "Int" => {
                let va = self
                    .builder
                    .build_load(i64t, a, "va")
                    .unwrap()
                    .into_int_value();
                let vb = self
                    .builder
                    .build_load(i64t, b, "vb")
                    .unwrap()
                    .into_int_value();
                let r1 = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, va, vb, "r1")
                    .unwrap();
                let r = self.builder.build_int_z_extend(r1, i32t, "r").unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let va = self
                    .builder
                    .build_load(i8t, a, "va")
                    .unwrap()
                    .into_int_value();
                let vb = self
                    .builder
                    .build_load(i8t, b, "vb")
                    .unwrap()
                    .into_int_value();
                let r1 = self
                    .builder
                    .build_int_compare(IntPredicate::EQ, va, vb, "r1")
                    .unwrap();
                let r = self.builder.build_int_z_extend(r1, i32t, "r").unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
            _ => {
                // String: deref to the C string ptr, delegate to tyra_cstr_eq.
                let sa = self
                    .builder
                    .build_load(ptr, a, "sa")
                    .unwrap()
                    .into_pointer_value();
                let sb = self
                    .builder
                    .build_load(ptr, b, "sb")
                    .unwrap()
                    .into_pointer_value();
                let cstr_eq = self.runtime_fn("tyra_cstr_eq");
                let r = self
                    .builder
                    .build_call(cstr_eq, &[sa.into(), sb.into()], "r")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
        }

        // hash body: load the box, hash.
        let ha = hash.get_nth_param(0).unwrap().into_pointer_value();
        self.builder.position_at_end(hash_entry);
        match k {
            "Int" => {
                let v = self
                    .builder
                    .build_load(i64t, ha, "v")
                    .unwrap()
                    .into_int_value();
                // Knuth multiplicative hash (odd 64-bit constant).
                let knuth = i64t.const_int((-3932073806218323177i64) as u64, false);
                let h = self.builder.build_int_mul(v, knuth, "h").unwrap();
                self.builder.build_return(Some(&h)).unwrap();
            }
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let v = self
                    .builder
                    .build_load(i8t, ha, "v")
                    .unwrap()
                    .into_int_value();
                let h = self.builder.build_int_z_extend(v, i64t, "h").unwrap();
                self.builder.build_return(Some(&h)).unwrap();
            }
            _ => {
                let sp = self
                    .builder
                    .build_load(ptr, ha, "sp")
                    .unwrap()
                    .into_pointer_value();
                let hash_cstr = self.runtime_fn("tyra_hash_cstr");
                let h = self
                    .builder
                    .build_call(hash_cstr, &[sp.into()], "h")
                    .unwrap()
                    .try_as_basic_value()
                    .basic()
                    .unwrap();
                self.builder.build_return(Some(&h)).unwrap();
            }
        }
    }
}

/// The key/element type name for a collection `*_new` or `*_contains` builtin
/// call, or `None` for any other call.
fn key_type_of_call(func: &str) -> Option<&str> {
    for fam in [
        "sorted_map",
        "sorted_set",
        "linked_map",
        "linked_set",
        "map",
        "set",
    ] {
        if let Some(rest) = func.strip_prefix(&format!("__{fam}_new__")) {
            return rest.split("__").next();
        }
        if let Some(rest) = func.strip_prefix(&format!("__{fam}_contains__")) {
            return Some(rest);
        }
    }
    None
}

/// Returns `true` if `func` is a sorted-collection builtin (needs cmp, not eq+hash).
fn is_sorted_call(func: &str) -> bool {
    func.starts_with("__sorted_map_") || func.starts_with("__sorted_set_")
}
