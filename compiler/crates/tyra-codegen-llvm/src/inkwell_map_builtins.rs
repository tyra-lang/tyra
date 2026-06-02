//! Inkwell I4e: boxed-collection builtins — `Map`/`Set`/`LinkedMap`/`LinkedSet`.
//!
//! These four families (§17.3.6 / §11 / ADR-0015 / ADR-0019) share one shape:
//! keys (and values) are *boxed* — `GC_malloc(8)` + a typed store — so the
//! runtime can treat every entry uniformly behind a `ptr`, and equality/hashing
//! go through compiler-generated `@tyra_eq_<K>` / `@tyra_hash_<K>` functions
//! passed to the `*_new` constructor. The runtime intrinsics themselves are
//! plain externs declared in I1; the work here is (a) emitting those eq/hash
//! functions once per key type used in the program, and (b) boxing arguments
//! and threading the `ptr` handle through each call.
//!
//! Monomorphized builtin names carry the element types as a suffix:
//!   `__map_new__K__V`, `__map_insert__K__V`, `__map_contains__K`,
//!   `__map_remove__K`, `__map_len` (and the `set`/`linked_map`/`linked_set`
//!   analogues). `set`/`linked_set` are single-element (no value); `linked_map`
//!   uses `tyra_linked_map_contains_key` for `contains` (the only callee that
//!   diverges from the regular `_contains` naming).
//!
//! Boxing (mirrors legacy `emit_box_value`): `Int` → store i64; `Bool` → zext
//! i1→i8, store i8; everything else (`String`, data ptrs) → store ptr.
//!
//! `Map`/`LinkedMap` value *retrieval* is NOT here — it lowers to the dedicated
//! `MapGetOption`/`LinkedMapGetOption` MIR instructions, ported separately.

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
    /// `true` for `Map`/`LinkedMap` (insert takes key + value).
    kv: bool,
}

const MAP: CollFamily = CollFamily {
    tag: "map",
    new: "tyra_map_new",
    insert: "tyra_map_insert",
    remove: "tyra_map_remove",
    contains: "tyra_map_contains",
    len: "tyra_map_len",
    kv: true,
};
const SET: CollFamily = CollFamily {
    tag: "set",
    new: "tyra_set_new",
    insert: "tyra_set_insert",
    remove: "tyra_set_remove",
    contains: "tyra_set_contains",
    len: "tyra_set_len",
    kv: false,
};
const LINKED_MAP: CollFamily = CollFamily {
    tag: "linked_map",
    new: "tyra_linked_map_new",
    insert: "tyra_linked_map_insert",
    remove: "tyra_linked_map_remove",
    contains: "tyra_linked_map_contains_key",
    len: "tyra_linked_map_len",
    kv: true,
};
const LINKED_SET: CollFamily = CollFamily {
    tag: "linked_set",
    new: "tyra_linked_set_new",
    insert: "tyra_linked_set_insert",
    remove: "tyra_linked_set_remove",
    contains: "tyra_linked_set_contains",
    len: "tyra_linked_set_len",
    kv: false,
};

impl<'ctx> CodeGen<'ctx> {
    /// Is `name` a boxed-collection builtin (any family/op)?
    pub(crate) fn is_collection_builtin(name: &str) -> bool {
        matches!(name, "__map_len" | "__set_len" | "__linked_map_len" | "__linked_set_len")
            || [
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
            ]
            .iter()
            .any(|p| name.starts_with(p))
    }

