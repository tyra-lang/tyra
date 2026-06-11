// ADT-related lowering helpers: type registration, constructor inference, and monomorphization.

use tyra_types::Ty;

use crate::monomorphize::substitute_fn_def;

impl super::LowerCtx<'_> {
    /// Register an ADT struct def for a generic type (Option<T>, Result<T, E>).
    /// Creates a monomorphized StructDef if not already registered.
    pub(super) fn register_adt_type(&mut self, ty: &Ty) {
        let mono_name = ty.monomorphized_name();
        if self.adt_struct_defs.contains_key(&mono_name) {
            return;
        }
        if let Some(inner) = ty.option_inner() {
            // Option<T> = { tag: Int, value: T }
            self.adt_struct_defs.insert(
                mono_name,
                vec![("tag".into(), Ty::Int), ("value".into(), inner.clone())],
            );
        } else if let (Some(ok_ty), Some(err_ty)) = (ty.result_ok_type(), ty.result_err_type()) {
            // Result<T, E> = { tag: Int, ok_value: T, err_value: E }
            // For v0.1, we store both ok and err payloads separately.
            self.adt_struct_defs.insert(
                mono_name,
                vec![
                    ("tag".into(), Ty::Int),
                    ("ok_value".into(), ok_ty.clone()),
                    ("err_value".into(), err_ty.clone()),
                ],
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "Map") {
            // Map<K,V> (ADR-0015): single opaque handle ptr to runtime hash table.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "Set") {
            // Set<T> (ADR-0015): single opaque handle ptr to runtime hash set.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "LinkedMap") {
            // LinkedMap<K,V> (ADR-0019): opaque handle ptr to runtime insertion-order map.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "LinkedSet") {
            // LinkedSet<T> (ADR-0019): opaque handle ptr to runtime insertion-order set.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "SortedMap") {
            // SortedMap<K,V> (ADR-0024): opaque handle ptr to runtime sorted map.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if matches!(ty, Ty::Generic(n, _) if n == "SortedSet") {
            // SortedSet<T> (ADR-0024): opaque handle ptr to runtime sorted set.
            self.adt_struct_defs.insert(
                mono_name,
                vec![("handle".into(), Ty::String)], // ptr in LLVM
            );
        } else if let Some(elem_ty) = ty.list_elem() {
            // List<T> = { data: ptr, len: Int } (§11)
            // data is a heap-allocated array of T. We use Ty::String as a proxy for
            // "pointer type" since String → ptr in LLVM IR. TODO: add Ty::Ptr if needed.
            self.adt_struct_defs.insert(
                mono_name,
                vec![
                    ("data".into(), Ty::String), // ptr in LLVM
                    ("len".into(), Ty::Int),
                ],
            );
            // Also register Option<T> for .get() safe access
            let opt_ty = Ty::Generic("Option".into(), vec![elem_ty.clone()]);
            self.register_adt_type(&opt_ty);
        }
    }

    /// Infer the full generic type of a call expression like Some(x), Ok(x), Err(e).
    /// Returns None if not a prelude constructor.
    pub(super) fn infer_adt_call_type(&self, func_name: &str, arg_type: &Ty) -> Option<Ty> {
        match func_name {
            "Some" => Some(Ty::Generic("Option".into(), vec![arg_type.clone()])),
            "Ok" => {
                // Infer from current function return type
                if let Some(err_ty) = self.current_fn_return_type.result_err_type() {
                    Some(Ty::Generic(
                        "Result".into(),
                        vec![arg_type.clone(), err_ty.clone()],
                    ))
                } else {
                    Some(Ty::Generic(
                        "Result".into(),
                        vec![arg_type.clone(), Ty::Named("Error".into())],
                    ))
                }
            }
            "Err" => {
                // Infer from current function return type
                if let Some(ok_ty) = self.current_fn_return_type.result_ok_type() {
                    Some(Ty::Generic(
                        "Result".into(),
                        vec![ok_ty.clone(), arg_type.clone()],
                    ))
                } else {
                    // Function does not return Result — use Int as the Ok placeholder.
                    // Int (i64) has the same IR layout as Unit and avoids emitting an
                    // undefined `Result__Value__String` struct that conflicts with
                    // `Result__Int__String` at use sites.
                    Some(Ty::Generic(
                        "Result".into(),
                        vec![Ty::Int, arg_type.clone()],
                    ))
                }
            }
            _ => None,
        }
    }

    /// Monomorphize a generic function with concrete type arguments (§8.4).
    /// Returns the mangled function name. If not yet monomorphized, creates and
    /// lowers a specialized copy of the function with type parameters substituted.
    pub(super) fn monomorphize(&mut self, fn_name: &str, type_args: &[Ty]) -> Option<String> {
        // Generate mangled name: fn_name__Type1__Type2
        let type_suffix: Vec<String> = type_args.iter().map(|t| t.monomorphized_name()).collect();
        let mangled = format!("{}__{}", fn_name, type_suffix.join("__"));

        // Check cache
        if self.mono_cache.contains(&mangled) {
            return Some(mangled);
        }

        let fn_def = self.fn_defs.get(fn_name)?.clone();
        if fn_def.type_params.len() != type_args.len() {
            eprintln!(
                "warning: turbofish on '{}' expects {} type args, got {}",
                fn_name,
                fn_def.type_params.len(),
                type_args.len()
            );
            return None;
        }

        // Build substitution map: type_param_name → concrete Ty
        let subst: std::collections::HashMap<String, Ty> = fn_def
            .type_params
            .iter()
            .zip(type_args.iter())
            .map(|(tp, ty)| (tp.name.clone(), ty.clone()))
            .collect();

        // Cache before lowering (prevents infinite recursion)
        self.mono_cache.insert(mangled.clone());

        // Create substituted FnDef
        let mono_def = substitute_fn_def(&fn_def, &subst, &mangled);

        // Register return type
        let ret_ty = mono_def
            .return_type
            .as_ref()
            .map(Ty::from_type_expr)
            .unwrap_or(Ty::Unit);
        self.fn_return_types.insert(mangled.clone(), ret_ty);

        // Save per-function state before re-entrant lower_fn call.
        // lower_fn clears these fields, which would corrupt the caller's state.
        let saved_var_types = self.var_types.clone();
        let saved_float_vars = self.float_vars.clone();
        let saved_string_vars = self.string_vars.clone();
        let saved_mut_vars = self.mut_vars.clone();
        let saved_generic_var_types = self.generic_var_types.clone();
        let saved_deferred_exprs = self.deferred_exprs.clone();
        let saved_return_type = self.current_fn_return_type.clone();

        let func = self.lower_fn(&mono_def);
        self.functions.push(func);

        // Restore caller's per-function state
        self.var_types = saved_var_types;
        self.float_vars = saved_float_vars;
        self.string_vars = saved_string_vars;
        self.mut_vars = saved_mut_vars;
        self.generic_var_types = saved_generic_var_types;
        self.deferred_exprs = saved_deferred_exprs;
        self.current_fn_return_type = saved_return_type;

        Some(mangled)
    }
}