    /// Resolve the family of a collection builtin. `linked_*` is checked first
    /// because the plain `map_`/`set_` infixes would also match its tail.
    fn collection_family(fname: &str) -> Option<&'static CollFamily> {
        if fname.starts_with("__linked_map_") {
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
                // rest = "K__V" (map) or "T" (set); only the key/elem type drives
                // eq/hash. `new` takes the two function pointers, no boxing.
                let k = rest.split("__").next().unwrap_or("String");
                let (eq, hash) = self.eq_hash_fns(k);
                let f = self.runtime_fn(fam.new);
                let cs = self
                    .builder
                    .build_call(
                        f,
                        &[
                            eq.as_global_value().as_pointer_value().into(),
                            hash.as_global_value().as_pointer_value().into(),
                        ],
                        dest.as_deref().unwrap_or(""),
                    )
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "insert" if fam.kv => {
                // rest = "K__V". Box key and value, then insert(coll, kbox, vbox).
                let (k, v) = rest.split_once("__").unwrap_or((rest, "Int"));
                let coll = self.operand(&args[0]);
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
                let coll = self.operand(&args[0]);
                let ebox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.insert);
                let cs = self
                    .builder
                    .build_call(f, &[coll.into(), ebox.into()], dest.as_deref().unwrap_or(""))
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "remove" => {
                // rest = "K"/"T". Box the key/elem, then remove(coll, kbox).
                let coll = self.operand(&args[0]);
                let kbox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.remove);
                let cs = self
                    .builder
                    .build_call(f, &[coll.into(), kbox.into()], dest.as_deref().unwrap_or(""))
                    .unwrap();
                self.store_call_result(dest, cs);
            }
            "contains" => {
                // rest = "K"/"T". Box the key/elem; the runtime returns i32 → i1.
                let coll = self.operand(&args[0]);
                let kbox = self.box_arg(&args[1], rest);
                let f = self.runtime_fn(fam.contains);
                let raw = dest.as_deref().map(|d| format!("{d}.i32")).unwrap_or_default();
                let cs = self
                    .builder
                    .build_call(f, &[coll.into(), kbox.into()], &raw)
                    .unwrap();
                if let Some(d) = dest {
                    let i = cs.try_as_basic_value().basic().unwrap().into_int_value();
                    let zero = self.ctx.i32_type().const_zero();
                    let b = self.builder.build_int_compare(IntPredicate::NE, i, zero, d).unwrap();
                    self.values.insert(d.clone(), b.into());
                }
            }
            "len" => {
                let coll = self.operand(&args[0]);
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
    /// Shape (mirrors legacy `emit_instruction`'s `MapGetOption` arm, single
    /// basic block via `select` rather than branches):
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
        let coll = self.operand(handle).into_pointer_value();
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
        agg = self
            .builder
            .build_insert_value(agg, val, 1, dest)
            .unwrap();
        self.values.insert(dest.to_string(), agg.into_struct_value().into());
    }

    /// Store a collection call's basic return value (a `ptr` for
    /// new/insert/remove, an i64 for len) under `dest`, if present.
    fn store_call_result(&mut self, dest: &Option<String>, cs: CallSiteValue<'ctx>) {
        if let (Some(d), Some(rv)) = (dest, cs.try_as_basic_value().basic()) {
            self.values.insert(d.clone(), rv);
        }
    }

    /// Box an argument value: `GC_malloc(8)` + a typed store, mirroring legacy
    /// `emit_box_value`. `Int` stores i64, `Bool` zero-extends i1→i8, everything
    /// else (`String`, data ptrs) stores the ptr.
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
    /// builtin in the program. Mirrors legacy `collect_elem_types` +
    /// `emit_map_eq_hash`. Every collection is created by a `*_new` call (whose
    /// K is collected), so this covers every type that any reachable
    /// `get`/`insert` could need. Idempotent per type.
    pub(crate) fn emit_collection_eq_hash(&mut self, program: &Program) {
        let mut keys: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for f in &program.functions {
            for stmt in &f.body {
                // Two sources of a key type needing eq/hash: a `*_new`/`*_contains`
                // builtin Call, and a `Map`/`LinkedMap` `.get(k)` instruction
                // (MapGetOption/LinkedMapGetOption boxes its key). A function that
                // only receives a map by param and calls `.get` has no `*_new` of
                // its own, so the latter is required for legacy parity — mirrors
                // codegen.rs collect_elem_types_stmt.
                let k: Option<String> = match &stmt.instr {
                    Instruction::Call { func, .. } => key_type_of_call(func).map(str::to_string),
                    Instruction::MapGetOption { key_ty, .. }
                    | Instruction::LinkedMapGetOption { key_ty, .. } => {
                        Some(key_ty.monomorphized_name())
                    }
                    _ => None,
                };
                let Some(k) = k else { continue };
                if seen.insert(k.clone()) {
                    keys.push(k);
                }
            }
        }
        for k in keys {
            self.emit_eq_hash_fns(&k);
        }
    }

    /// Get (creating on first use) the `(eq, hash)` function pair for key type
    /// `k`. The functions are emitted up front by `emit_collection_eq_hash`, so
    /// this normally just looks them up; it builds them on demand as a fallback.
    fn eq_hash_fns(&mut self, k: &str) -> (FunctionValue<'ctx>, FunctionValue<'ctx>) {
        self.emit_eq_hash_fns(k);
        (
            self.module.get_function(&format!("tyra_eq_{k}")).unwrap(),
            self.module.get_function(&format!("tyra_hash_{k}")).unwrap(),
        )
    }

    /// Emit `@tyra_eq_<k>` (`fn(ptr, ptr) -> i32`) and `@tyra_hash_<k>`
    /// (`fn(ptr) -> i64`) as private functions, mirroring legacy
    /// `emit_map_eq_hash` byte-for-byte in behavior. No-op if already emitted or
    /// if `k` is not a primitive (the type checker rejects non-primitive keys
    /// before codegen, so the latter never occurs for valid programs).
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
        let hash =
            self.module
                .add_function(&hash_name, i64t.fn_type(&[ptr.into()], false), Some(Linkage::Private));
        let hash_entry = self.ctx.append_basic_block(hash, "entry");

        // eq body: load both boxes, compare, zext to i32.
        let a = eq.get_nth_param(0).unwrap().into_pointer_value();
        let b = eq.get_nth_param(1).unwrap().into_pointer_value();
        self.builder.position_at_end(eq_entry);
        match k {
            "Int" => {
                let va = self.builder.build_load(i64t, a, "va").unwrap().into_int_value();
                let vb = self.builder.build_load(i64t, b, "vb").unwrap().into_int_value();
                let r1 = self.builder.build_int_compare(IntPredicate::EQ, va, vb, "r1").unwrap();
                let r = self.builder.build_int_z_extend(r1, i32t, "r").unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let va = self.builder.build_load(i8t, a, "va").unwrap().into_int_value();
                let vb = self.builder.build_load(i8t, b, "vb").unwrap().into_int_value();
                let r1 = self.builder.build_int_compare(IntPredicate::EQ, va, vb, "r1").unwrap();
                let r = self.builder.build_int_z_extend(r1, i32t, "r").unwrap();
                self.builder.build_return(Some(&r)).unwrap();
            }
            _ => {
                // String: deref to the C string ptr, delegate to tyra_cstr_eq.
                let sa = self.builder.build_load(ptr, a, "sa").unwrap().into_pointer_value();
                let sb = self.builder.build_load(ptr, b, "sb").unwrap().into_pointer_value();
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
                let v = self.builder.build_load(i64t, ha, "v").unwrap().into_int_value();
                // Knuth multiplicative hash (odd 64-bit constant), bit-for-bit
                // identical to the legacy backend.
                let knuth = i64t.const_int((-3932073806218323177i64) as u64, false);
                let h = self.builder.build_int_mul(v, knuth, "h").unwrap();
                self.builder.build_return(Some(&h)).unwrap();
            }
            "Bool" => {
                let i8t = self.ctx.i8_type();
                let v = self.builder.build_load(i8t, ha, "v").unwrap().into_int_value();
                let h = self.builder.build_int_z_extend(v, i64t, "h").unwrap();
                self.builder.build_return(Some(&h)).unwrap();
            }
            _ => {
                let sp = self.builder.build_load(ptr, ha, "sp").unwrap().into_pointer_value();
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

/// The key/element type name needing eq/hash for a collection `*_new` or
/// `*_contains` builtin call, or `None` for any other call. Mirrors legacy
/// `collect_elem_types_stmt` (the `_new`/`_contains` cases): `new` carries the
/// key as the first suffix segment; `contains` carries it whole.
fn key_type_of_call(func: &str) -> Option<&str> {
    for fam in ["map", "set", "linked_map", "linked_set"] {
        if let Some(rest) = func.strip_prefix(&format!("__{fam}_new__")) {
            return rest.split("__").next();
        }
        if let Some(rest) = func.strip_prefix(&format!("__{fam}_contains__")) {
            return Some(rest);
        }
    }
    None
}
