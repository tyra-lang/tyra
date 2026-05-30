// AST to MIR lowering.
//
// Walks the AST and produces a flat sequence of MIR instructions.
// Expressions are flattened into named temporaries.
// Control flow is desugared into labels and branches.
#![allow(clippy::collapsible_if, clippy::collapsible_else_if)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::unnecessary_map_or)]

mod adt;
mod call;
mod expr;
mod match_lower;
mod method;
mod propagate;
mod types;

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

/// Lower a source file to MIR.
pub fn lower(file: &SourceFile, sources: &tyra_diagnostics::SourceMap) -> Program {
    let mut ctx = LowerCtx::new(sources);

    let has_explicit_main = file
        .items
        .iter()
        .any(|item| matches!(item, Item::FnDef(f) if f.name == "main"));

    let has_top_level_stmts = file.items.iter().any(|item| matches!(item, Item::Stmt(_)));

    // ADR-0006 Rule 2: fn main and top-level statements are mutually exclusive.
    // The resolver already emitted E0213 for this case; skip MIR lowering here
    // to avoid producing invalid MIR with duplicate main functions.
    if has_explicit_main && has_top_level_stmts {
        return Program {
            functions: vec![],
            string_constants: vec![],
            struct_defs: vec![],
            source_files: vec![],
            lower_errors: vec![],
        };
    }

    // Collect type definitions for ADT tag assignment and value field info
    for item in &file.items {
        match item {
            Item::TypeDef(t) => {
                if let TypeDefKind::Adt(variants) = &t.kind {
                    // Per-variant field layout: each variant's fields get their own slots
                    // starting after the tag. This avoids LLVM type conflicts when variants
                    // have incompatible field types (e.g., struct vs ptr).
                    let mut struct_fields: Vec<(String, Ty)> = vec![("tag".into(), Ty::Int)];

                    for (i, variant) in variants.iter().enumerate() {
                        ctx.variant_tags
                            .insert((t.name.clone(), variant.name.clone()), i as i64);

                        let vfields: Vec<(String, Ty)> = variant
                            .fields
                            .iter()
                            .map(|f| (f.name.clone(), Ty::from_type_expr(&f.type_annotation)))
                            .collect();
                        ctx.adt_variant_fields
                            .insert((t.name.clone(), variant.name.clone()), vfields.clone());

                        // Record the starting slot for this variant (1-based struct field index).
                        let offset = struct_fields.len();
                        ctx.variant_field_offsets
                            .insert((t.name.clone(), variant.name.clone()), offset);

                        // Append this variant's fields as separate slots.
                        struct_fields.extend(vfields);
                    }

                    // Register struct def only when there are payload fields.
                    if struct_fields.len() > 1 {
                        ctx.adt_struct_defs.insert(t.name.clone(), struct_fields);
                    }
                }
            }
            Item::ValueDef(v) => {
                let fields: Vec<(String, Ty)> = v
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), Ty::from_type_expr(&f.type_annotation)))
                    .collect();
                ctx.struct_fields.insert(v.name.clone(), fields);
            }
            Item::DataDef(d) => {
                // Data types use the same struct representation as value types.
                // Reference semantics (GC-managed pointers) deferred to later milestone.
                let fields: Vec<(String, Ty)> = d
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), Ty::from_type_expr(&f.type_annotation)))
                    .collect();
                ctx.struct_fields.insert(d.name.clone(), fields);
                ctx.data_types.insert(d.name.clone());
            }
            _ => {}
        }
    }

    // Collect imported module names for module-qualified call resolution (§13)
    for item in &file.items {
        if let Item::Import(imp) = item {
            let local_name = imp
                .alias
                .as_deref()
                .or_else(|| imp.path.last().map(String::as_str))
                .unwrap_or("_unknown");
            ctx.imported_modules.insert(local_name.to_string());

            // Register the canonical module path too so aliased imports
            // (e.g. `import http.server as srv`) can still be recognized
            // by passes that care about the underlying module identity
            // (the M11 phase 2 http.server safety gate is the current
            // consumer). Inserting both the alias and the dotted path is
            // cheap and avoids a separate set.
            let module_key: String = imp.path.join(".");
            ctx.imported_modules.insert(module_key.clone());

            // Track local_name → canonical_name (last path segment) for
            // alias-aware special-case dispatch (e.g. assert.eq).
            let canonical = imp.path.last().cloned().unwrap_or_default();
            ctx.module_local_to_canonical
                .insert(local_name.to_string(), canonical);

            // Register built-in module types
            if module_key == "core.sys" {
                // sys.args() -> List<String>
                let list_string = Ty::Generic("List".into(), vec![Ty::String]);
                ctx.register_adt_type(&list_string);
                ctx.fn_return_types.insert("sys__args".into(), list_string);
                // sys.exit(_ code: Int) -> Never
                ctx.fn_return_types.insert("sys__exit".into(), Ty::Never);
            }
            // M10 phase 1: __fs_read_raw / __fs_errno are registered below
            // (outside the import loop) since they are prelude-level, not
            // tied to any module import.
            // core.tasks: tasks.join_all is handled as identity in call lowering.
            // No fn_return_types entry needed; the list arg type is propagated directly.
        }
    }

    // §18.8: bench clock intrinsic (v0.4.0).
    ctx.fn_return_types
        .insert("__bench_clock_ns".into(), Ty::Int);
    ctx.fn_param_types.insert("__bench_clock_ns".into(), vec![]);

    // M10 phase 1: fs stdlib intrinsics. Registered unconditionally so that
    // `stdlib/fs.tyra` can call them without an `import` (no circularity).
    ctx.fn_return_types
        .insert("__fs_read_raw".into(), Ty::String);
    ctx.fn_param_types
        .insert("__fs_read_raw".into(), vec![Ty::String]);
    ctx.fn_return_types.insert("__fs_errno".into(), Ty::Int);
    ctx.fn_param_types.insert("__fs_errno".into(), vec![]);
    ctx.fn_return_types.insert("__fs_errmsg".into(), Ty::String);
    ctx.fn_param_types.insert("__fs_errmsg".into(), vec![]);
    ctx.fn_return_types
        .insert("__fs_write_raw".into(), Ty::Unit);
    ctx.fn_param_types
        .insert("__fs_write_raw".into(), vec![Ty::String, Ty::String]);
    ctx.fn_return_types.insert("__fs_exists".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__fs_exists".into(), vec![Ty::String]);

    // M11 phase 1: http client intrinsics.
    ctx.fn_return_types.insert("__http_get".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__http_get".into(), vec![Ty::String]);
    ctx.fn_return_types.insert("__http_status".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__http_status".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__http_body".into(), Ty::String);
    ctx.fn_param_types
        .insert("__http_body".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__http_errno".into(), Ty::Int);
    ctx.fn_param_types.insert("__http_errno".into(), vec![]);
    ctx.fn_return_types
        .insert("__http_errmsg".into(), Ty::String);
    ctx.fn_param_types.insert("__http_errmsg".into(), vec![]);
    // M11 phase 2: http server intrinsics.
    ctx.fn_return_types
        .insert("__http_server_new".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__http_server_new".into(), vec![]);
    ctx.fn_return_types
        .insert("__http_server_route".into(), Ty::Unit);
    ctx.fn_param_types.insert(
        "__http_server_route".into(),
        vec![Ty::Int, Ty::String, Ty::String, Ty::String],
    );
    ctx.fn_return_types
        .insert("__http_server_listen".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__http_server_listen".into(), vec![Ty::Int, Ty::Int]);

    // M10 phase 2: json stdlib intrinsics.
    ctx.fn_return_types.insert("__json_parse".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__json_parse".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__json_err_msg".into(), Ty::String);
    ctx.fn_param_types.insert("__json_err_msg".into(), vec![]);
    ctx.fn_return_types
        .insert("__json_err_line".into(), Ty::Int);
    ctx.fn_param_types.insert("__json_err_line".into(), vec![]);
    ctx.fn_return_types.insert("__json_err_col".into(), Ty::Int);
    ctx.fn_param_types.insert("__json_err_col".into(), vec![]);
    ctx.fn_return_types.insert("__json_kind".into(), Ty::String);
    ctx.fn_param_types
        .insert("__json_kind".into(), vec![Ty::Int]);
    ctx.fn_return_types
        .insert("__json_is_string".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__json_is_string".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__json_is_int".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__json_is_int".into(), vec![Ty::Int]);
    ctx.fn_return_types
        .insert("__json_is_bool".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__json_is_bool".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__json_str".into(), Ty::String);
    ctx.fn_param_types
        .insert("__json_str".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__json_int".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__json_int".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__json_bool".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__json_bool".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__json_get".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__json_get".into(), vec![Ty::Int, Ty::String]);
    ctx.fn_return_types.insert("__json_at".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__json_at".into(), vec![Ty::Int, Ty::Int]);

    // stdin intrinsics. See runtime/src/stdlib_io.rs.
    ctx.fn_return_types
        .insert("__io_read_line".into(), Ty::String);
    ctx.fn_param_types.insert("__io_read_line".into(), vec![]);
    ctx.fn_return_types
        .insert("__io_read_to_end".into(), Ty::String);
    ctx.fn_param_types.insert("__io_read_to_end".into(), vec![]);
    ctx.fn_return_types.insert("__io_eof".into(), Ty::Bool);
    ctx.fn_param_types.insert("__io_eof".into(), vec![]);

    // §17.3.4: string stdlib intrinsics.
    ctx.fn_return_types.insert("__string_len".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__string_len".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_is_empty".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__string_is_empty".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_trim".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_trim".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_to_upper".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_to_upper".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_to_lower".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_to_lower".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_contains".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__string_contains".into(), vec![Ty::String, Ty::String]);
    ctx.fn_return_types
        .insert("__string_starts_with".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__string_starts_with".into(), vec![Ty::String, Ty::String]);
    ctx.fn_return_types
        .insert("__string_ends_with".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__string_ends_with".into(), vec![Ty::String, Ty::String]);
    ctx.fn_return_types
        .insert("__string_parse_int".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__string_parse_int".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_parse_errno".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__string_parse_errno".into(), vec![]);
    ctx.fn_return_types
        .insert("__string_byte_at".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__string_byte_at".into(), vec![Ty::String, Ty::Int]);
    ctx.fn_return_types
        .insert("__string_substring".into(), Ty::String);
    ctx.fn_param_types.insert(
        "__string_substring".into(),
        vec![Ty::String, Ty::Int, Ty::Int],
    );
    ctx.fn_return_types
        .insert("__string_reverse".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_reverse".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_from_byte".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_from_byte".into(), vec![Ty::Int]);
    let list_string = Ty::Generic("List".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_split_whitespace".into(), list_string.clone());
    ctx.fn_param_types
        .insert("__string_split_whitespace".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__string_split".into(), list_string.clone());
    ctx.fn_param_types
        .insert("__string_split".into(), vec![Ty::String, Ty::String]);
    ctx.fn_return_types
        .insert("__string_replace".into(), Ty::String);
    ctx.fn_param_types.insert(
        "__string_replace".into(),
        vec![Ty::String, Ty::String, Ty::String],
    );
    ctx.fn_return_types
        .insert("__string_join".into(), Ty::String);
    ctx.fn_param_types
        .insert("__string_join".into(), vec![list_string, Ty::String]);
    // §17.3.x: float stdlib intrinsics.
    ctx.fn_return_types.insert("__float_eq".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__float_eq".into(), vec![Ty::Float, Ty::Float]);
    ctx.fn_return_types
        .insert("__float_approx_eq".into(), Ty::Bool);
    ctx.fn_param_types.insert(
        "__float_approx_eq".into(),
        vec![Ty::Float, Ty::Float, Ty::Float],
    );
    ctx.fn_return_types.insert("__float_abs".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_abs".into(), vec![Ty::Float]);
    ctx.fn_return_types
        .insert("__float_floor".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_floor".into(), vec![Ty::Float]);
    ctx.fn_return_types.insert("__float_ceil".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_ceil".into(), vec![Ty::Float]);
    ctx.fn_return_types
        .insert("__float_round".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_round".into(), vec![Ty::Float]);
    ctx.fn_return_types.insert("__float_min".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_min".into(), vec![Ty::Float, Ty::Float]);
    ctx.fn_return_types.insert("__float_max".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_max".into(), vec![Ty::Float, Ty::Float]);
    ctx.fn_return_types
        .insert("__float_to_string".into(), Ty::String);
    ctx.fn_param_types
        .insert("__float_to_string".into(), vec![Ty::Float]);
    ctx.fn_return_types
        .insert("__float_parse".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_parse".into(), vec![Ty::String]);
    ctx.fn_return_types
        .insert("__float_parse_errno".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__float_parse_errno".into(), vec![]);
    ctx.fn_return_types
        .insert("__float_from_int".into(), Ty::Float);
    ctx.fn_param_types
        .insert("__float_from_int".into(), vec![Ty::Int]);
    ctx.fn_return_types.insert("__float_to_int".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__float_to_int".into(), vec![Ty::Float]);
    ctx.fn_return_types
        .insert("__float_is_nan".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__float_is_nan".into(), vec![Ty::Float]);
    ctx.fn_return_types
        .insert("__float_is_infinite".into(), Ty::Bool);
    ctx.fn_param_types
        .insert("__float_is_infinite".into(), vec![Ty::Float]);
    // §17.3.6 Map<K,V> generic intrinsics (ADR-0015).
    // Register common K/V combos upfront; additional combos are registered
    // lazily via register_map_intrinsics() when a MapLit is lowered.
    for k in &["String", "Int", "Bool"] {
        for v in &["String", "Int", "Bool"] {
            ctx.register_map_intrinsics(k, v);
        }
    }

    // §17.3.5: list stdlib intrinsics (List<Int> only).
    let list_int = Ty::Generic("List".into(), vec![Ty::Int]);
    let opt_int = Ty::Generic("Option".into(), vec![Ty::Int]);
    ctx.fn_return_types
        .insert("__list_int_push".into(), list_int.clone());
    ctx.fn_param_types
        .insert("__list_int_push".into(), vec![list_int.clone(), Ty::Int]);
    ctx.fn_return_types.insert("__list_int_sum".into(), Ty::Int);
    ctx.fn_param_types
        .insert("__list_int_sum".into(), vec![list_int.clone()]);
    ctx.fn_return_types
        .insert("__list_int_max".into(), opt_int.clone());
    ctx.fn_param_types
        .insert("__list_int_max".into(), vec![list_int.clone()]);
    ctx.fn_return_types
        .insert("__list_int_min".into(), opt_int.clone());
    ctx.fn_param_types
        .insert("__list_int_min".into(), vec![list_int.clone()]);
    ctx.fn_return_types
        .insert("__list_int_contains".into(), Ty::Bool);
    ctx.fn_param_types.insert(
        "__list_int_contains".into(),
        vec![list_int.clone(), Ty::Int],
    );
    ctx.fn_return_types
        .insert("__list_int_index_of".into(), opt_int);
    ctx.fn_param_types
        .insert("__list_int_index_of".into(), vec![list_int, Ty::Int]);

    // §17.3.5 Phase C: list.map / list.filter / list.fold intrinsics.
    // register_adt_type is intentionally omitted; call.rs registers struct
    // types lazily when the intrinsic is actually called in the program.
    {
        let li = Ty::Generic("List".into(), vec![Ty::Int]);
        let ls = Ty::Generic("List".into(), vec![Ty::String]);
        ctx.fn_return_types
            .insert("__list_map_int".into(), li.clone());
        ctx.fn_return_types
            .insert("__list_filter_int".into(), li.clone());
        ctx.fn_return_types
            .insert("__list_fold_int".into(), Ty::Int);
        ctx.fn_return_types
            .insert("__list_map_str".into(), ls.clone());
        ctx.fn_return_types
            .insert("__list_filter_str".into(), ls.clone());
        ctx.fn_return_types
            .insert("__list_fold_str".into(), Ty::String);
    }

    // Collect function return types and store definitions for monomorphization
    for item in &file.items {
        if let Item::FnDef(f) = item {
            let ret_ty = f
                .return_type
                .as_ref()
                .map(Ty::from_type_expr)
                .unwrap_or(Ty::Unit);
            ctx.fn_return_types.insert(f.name.clone(), ret_ty);
            let param_tys: Vec<Ty> = f
                .params
                .iter()
                .map(|p| Ty::from_type_expr(&p.type_annotation))
                .collect();
            ctx.fn_param_types.insert(f.name.clone(), param_tys);
            // Store generic function definitions for turbofish monomorphization (§8.4)
            if !f.type_params.is_empty() {
                ctx.fn_defs.insert(f.name.clone(), f.clone());
            }
        }
    }

    // Collect impl block methods for method dispatch (§8.7)
    for item in &file.items {
        if let Item::ImplDef(impl_def) = item {
            if let TypeExprKind::Named(target_name) = &impl_def.target_type.kind {
                for method in &impl_def.methods {
                    let mangled = format!("{target_name}__{}", method.name);
                    let ret_ty = method
                        .return_type
                        .as_ref()
                        .map(Ty::from_type_expr)
                        .unwrap_or(Ty::Unit);
                    ctx.fn_return_types.insert(mangled.clone(), ret_ty);
                    ctx.impl_methods
                        .insert((target_name.clone(), method.name.clone()), mangled);
                }
            }
        }
    }

    // Lower function definitions
    for item in &file.items {
        if let Item::FnDef(f) = item {
            let mut func = ctx.lower_fn(f);
            if f.name == "main" {
                func.is_main = true;
            }
            ctx.functions.push(func);
        }
    }

    // Lower impl method definitions as mangled functions (§8.7, static dispatch)
    for item in &file.items {
        if let Item::ImplDef(impl_def) = item {
            if let TypeExprKind::Named(target_name) = &impl_def.target_type.kind {
                for method in &impl_def.methods {
                    let func = ctx.lower_impl_method(method, target_name);
                    ctx.functions.push(func);
                }
            }
        }
    }

    // Lower top-level statements into an implicit main (§6.1).
    // The per-function state populated by prior `lower_fn` calls (match
    // pattern_vars, mut_vars, string/float trackers, etc.) must be
    // reset first — otherwise a pattern-bound name from a user function
    // (e.g. `when Rectangle(width: w, height: h)` inside `fn area`)
    // leaks into top-level scope, and a `let w = ...` at main level is
    // mis-classified as alloca-backed. Ident references then emit spurious
    // `Load i64, ptr %w` against the already-Copy'd struct SSA, tripping
    // E0500 `'%X' defined with type 'struct.Y' but expected 'ptr'`.
    if has_top_level_stmts {
        ctx.var_types.clear();
        ctx.float_vars.clear();
        ctx.string_vars.clear();
        ctx.mut_vars.clear();
        ctx.pattern_vars.clear();
        ctx.local_binding_names.clear();
        ctx.generic_var_types.clear();
        ctx.deferred_exprs.clear();
        let mut body = Vec::new();
        // Hoist pattern-binding allocas (see lower_fn for rationale).
        let top_stmts: Vec<Stmt> = file
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Stmt(s) = item {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut pattern_bindings: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        collect_pattern_bindings_in_stmts(&top_stmts, &mut pattern_bindings);
        let mut let_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        collect_let_binding_counts_in_stmts(&top_stmts, &mut let_counts);
        for name in &pattern_bindings {
            *let_counts.entry(name.clone()).or_insert(0) += 1;
        }
        for name in &pattern_bindings {
            if ctx.mut_vars.contains(name) {
                continue;
            }
            ctx.emit(&mut body, Instruction::Alloca { dest: name.clone() });
            ctx.pattern_vars.insert(name.clone());
            ctx.local_binding_names.insert(name.clone());
        }
        for (name, count) in &let_counts {
            if *count > 1 && !ctx.pattern_vars.contains(name) && !ctx.mut_vars.contains(name) {
                ctx.emit(&mut body, Instruction::Alloca { dest: name.clone() });
                ctx.pattern_vars.insert(name.clone());
                ctx.local_binding_names.insert(name.clone());
            }
        }
        for item in &file.items {
            if let Item::Stmt(s) = item {
                ctx.lower_stmt(s, &mut body);
            }
        }
        // spec §12.3: emit deferred expressions before implicit main return
        ctx.emit_deferred(&mut body);
        ctx.emit(&mut body, Instruction::Return { value: None });

        ctx.functions.push(Function {
            name: "main".into(),
            params: vec![],
            return_type: Ty::Unit,
            body,
            is_main: true,
            local_metas: vec![],
        });
    }

    let mut struct_defs: Vec<crate::ir::StructDef> = ctx
        .struct_fields
        .iter()
        .map(|(name, fields)| crate::ir::StructDef {
            name: name.clone(),
            fields: fields.clone(),
            is_data: ctx.data_types.contains(name),
            recursive_fields: vec![false; fields.len()],
        })
        .collect();

    // Add ADT struct defs (monomorphized Option/Result types).
    //
    // An ADT field is "recursive" when its declared type is the same
    // named ADT as the enclosing struct. Recursive fields are boxed as
    // GC-heap ptrs by codegen to avoid an otherwise-infinite LLVM
    // struct layout (§8.5 recursive ADTs — Tree / Expr / IntList).
    for (name, fields) in &ctx.adt_struct_defs {
        let recursive_fields: Vec<bool> = fields
            .iter()
            .map(|(_, ty)| matches!(ty, Ty::Named(n) if n == name))
            .collect();
        struct_defs.push(crate::ir::StructDef {
            name: name.clone(),
            fields: fields.clone(),
            is_data: false, // ADTs are not data types
            recursive_fields,
        });
    }

    // Add closure env struct defs (ADR-0011): __closure_env_N structs emitted
    // during lambda lowering. These are data types (is_data = true) so that
    // FieldGet / FieldSet use the GEP+load/store path in codegen.
    for sd in ctx.closure_struct_defs {
        struct_defs.push(sd);
    }

    Program {
        functions: ctx.functions,
        string_constants: ctx.string_constants,
        struct_defs,
        source_files: ctx.source_files,
        lower_errors: ctx.lower_errors,
    }
}

pub(crate) struct LowerCtx<'a> {
    /// Source map for converting byte-offset spans to (line, col) pairs (ADR 0014).
    pub(crate) source_map: &'a tyra_diagnostics::SourceMap,
    /// Maps SourceId → MIR file_id (index into source_files). Built lazily.
    pub(crate) source_id_map: std::collections::HashMap<tyra_diagnostics::SourceId, u32>,
    /// File names in file_id order; transferred to Program::source_files at the end.
    pub(crate) source_files: Vec<String>,
    pub(crate) functions: Vec<Function>,
    pub(crate) string_constants: Vec<String>,
    pub(crate) temp_counter: u32,
    pub(crate) label_counter: u32,
    /// ADT variant tag map: (type_name, variant_name) -> tag index
    pub(crate) variant_tags: std::collections::HashMap<(String, String), i64>,
    /// Struct field info for value and data types: type_name -> list of (field_name, field_type)
    pub(crate) struct_fields: std::collections::HashMap<String, Vec<(String, Ty)>>,
    /// Set of type names that are data types (reference semantics, §8.6).
    pub(crate) data_types: std::collections::HashSet<String>,
    /// Tracks variable/temp → struct type name mapping for correct type resolution
    pub(crate) var_types: std::collections::HashMap<String, String>,
    /// Tracks variables/temps known to hold Float values (for correct binop selection)
    pub(crate) float_vars: std::collections::HashSet<String>,
    /// Tracks variables/temps known to hold String values (for interpolation type detection)
    pub(crate) string_vars: std::collections::HashSet<String>,
    /// Tracks mutable local variables (use alloca/store/load instead of SSA copy)
    pub(crate) mut_vars: std::collections::HashSet<String>,
    /// Tracks pattern-bound variables (alloca-backed but semantically immutable).
    /// Kept separate from mut_vars so future immutability checks are not confused.
    pub(crate) pattern_vars: std::collections::HashSet<String>,
    /// Tracks EVERY local binding name introduced in the current function,
    /// regardless of the binding's type (Int/Bool/Unit for-loop induction
    /// vars would otherwise fall through the type-keyed maps above). Used
    /// as the authoritative "is this Ident a local shadow?" signal for
    /// the M11 phase 2 http.server handler gate. Populated by:
    ///   - function parameters (in `lower_fn`);
    ///   - the synthesized `self` on impl methods (conditional in
    ///     `lower_fn` when `self_type.is_some()`);
    ///   - `Stmt::Let` and `Stmt::Mut` bindings;
    ///   - match pattern bindings (alongside `pattern_vars.insert`);
    ///   - for-loop induction variables (both list-iter and fallback paths).
    /// Anything that introduces a local name must also insert here.
    pub(crate) local_binding_names: std::collections::HashSet<String>,
    /// Function return type registry: fn_name → return_type (for type inference in interpolation)
    pub(crate) fn_return_types: std::collections::HashMap<String, Ty>,
    /// Function parameter type registry (M9): fn_name → parameter types in
    /// declaration order. Populated for ALL functions (unlike fn_defs which
    /// only stores generics) so `spawn f(args)` can emit typed arg boxes.
    pub(crate) fn_param_types: std::collections::HashMap<String, Vec<Ty>>,
    /// Temporaries that hold a live Task<T> handle (M9 real thread-pool — §14).
    /// `.await` uses this to decide whether to emit an `Await` instruction.
    /// Kept separate from `generic_var_types` so downstream code (propagate,
    /// match, list ops) continues to see the underlying T for type lookups.
    pub(crate) task_result_types: std::collections::HashMap<String, Ty>,
    /// Impl method registry: (target_type_name, method_name) → mangled_fn_name
    pub(crate) impl_methods: std::collections::HashMap<(String, String), String>,
    /// Imported module names for module-qualified call resolution (§13)
    pub(crate) imported_modules: std::collections::HashSet<String>,
    /// local_name → canonical_name for alias import support.
    /// e.g. `import assert as a` → "a" → "assert".
    pub(crate) module_local_to_canonical: std::collections::HashMap<String, String>,
    /// Current self type when lowering impl method bodies (None outside impl methods)
    pub(crate) self_type: Option<String>,
    /// Tracks variables/temps with generic types (Option<T>, Result<T, E>) for ADT lowering
    pub(crate) generic_var_types: std::collections::HashMap<String, Ty>,
    /// ADT variant field definitions: (type_name, variant_name) → [(field_name, field_type)]
    pub(crate) adt_variant_fields: std::collections::HashMap<(String, String), Vec<(String, Ty)>>,
    /// Per-variant slot offset: (type_name, variant_name) → first struct field index (1-based)
    /// Each variant's fields occupy consecutive slots starting at this offset.
    pub(crate) variant_field_offsets: std::collections::HashMap<(String, String), usize>,
    /// Return type of the function currently being lowered (for ? operator)
    pub(crate) current_fn_return_type: Ty,
    /// Stack of loop-exit labels for `break` lowering. Each while/for body
    /// push its end_label here; `break` emits a jump to the top of the stack.
    pub(crate) loop_exit_stack: Vec<String>,
    /// Stack of loop-head (condition-check) labels for `continue` lowering.
    /// Parallel to loop_exit_stack; `continue` jumps to the top of this stack.
    pub(crate) loop_head_stack: Vec<String>,
    /// Active type hint from a `let x: T = ...` / `mut x: T = ...`
    /// annotation, used to type context-sensitive RHS expressions like
    /// a bare `None` (would otherwise default to `Option<Int>`).
    pub(crate) binding_type_hint: Option<Ty>,
    /// Collected ADT struct defs (monomorphized Option/Result types)
    pub(crate) adt_struct_defs: std::collections::HashMap<String, Vec<(String, Ty)>>,
    /// Deferred expressions for the current function (spec §12.3, LIFO
    /// execution). Each entry pairs the deferred expression with the name
    /// of a runtime bool alloca that tracks whether the `defer` statement
    /// was actually reached — emit_deferred only executes expressions whose
    /// activation flag is `true`. The allocas are created up front at
    /// function entry (see `collect_defer_sites`) so flags dominate every
    /// return path.
    pub(crate) deferred_exprs: Vec<(String, Expr)>,
    /// Running ordinal that matches `defer` statements with their
    /// pre-allocated activation flag (`.defer_active_N`). Reset at the
    /// start of each function; incremented on every `Stmt::Defer` during
    /// body lowering in the same walk order as `count_defer_sites_in_stmts`.
    pub(crate) next_defer_index: usize,
    /// Total number of `.defer_active_*` flags pre-allocated for the
    /// current function. Used to assert `next_defer_index` stays in bounds.
    pub(crate) defer_flag_count: usize,
    /// Generic function definitions for monomorphization (§8.4).
    pub(crate) fn_defs: std::collections::HashMap<String, FnDef>,
    /// Monomorphization cache: mangled_name → true.
    pub(crate) mono_cache: std::collections::HashSet<String>,
    /// Counter for generating unique `__lambda_N` / `__closure_env_N` names (ADR-0011).
    pub(crate) closure_counter: u32,
    /// Variables/temps whose LLVM type is `i1` (Bool).
    /// Needed to correctly type Bool captures in lambda env structs (Fix 2).
    pub(crate) bool_vars: std::collections::HashSet<String>,
    /// Temporaries/variables that hold closure fat-pointer values (ADR-0011).
    /// Used to emit IndirectCall instead of Call at closure call sites.
    pub(crate) closure_vars: std::collections::HashSet<String>,
    /// Maps closure-valued temp/variable names to their `Ty::Fn(params, ret)` type.
    /// Propagated through let/mut bindings so call sites can emit typed IndirectCall.
    pub(crate) closure_fn_types: std::collections::HashMap<String, Ty>,
    /// Env struct StructDefs accumulated while lowering lambdas (ADR-0011).
    /// Collected into Program::struct_defs at the end of `lower()`.
    pub(crate) closure_struct_defs: Vec<crate::ir::StructDef>,
    /// Source location of the AST node currently being lowered (ADR 0014).
    /// Updated at the start of each statement lowering.
    pub(crate) current_loc: crate::ir::SourceLoc,
    /// Diagnostics accumulated during MIR lowering (e.g. E0204 for unknown
    /// stdlib methods).  Transferred to Program::lower_errors so the driver
    /// can forward them to its Report and trigger hard-error propagation.
    pub(crate) lower_errors: Vec<tyra_diagnostics::Diagnostic>,
}

/// Result of resolving an impl method call.
pub(crate) enum ImplMethodResult {
    /// Resolved to a mangled function name.
    Resolved(String),
    /// Multiple impls define this method; can't disambiguate without type info.
    Ambiguous,
    /// No impl found for this method name.
    NotFound,
}

impl<'a> LowerCtx<'a> {
    fn new(source_map: &'a tyra_diagnostics::SourceMap) -> Self {
        Self {
            source_map,
            source_id_map: std::collections::HashMap::new(),
            source_files: Vec::new(),
            functions: Vec::new(),
            string_constants: Vec::new(),
            temp_counter: 0,
            label_counter: 0,
            variant_tags: std::collections::HashMap::new(),
            struct_fields: std::collections::HashMap::new(),
            data_types: std::collections::HashSet::new(),
            var_types: std::collections::HashMap::new(),
            float_vars: std::collections::HashSet::new(),
            string_vars: std::collections::HashSet::new(),
            mut_vars: std::collections::HashSet::new(),
            pattern_vars: std::collections::HashSet::new(),
            local_binding_names: std::collections::HashSet::new(),
            fn_return_types: std::collections::HashMap::new(),
            fn_param_types: std::collections::HashMap::new(),
            task_result_types: std::collections::HashMap::new(),
            imported_modules: std::collections::HashSet::new(),
            module_local_to_canonical: std::collections::HashMap::new(),
            impl_methods: std::collections::HashMap::new(),
            self_type: None,
            generic_var_types: std::collections::HashMap::new(),
            adt_variant_fields: std::collections::HashMap::new(),
            variant_field_offsets: std::collections::HashMap::new(),
            current_fn_return_type: Ty::Unit,
            loop_exit_stack: Vec::new(),
            loop_head_stack: Vec::new(),
            binding_type_hint: None,
            adt_struct_defs: std::collections::HashMap::new(),
            deferred_exprs: Vec::new(),
            next_defer_index: 0,
            defer_flag_count: 0,
            fn_defs: std::collections::HashMap::new(),
            mono_cache: std::collections::HashSet::new(),
            bool_vars: std::collections::HashSet::new(),
            closure_counter: 0,
            closure_vars: std::collections::HashSet::new(),
            closure_fn_types: std::collections::HashMap::new(),
            closure_struct_defs: Vec::new(),
            current_loc: crate::ir::SourceLoc::dummy(),
            lower_errors: Vec::new(),
        }
    }

    /// Emit an instruction tagged with the current source location.
    /// All lowering code should use this instead of self.emit(body, Instruction::...)
    /// so that instructions carry accurate source positions (ADR 0014).
    #[inline]
    pub(crate) fn emit(&self, body: &mut Vec<MirStmt>, instr: Instruction) {
        body.push(MirStmt::new(self.current_loc, instr));
    }

    /// Emit an instruction with an explicit source location, ignoring `current_loc`.
    /// Use for the "result" instruction of a non-leaf expression, so the location
    /// of the outer expression (not its last child) is recorded (ADR 0014).
    #[inline]
    pub(crate) fn emit_at(
        &self,
        body: &mut Vec<MirStmt>,
        loc: crate::ir::SourceLoc,
        instr: Instruction,
    ) {
        body.push(MirStmt::new(loc, instr));
    }

    /// Emit a compiler-synthesized instruction with no source position.
    /// Use for control-flow glue (Label, Jump, BranchIf, Phi), alloca
    /// result-slots, and implicit returns that have no AST counterpart.
    #[inline]
    pub(crate) fn emit_synthetic(&self, body: &mut Vec<MirStmt>, instr: Instruction) {
        body.push(MirStmt::synthetic(instr));
    }

    /// Convert an AST `Span` to a `SourceLoc`, assigning a new file_id on the
    /// first encounter of each `SourceId` (ADR 0014).
    pub(crate) fn span_to_loc(&mut self, span: tyra_diagnostics::Span) -> crate::ir::SourceLoc {
        use std::collections::hash_map::Entry;
        let file_id = match self.source_id_map.entry(span.source) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let id = self.source_files.len() as u32;
                self.source_files
                    .push(self.source_map.name(span.source).to_owned());
                e.insert(id);
                id
            }
        };
        let (line, col) = self.source_map.line_col(span.source, span.start);
        crate::ir::SourceLoc { file_id, line, col }
    }

    fn fresh_temp(&mut self) -> String {
        let t = format!("_t{}", self.temp_counter);
        self.temp_counter += 1;
        t
    }

    /// Register fn_return_types / fn_param_types for a Map<K,V> combination.
    /// The handle is a raw pointer surfaced as Ty::String (ptr convention).
    pub(super) fn register_map_intrinsics(&mut self, k: &str, v: &str) {
        let new_fn = format!("__map_new__{k}__{v}");
        let insert_fn = format!("__map_insert__{k}__{v}");
        let remove_fn = format!("__map_remove__{k}");
        let contains_fn = format!("__map_contains__{k}");
        // new: () -> ptr (String = ptr hack)
        self.fn_return_types.insert(new_fn.clone(), Ty::String);
        self.fn_param_types.insert(new_fn, vec![]);
        // insert: (ptr, K, V) -> ptr
        self.fn_return_types.insert(insert_fn.clone(), Ty::String);
        self.fn_param_types
            .insert(insert_fn, vec![Ty::String, Ty::String, Ty::String]);
        // remove: (ptr, K) -> ptr
        self.fn_return_types.insert(remove_fn.clone(), Ty::String);
        self.fn_param_types
            .insert(remove_fn, vec![Ty::String, Ty::String]);
        // contains: (ptr, K) -> Bool
        self.fn_return_types.insert(contains_fn.clone(), Ty::Bool);
        self.fn_param_types
            .insert(contains_fn, vec![Ty::String, Ty::String]);
    }

    /// If the last instruction in `body` is an `AdtPayload { field_index: 1 }`
    /// whose `dest` is `temp` (i.e. `temp` was produced by extracting the Ok
    /// payload with `?`) AND `return_type` is `Result<T,E>`, emit
    /// `AdtInit { Ok(temp) }` and return the new temp.  Otherwise `temp` is
    /// already a full Result value and is returned unchanged.
    ///
    /// This handles `fn f() -> Result<T,E> { ... expr? }`: the `?` operator
    /// extracts T in its Ok branch, leaving us with T instead of Result<T,E>.
    fn maybe_wrap_ok_for_return(
        &mut self,
        temp: String,
        return_type: &Ty,
        body: &mut Vec<MirStmt>,
    ) -> String {
        if !return_type.is_result() {
            return temp;
        }
        // Only wrap when the last instruction is AdtPayload field 1 (the ?
        // ok-extract).  Any other instruction means the value is already a
        // full Result struct (Load from alloca, AdtInit, Call, …).
        let is_adt_payload_ok = body.last().map_or(false, |stmt| {
            matches!(
                stmt.instr,
                Instruction::AdtPayload { dest: ref d, field_index: 1, .. } if d == &temp
            )
        });
        if is_adt_payload_ok {
            let ret_type_name = return_type.monomorphized_name();
            self.register_adt_type(return_type);
            let ok_temp = self.fresh_temp();
            self.emit(
                body,
                Instruction::AdtInit {
                    dest: ok_temp.clone(),
                    type_name: ret_type_name,
                    tag: 0,
                    fields: vec![
                        Operand::Var(temp),
                        Operand::Const(Constant::Int(0)), // zero-placeholder for err field
                    ],
                },
            );
            ok_temp
        } else {
            temp
        }
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        let l = format!("{prefix}_{}", self.label_counter);
        self.label_counter += 1;
        l
    }

    fn intern_string(&mut self, s: &str) -> usize {
        if let Some(idx) = self.string_constants.iter().position(|c| c == s) {
            idx
        } else {
            let idx = self.string_constants.len();
            self.string_constants.push(s.to_string());
            idx
        }
    }

    /// Lower a lambda expression into a lifted LLVM function (ADR-0011 §Phase B).
    ///
    /// Saves per-function state, builds a new function whose first parameter is
    /// `__env: ptr` (a pointer to the GC-allocated env struct), lowers the
    /// lambda body with captures loaded from the env struct at entry, then
    /// restores state and pushes the resulting `Function` to `self.functions`.
    ///
    /// `lambda_id` — the unique counter value assigned to this lambda.
    /// `lam`       — the AST lambda expression.
    /// `captures`  — capture names in lexical first-use order.
    /// `env_struct_name` — the `__closure_env_N` struct name (or "" if empty).
    /// `param_types` / `ret_ty` — the lifted function's user-visible signature.
    /// `saved_type_maps` — the enclosing function's type maps, used to recover
    ///   capture types when setting up the lifted function's context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_lifted_lambda(
        &mut self,
        lambda_id: u32,
        lam: &LambdaExpr,
        captures: &[String],
        env_struct_name: &str,
        param_types: &[Ty],
        ret_ty: Ty,
        saved_var_types: &std::collections::HashMap<String, String>,
        saved_float_vars: &std::collections::HashSet<String>,
        saved_string_vars: &std::collections::HashSet<String>,
        saved_bool_vars: &std::collections::HashSet<String>,
        saved_generic_var: &std::collections::HashMap<String, Ty>,
    ) {
        // Save and clear per-function state so the lifted function gets a fresh context.
        let bak_var_types = std::mem::take(&mut self.var_types);
        let bak_float_vars = std::mem::take(&mut self.float_vars);
        let bak_string_vars = std::mem::take(&mut self.string_vars);
        let bak_bool_vars = std::mem::take(&mut self.bool_vars);
        let bak_mut_vars = std::mem::take(&mut self.mut_vars);
        let bak_pattern_vars = std::mem::take(&mut self.pattern_vars);
        let bak_local_names = std::mem::take(&mut self.local_binding_names);
        let bak_generic_var = std::mem::take(&mut self.generic_var_types);
        let bak_deferred = std::mem::take(&mut self.deferred_exprs);
        let bak_next_defer = self.next_defer_index;
        let bak_defer_count = self.defer_flag_count;
        let bak_return_type = self.current_fn_return_type.clone();
        let bak_closure_vars = std::mem::take(&mut self.closure_vars);
        let bak_closure_fn_types = std::mem::take(&mut self.closure_fn_types);

        self.current_fn_return_type = ret_ty.clone();
        self.register_adt_type(&ret_ty);

        // Build params: (__env: ptr to env struct) + user params.
        // Non-capturing lambdas use "" for env_struct_name; Ty::String maps to
        // ptr in LLVM (consistent with "null" env_ptr in ClosureBuild, ADR-0011 §1).
        let env_ty = if env_struct_name.is_empty() {
            Ty::String
        } else {
            Ty::Named(env_struct_name.to_string())
        };
        let mut params: Vec<(String, Ty)> = vec![("__env".into(), env_ty)];
        self.local_binding_names.insert("__env".into());

        for (p, ty) in lam.params.iter().zip(param_types.iter()) {
            self.local_binding_names.insert(p.name.clone());
            self.register_adt_type(ty);
            match ty {
                Ty::Float => {
                    self.float_vars.insert(p.name.clone());
                }
                Ty::String => {
                    self.string_vars.insert(p.name.clone());
                }
                Ty::Bool => {
                    self.bool_vars.insert(p.name.clone());
                }
                Ty::Named(n) => {
                    self.var_types.insert(p.name.clone(), n.clone());
                }
                _ if ty.is_option() || ty.is_result() || ty.is_list() => {
                    self.generic_var_types.insert(p.name.clone(), ty.clone());
                    self.var_types
                        .insert(p.name.clone(), ty.monomorphized_name());
                }
                _ => {}
            }
            params.push((p.name.clone(), ty.clone()));
        }

        let mut body: Vec<MirStmt> = Vec::new();

        // Load each capture from the env struct at function entry (GEP+load).
        // The FieldGet instruction uses data-type semantics because
        // __closure_env_N is registered with is_data=true.
        if !captures.is_empty() {
            for (i, cap_name) in captures.iter().enumerate() {
                self.emit(
                    &mut body,
                    Instruction::FieldGet {
                        dest: cap_name.clone(),
                        obj: Operand::Var("__env".into()),
                        type_name: env_struct_name.to_string(),
                        field_index: i as u32,
                    },
                );
                // Restore the capture's type in the lifted function's type maps.
                if saved_float_vars.contains(cap_name.as_str()) {
                    self.float_vars.insert(cap_name.clone());
                } else if saved_string_vars.contains(cap_name.as_str()) {
                    self.string_vars.insert(cap_name.clone());
                } else if saved_bool_vars.contains(cap_name.as_str()) {
                    self.bool_vars.insert(cap_name.clone());
                } else if let Some(gt) = saved_generic_var.get(cap_name.as_str()).cloned() {
                    self.generic_var_types.insert(cap_name.clone(), gt.clone());
                    self.var_types
                        .insert(cap_name.clone(), gt.monomorphized_name());
                } else if let Some(st) = saved_var_types.get(cap_name.as_str()).cloned() {
                    self.var_types.insert(cap_name.clone(), st);
                }
                self.local_binding_names.insert(cap_name.clone());
            }
        }

        // Lower the lambda body statements (mirrors lower_fn implicit-return logic).
        let mut last_expr_result = None;
        for stmt in &lam.body {
            if let tyra_ast::Stmt::Expr(s) = stmt {
                self.current_loc = self.span_to_loc(s.span);
                last_expr_result = Some(self.lower_expr(&s.expr, &mut body));
            } else {
                last_expr_result = None;
                self.lower_stmt(stmt, &mut body);
            }
        }

        // Ensure the function ends with a Return that carries the value when needed.
        // Mirrors lower_fn: capture last temp BEFORE emitting defers (spec §12.3).
        if !body
            .last()
            .is_some_and(|s| matches!(s.instr, Instruction::Return { .. }))
        {
            let pre_defer_last_temp = self.last_temp_name(&body);
            self.emit_deferred(&mut body);
            if ret_ty == Ty::Unit {
                self.emit(&mut body, Instruction::Return { value: None });
            } else if let Some(expr_val) = last_expr_result {
                // Prioritize last_expr_result: last_temp_name may pick up a
                // void-return call dest that is never defined in LLVM IR.
                let ret_val = self.maybe_wrap_ok_for_return(expr_val, &ret_ty, &mut body);
                self.emit(
                    &mut body,
                    Instruction::Return {
                        value: Some(Operand::Var(ret_val)),
                    },
                );
            } else if let Some(last_temp) = pre_defer_last_temp {
                let ret_val = self.maybe_wrap_ok_for_return(last_temp, &ret_ty, &mut body);
                self.emit(
                    &mut body,
                    Instruction::Return {
                        value: Some(Operand::Var(ret_val)),
                    },
                );
            } else {
                self.emit(&mut body, Instruction::Return { value: None });
            }
        }

        self.functions.push(Function {
            name: format!("__lambda_{lambda_id}"),
            params,
            return_type: ret_ty,
            body,
            is_main: false,
            local_metas: vec![],
        });

        // Restore per-function state.
        self.var_types = bak_var_types;
        self.float_vars = bak_float_vars;
        self.string_vars = bak_string_vars;
        self.bool_vars = bak_bool_vars;
        self.mut_vars = bak_mut_vars;
        self.pattern_vars = bak_pattern_vars;
        self.local_binding_names = bak_local_names;
        self.generic_var_types = bak_generic_var;
        self.deferred_exprs = bak_deferred;
        self.next_defer_index = bak_next_defer;
        self.defer_flag_count = bak_defer_count;
        self.current_fn_return_type = bak_return_type;
        self.closure_vars = bak_closure_vars;
        self.closure_fn_types = bak_closure_fn_types;
    }

    /// Generate a lifted callback function for a Map or Set `for` loop (v0.7.0).
    ///
    /// The callback signature is:
    ///   `fn __map_iter_N(__env: ptr, __b0box: ptr[, __b1box: ptr]) -> void`
    ///
    /// Parameters:
    /// - `iter_id`       — unique counter for this for-each site.
    /// - `is_map`        — true for Map (2 box params), false for Set (1 box param).
    /// - `bindings`      — user-visible binding name(s) ([elem] or [key, val]).
    /// - `binding_tys`   — their Tyra types, used for PtrLoad.
    /// - `body_stmts`    — the for-loop body AST statements.
    /// - `captures`      — enclosing variables captured by the body.
    /// - `env_struct_name` — name of the closure env struct (empty if no captures).
    ///
    /// The generated function:
    ///   1. Loads each captured field from `__env` (GEP+PtrLoad).
    ///   2. Emits `PtrLoad` to unbox each box parameter into the binding name.
    ///   3. Lowers the for-body stmts.
    ///   4. Returns void (Unit).
    ///
    /// The function is appended to `self.functions`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lower_for_each_callback(
        &mut self,
        iter_id: u32,
        is_map: bool,
        fn_prefix: Option<&str>,
        bindings: &[String],
        binding_tys: &[Ty],
        body_stmts: &[Stmt],
        captures: &[String],
        env_struct_name: &str,
        saved_var_types: &std::collections::HashMap<String, String>,
        saved_float_vars: &std::collections::HashSet<String>,
        saved_string_vars: &std::collections::HashSet<String>,
        saved_bool_vars: &std::collections::HashSet<String>,
        saved_generic_var: &std::collections::HashMap<String, Ty>,
    ) {
        // Save and clear per-function state.
        let bak_var_types = std::mem::take(&mut self.var_types);
        let bak_float_vars = std::mem::take(&mut self.float_vars);
        let bak_string_vars = std::mem::take(&mut self.string_vars);
        let bak_bool_vars = std::mem::take(&mut self.bool_vars);
        let bak_mut_vars = std::mem::take(&mut self.mut_vars);
        let bak_pattern_vars = std::mem::take(&mut self.pattern_vars);
        let bak_local_names = std::mem::take(&mut self.local_binding_names);
        let bak_generic_var = std::mem::take(&mut self.generic_var_types);
        let bak_deferred = std::mem::take(&mut self.deferred_exprs);
        let bak_next_defer = self.next_defer_index;
        let bak_defer_count = self.defer_flag_count;
        let bak_return_type = self.current_fn_return_type.clone();
        let bak_closure_vars = std::mem::take(&mut self.closure_vars);
        let bak_closure_fn_types = std::mem::take(&mut self.closure_fn_types);

        self.current_fn_return_type = Ty::Unit;

        // Restore enclosing type context for capture-type reconstruction.
        self.var_types = saved_var_types.clone();
        self.float_vars = saved_float_vars.clone();
        self.string_vars = saved_string_vars.clone();
        self.bool_vars = saved_bool_vars.clone();
        self.generic_var_types = saved_generic_var.clone();

        let prefix = fn_prefix.unwrap_or(if is_map { "__map_iter" } else { "__set_iter" });
        let fn_name = format!("{prefix}_{iter_id}");

        // Build params: __env (ptr) + one or two box params.
        let env_ty = if env_struct_name.is_empty() {
            Ty::String // "ptr" in LLVM
        } else {
            Ty::Named(env_struct_name.to_string())
        };
        let mut params: Vec<(String, Ty)> = vec![("__env".into(), env_ty)];
        self.local_binding_names.insert("__env".into());

        let box_param_names: Vec<String> = if is_map {
            vec!["__kbox".into(), "__vbox".into()]
        } else {
            vec!["__elembox".into()]
        };
        for bp in &box_param_names {
            params.push((bp.clone(), Ty::String)); // ptr
            self.local_binding_names.insert(bp.clone());
        }

        let mut body: Vec<MirStmt> = Vec::new();

        // Load captured fields from env struct (same as lower_lifted_lambda).
        if !captures.is_empty() {
            let env_llvm_name = env_struct_name.to_string();
            for (i, cap_name) in captures.iter().enumerate() {
                // Infer capture type.
                let cap_ty = if self.float_vars.contains(cap_name.as_str()) {
                    Ty::Float
                } else if self.string_vars.contains(cap_name.as_str()) {
                    Ty::String
                } else if self.bool_vars.contains(cap_name.as_str()) {
                    Ty::Bool
                } else if let Some(gt) = self.generic_var_types.get(cap_name.as_str()).cloned() {
                    gt
                } else if let Some(type_name) = self.var_types.get(cap_name.as_str()).cloned() {
                    Ty::Named(type_name)
                } else {
                    Ty::Int
                };
                // Register in appropriate type sets.
                match &cap_ty {
                    Ty::Float => {
                        self.float_vars.insert(cap_name.clone());
                    }
                    Ty::String => {
                        self.string_vars.insert(cap_name.clone());
                    }
                    Ty::Bool => {
                        self.bool_vars.insert(cap_name.clone());
                    }
                    Ty::Named(n) => {
                        self.var_types.insert(cap_name.clone(), n.clone());
                    }
                    Ty::Generic(_, _) => {
                        self.generic_var_types
                            .insert(cap_name.clone(), cap_ty.clone());
                        self.var_types
                            .insert(cap_name.clone(), cap_ty.monomorphized_name());
                    }
                    _ => {}
                }
                self.local_binding_names.insert(cap_name.clone());
                self.emit(
                    &mut body,
                    Instruction::FieldGet {
                        dest: cap_name.clone(),
                        obj: Operand::Var("__env".into()),
                        type_name: env_llvm_name.clone(),
                        field_index: i as u32,
                    },
                );
            }
        }

        // Unbox each box parameter into the user-visible binding name.
        for (i, (binding_name, box_param)) in
            bindings.iter().zip(box_param_names.iter()).enumerate()
        {
            let ty = binding_tys.get(i).cloned().unwrap_or(Ty::Int);
            // Register the binding in type tracking sets.
            match &ty {
                Ty::Float => {
                    self.float_vars.insert(binding_name.clone());
                }
                Ty::String => {
                    self.string_vars.insert(binding_name.clone());
                }
                Ty::Bool => {
                    self.bool_vars.insert(binding_name.clone());
                }
                Ty::Named(n) => {
                    self.var_types.insert(binding_name.clone(), n.clone());
                }
                Ty::Generic(_, _) => {
                    self.generic_var_types
                        .insert(binding_name.clone(), ty.clone());
                    self.var_types
                        .insert(binding_name.clone(), ty.monomorphized_name());
                }
                _ => {}
            }
            self.local_binding_names.insert(binding_name.clone());
            // Emit PtrLoad to unbox.
            self.emit(
                &mut body,
                Instruction::PtrLoad {
                    dest: binding_name.clone(),
                    ptr: box_param.clone(),
                    ty,
                },
            );
        }

        // Lower the for-body.
        for stmt in body_stmts {
            self.lower_stmt(stmt, &mut body);
        }

        // Implicit return void.
        self.emit(&mut body, Instruction::Return { value: None });

        self.functions.push(Function {
            name: fn_name,
            params,
            return_type: Ty::Unit,
            body,
            is_main: false,
            local_metas: vec![],
        });

        // Restore per-function state.
        self.var_types = bak_var_types;
        self.float_vars = bak_float_vars;
        self.string_vars = bak_string_vars;
        self.bool_vars = bak_bool_vars;
        self.mut_vars = bak_mut_vars;
        self.pattern_vars = bak_pattern_vars;
        self.local_binding_names = bak_local_names;
        self.generic_var_types = bak_generic_var;
        self.deferred_exprs = bak_deferred;
        self.next_defer_index = bak_next_defer;
        self.defer_flag_count = bak_defer_count;
        self.current_fn_return_type = bak_return_type;
        self.closure_vars = bak_closure_vars;
        self.closure_fn_types = bak_closure_fn_types;
    }

    /// Lower an impl method as a standalone function with mangled name.
    /// Injects `self` as the first parameter with the target type.
    fn lower_impl_method(&mut self, f: &FnDef, target_type_name: &str) -> Function {
        self.self_type = Some(target_type_name.to_string());
        let mut func = self.lower_fn(f);

        // Inject self as first parameter
        if f.self_param.is_some() {
            let self_ty = Ty::Named(target_type_name.to_string());
            func.params.insert(0, ("self".into(), self_ty));
        }

        // Apply mangled name
        func.name = format!("{target_type_name}__{}", f.name);

        self.self_type = None;
        func
    }

    fn lower_fn(&mut self, f: &FnDef) -> Function {
        // Clear per-function state
        self.var_types.clear();
        self.float_vars.clear();
        self.string_vars.clear();
        self.mut_vars.clear();
        self.pattern_vars.clear();
        self.local_binding_names.clear();
        self.generic_var_types.clear();
        self.deferred_exprs.clear();

        let params: Vec<(String, Ty)> = f
            .params
            .iter()
            .map(|p| (p.name.clone(), Ty::from_type_expr(&p.type_annotation)))
            .collect();

        let return_type = f
            .return_type
            .as_ref()
            .map(Ty::from_type_expr)
            .unwrap_or(Ty::Unit);
        self.current_fn_return_type = return_type.clone();

        // Ensure ADT struct defs are registered for the return type
        self.register_adt_type(&return_type);

        // Register parameter types for correct type resolution. Parameters
        // are also local bindings from the safety-gate's perspective — a
        // user writing `fn setup(_ my_handler: String)` must not have the
        // handler-slot gate mistake the parameter for a top-level function
        // of the same name. Track every param name here regardless of type.
        for (name, _) in &params {
            self.local_binding_names.insert(name.clone());
        }
        // `self` is synthesized by `lower_impl_method` AFTER this function
        // returns (via `func.params.insert(0, ("self", ...))`), so it's
        // absent from `params` above. Keep the local-binding invariant
        // complete by tracking it here when we're lowering an impl method
        // (signaled by `self_type` being set). `self` is a reserved
        // identifier and cannot match a top-level fn name, so this is
        // defense-in-depth rather than a fix for a known exploit.
        if self.self_type.is_some() {
            self.local_binding_names.insert("self".into());
        }
        for (name, ty) in &params {
            // Register ADT struct defs for generic parameter types
            self.register_adt_type(ty);
            if ty.is_option() || ty.is_result() || ty.is_list() {
                self.generic_var_types.insert(name.clone(), ty.clone());
                self.var_types.insert(name.clone(), ty.monomorphized_name());
            }
            match ty {
                Ty::Float => {
                    self.float_vars.insert(name.clone());
                }
                Ty::String => {
                    self.string_vars.insert(name.clone());
                }
                Ty::Bool => {
                    self.bool_vars.insert(name.clone());
                }
                Ty::Named(type_name) => {
                    if self.struct_fields.contains_key(type_name)
                        || self.adt_struct_defs.contains_key(type_name)
                    {
                        self.var_types.insert(name.clone(), type_name.clone());
                    }
                }
                _ => {}
            }
        }

        let mut body: Vec<MirStmt> = Vec::new();

        // Pre-emit allocas for every match-pattern binding reachable in
        // this function. Placing them in the entry block guarantees they
        // dominate every arm (and any later `let`/`mut` shadow). Without
        // the hoist, a pattern binding placed inside a conditional arm
        // fails to dominate sibling matches or subsequent lets that reuse
        // the name, producing E0500 (`multiple definition of local value`
        // / `Instruction does not dominate all uses`). See
        // `collect_pattern_bindings_in_stmts`.
        let mut pattern_bindings: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        collect_pattern_bindings_in_stmts(&f.body, &mut pattern_bindings);
        let mut let_counts: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        collect_let_binding_counts_in_stmts(&f.body, &mut let_counts);
        // Param names and prior pattern collisions already count as one
        // "introduction" of the name, so a single later `let` of the same
        // name is also a collision requiring the hoisted alloca.
        for (name, _) in &params {
            *let_counts.entry(name.clone()).or_insert(0) += 1;
        }
        for name in &pattern_bindings {
            *let_counts.entry(name.clone()).or_insert(0) += 1;
        }
        for name in &pattern_bindings {
            if self.mut_vars.contains(name) {
                continue;
            }
            self.emit(&mut body, Instruction::Alloca { dest: name.clone() });
            self.pattern_vars.insert(name.clone());
            self.local_binding_names.insert(name.clone());
        }
        // Hoist alloca slots for `let` names introduced more than once.
        // We reuse `pattern_vars` as the carrier so `lower_expr` knows to
        // emit a Load on subsequent Ident references.
        for (name, count) in &let_counts {
            if *count > 1 && !self.pattern_vars.contains(name) && !self.mut_vars.contains(name) {
                self.emit(&mut body, Instruction::Alloca { dest: name.clone() });
                self.pattern_vars.insert(name.clone());
                self.local_binding_names.insert(name.clone());
            }
        }

        // spec §12.3 (M-defer-fix): pre-allocate one bool-backed activation
        // flag per `defer` statement in the function body (including nested
        // if/while/for/match scopes). Flags default to false; a defer
        // statement at runtime stores true. emit_deferred then only runs
        // deferred expressions whose flag is true, so a defer inside an
        // `if` branch that never executes does not fire at function return.
        let defer_count = count_defer_sites_in_stmts(&f.body);
        self.defer_flag_count = defer_count;
        for idx in 0..defer_count {
            let flag_name = format!(".defer_active_{idx}");
            self.emit(
                &mut body,
                Instruction::Alloca {
                    dest: flag_name.clone(),
                },
            );
            self.emit(
                &mut body,
                Instruction::Store {
                    dest: flag_name,
                    value: Operand::Const(Constant::Int(0)),
                },
            );
        }
        self.next_defer_index = 0;

        let mut last_expr_result = None;
        for stmt in &f.body {
            // Track the result of expression statements for implicit return.
            // Also update current_loc here so that Expr-statement instructions
            // (including panic) carry the correct source line (ADR 0014).
            if let Stmt::Expr(s) = stmt {
                self.current_loc = self.span_to_loc(s.span);
                last_expr_result = Some(self.lower_expr(&s.expr, &mut body));
            } else {
                last_expr_result = None;
                self.lower_stmt(stmt, &mut body);
            }
        }

        // If last instruction isn't a return, add implicit return
        if !body
            .last()
            .is_some_and(|s| matches!(s.instr, Instruction::Return { .. }))
        {
            // Capture last temp BEFORE emitting defers so defer calls don't
            // overwrite the return value (spec §12.3).
            let pre_defer_last_temp = self.last_temp_name(&body);
            // spec §12.3: emit deferred expressions before implicit return
            self.emit_deferred(&mut body);
            if return_type == Ty::Unit {
                self.emit(&mut body, Instruction::Return { value: None });
            } else if let Some(expr_val) = last_expr_result {
                // The last statement was an expression — use its result directly.
                // This takes priority over pre_defer_last_temp because last_temp_name
                // may pick up the dest of a void-return call (e.g. a Unit fn call
                // before a simple variable reference), yielding an undefined SSA value.
                let ret_val = self.maybe_wrap_ok_for_return(expr_val, &return_type, &mut body);
                self.emit(
                    &mut body,
                    Instruction::Return {
                        value: Some(Operand::Var(ret_val)),
                    },
                );
            } else if let Some(last_temp) = pre_defer_last_temp {
                // Fallback: last statement was not an expression (e.g. `let x = f()`).
                // If fn returns Result<T,E> but the tail temp is T (extracted by ?),
                // wrap it in Ok(T) so the return type matches.
                let ret_val = self.maybe_wrap_ok_for_return(last_temp, &return_type, &mut body);
                self.emit(
                    &mut body,
                    Instruction::Return {
                        value: Some(Operand::Var(ret_val)),
                    },
                );
            } else {
                self.emit(&mut body, Instruction::Return { value: None });
            }
        }

        // Collect local_metas for DWARF locals display (ADR-0014 §4a-ii).
        // Covers: function parameters (.addr alloca slots) and mut-binding alloca slots.
        let mut local_metas: Vec<crate::ir::LocalMeta> = Vec::new();
        for (name, ty) in &params {
            local_metas.push(crate::ir::LocalMeta {
                name: name.clone(),
                ty: ty.clone(),
                alloca_name: format!("{name}.addr"),
            });
        }
        for s in &body {
            if let Instruction::Alloca { dest } = &s.instr {
                if self.mut_vars.contains(dest) {
                    let ty = self.infer_alloca_type(dest);
                    local_metas.push(crate::ir::LocalMeta {
                        name: dest.clone(),
                        ty,
                        alloca_name: dest.clone(),
                    });
                }
            }
        }

        Function {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_main: false,
            local_metas,
        }
    }

    /// Infer the Tyra type of an alloca-backed binding from per-function type maps.
    fn infer_alloca_type(&self, name: &str) -> Ty {
        if let Some(ty) = self.generic_var_types.get(name) {
            return ty.clone();
        }
        if let Some(type_name) = self.var_types.get(name) {
            return Ty::Named(type_name.clone());
        }
        if self.float_vars.contains(name) {
            return Ty::Float;
        }
        if self.string_vars.contains(name) {
            return Ty::String;
        }
        if self.bool_vars.contains(name) {
            return Ty::Bool;
        }
        Ty::Int
    }

    fn lower_stmt(&mut self, stmt: &Stmt, body: &mut Vec<MirStmt>) {
        // Update the current source location from this statement's span (ADR 0014).
        let stmt_span = match stmt {
            Stmt::Let(s) => s.span,
            Stmt::Mut(s) => s.span,
            Stmt::Return(s) => s.span,
            Stmt::Defer(s) => s.span,
            Stmt::Break(s) => s.span,
            Stmt::Continue(s) => s.span,
            Stmt::Expr(s) => s.span,
        };
        self.current_loc = self.span_to_loc(stmt_span);
        match stmt {
            Stmt::Let(s) => {
                // Record the binding name first so even Int/Bool/Unit lets
                // (which don't populate any type-keyed map) appear in
                // local_binding_names. This keeps the M11 phase 2 safety
                // gate correct for shadows of any type.
                self.local_binding_names.insert(s.name.clone());
                let is_float = self.is_float_expr(&s.value);
                let is_string = self.is_string_expr(&s.value);
                let struct_type = self.expr_struct_type(&s.value);
                // See Stmt::Mut for the binding_type_hint rationale.
                let prev_hint = self.binding_type_hint.clone();
                if let Some(ann) = &s.type_annotation {
                    self.binding_type_hint = Some(Ty::from_type_expr(ann));
                }
                let val = self.lower_expr(&s.value, body);
                self.binding_type_hint = prev_hint;
                // Track types from AST analysis
                if is_float || self.float_vars.contains(&val) {
                    self.float_vars.insert(s.name.clone());
                }
                if is_string || self.string_vars.contains(&val) {
                    self.string_vars.insert(s.name.clone());
                }
                if self.bool_vars.contains(&val) {
                    self.bool_vars.insert(s.name.clone());
                }
                if let Some(stype) = struct_type {
                    self.var_types.insert(s.name.clone(), stype);
                } else if let Some(vtype) = self.var_types.get(&val).cloned() {
                    // Propagate struct type from the lowered temp
                    self.var_types.insert(s.name.clone(), vtype);
                }
                // Track generic types (Option/Result) from the value temp
                if let Some(gt) = self.generic_var_types.get(&val).cloned() {
                    self.generic_var_types.insert(s.name.clone(), gt.clone());
                    self.var_types
                        .insert(s.name.clone(), gt.monomorphized_name());
                }
                // Fallback: if the LHS carries an explicit Generic type
                // annotation (`let xs: List<Int> = []`), use it for the
                // binding's type tracking. The RHS may be a ListLit whose
                // element type is an unresolved Var (empty list), in which
                // case generic_var_types would otherwise not propagate —
                // leaving `.get()` / `.push()` method dispatch unable to
                // find the list element type.
                if let Some(ann) = &s.type_annotation {
                    let ann_ty = Ty::from_type_expr(ann);
                    if ann_ty.is_option()
                        || ann_ty.is_result()
                        || ann_ty.is_list()
                        || ann_ty.is_set()
                        || matches!(&ann_ty, Ty::Generic(n, _) if n == "Map")
                    {
                        self.register_adt_type(&ann_ty);
                        self.generic_var_types
                            .insert(s.name.clone(), ann_ty.clone());
                        self.var_types
                            .insert(s.name.clone(), ann_ty.monomorphized_name());
                    } else if let Ty::Named(n) = &ann_ty {
                        self.var_types.insert(s.name.clone(), n.clone());
                    }
                }
                // Propagate M9 task-handle tracking across let-binding copy.
                if let Some(trt) = self.task_result_types.get(&val).cloned() {
                    self.task_result_types.insert(s.name.clone(), trt);
                }
                // Propagate closure fat-pointer tracking (ADR-0011).
                if self.closure_vars.contains(&val) {
                    self.closure_vars.insert(s.name.clone());
                }
                if let Some(fn_ty) = self.closure_fn_types.get(&val).cloned() {
                    self.closure_fn_types.insert(s.name.clone(), fn_ty);
                }
                // If the name is already backed by an alloca (from a prior
                // match pattern binding or a prior `mut`), reuse the slot
                // via Store instead of emitting a fresh SSA Copy. Tyra's
                // match pattern variables are tracked function-wide today
                // (not per-arm), so a `let v = foo()` after a sibling match
                // that bound `v` would otherwise collide on the `%v` SSA
                // name and trip LLVM's "multiple definition of local value"
                // verifier. Semantically equivalent: subsequent Ident("v")
                // lookups already Load from the alloca for pattern_vars /
                // mut_vars (see lower/expr.rs:82).
                if self.pattern_vars.contains(&s.name) || self.mut_vars.contains(&s.name) {
                    self.emit(
                        body,
                        Instruction::Store {
                            dest: s.name.clone(),
                            value: Operand::Var(val),
                        },
                    );
                } else {
                    self.emit(
                        body,
                        Instruction::Copy {
                            dest: s.name.clone(),
                            source: val,
                        },
                    );
                }
            }
            Stmt::Mut(s) => {
                self.local_binding_names.insert(s.name.clone());
                let is_float = self.is_float_expr(&s.value);
                let is_string = self.is_string_expr(&s.value);
                let struct_type = self.expr_struct_type(&s.value);
                // Push the annotation as a typing hint so RHS expressions
                // that depend on context (a bare `None`, an empty list
                // literal) can pick up the declared `Option<T>` /
                // `List<T>` instead of the default Option<Int> / List<Int>.
                let prev_hint = self.binding_type_hint.clone();
                if let Some(ann) = &s.type_annotation {
                    self.binding_type_hint = Some(Ty::from_type_expr(ann));
                }
                let val = self.lower_expr(&s.value, body);
                self.binding_type_hint = prev_hint;
                if is_float {
                    self.float_vars.insert(s.name.clone());
                }
                if is_string {
                    self.string_vars.insert(s.name.clone());
                }
                if self.bool_vars.contains(&val) {
                    self.bool_vars.insert(s.name.clone());
                }
                if let Some(stype) = struct_type {
                    self.var_types.insert(s.name.clone(), stype);
                }
                // Propagate generic tracking from the RHS temp (List<T>
                // returned from a user function, Option/Result from a call).
                if let Some(gt) = self.generic_var_types.get(&val).cloned() {
                    self.generic_var_types.insert(s.name.clone(), gt.clone());
                    self.var_types
                        .insert(s.name.clone(), gt.monomorphized_name());
                }
                // Fallback to explicit annotation (see Stmt::Let rationale).
                if let Some(ann) = &s.type_annotation {
                    let ann_ty = Ty::from_type_expr(ann);
                    if ann_ty.is_option()
                        || ann_ty.is_result()
                        || ann_ty.is_list()
                        || ann_ty.is_set()
                        || matches!(&ann_ty, Ty::Generic(n, _) if n == "Map")
                    {
                        self.register_adt_type(&ann_ty);
                        self.generic_var_types
                            .insert(s.name.clone(), ann_ty.clone());
                        self.var_types
                            .insert(s.name.clone(), ann_ty.monomorphized_name());
                    } else if let Ty::Named(n) = &ann_ty {
                        self.var_types.insert(s.name.clone(), n.clone());
                    }
                }
                // M9 follow-up: propagate Task<T> handle tracking across
                // `mut t = spawn ...` so `t.await` later unboxes correctly.
                if let Some(trt) = self.task_result_types.get(&val).cloned() {
                    self.task_result_types.insert(s.name.clone(), trt);
                }
                // Propagate closure fat-pointer tracking (ADR-0011).
                if self.closure_vars.contains(&val) {
                    self.closure_vars.insert(s.name.clone());
                }
                if let Some(fn_ty) = self.closure_fn_types.get(&val).cloned() {
                    self.closure_fn_types.insert(s.name.clone(), fn_ty);
                }
                // Mutable locals use alloca+store for SSA-compatible mutation.
                // Skip the alloca if the slot was already emitted — either
                // hoisted to function entry (when the same name appears >1
                // times, see collect_let_binding_counts_in_stmts) or created
                // by a prior `mut`/`let`/pattern binding earlier in the
                // function. Without this guard, `mut v = ...` inside two
                // sibling branches produces `%v = alloca i64` twice and LLVM
                // rejects with "multiple definition of local value".
                //
                // Record the name as mut BEFORE the guard so later reads in
                // the same statement's value expression (rare but possible
                // via closures / self-reference) see the slot semantics.
                let already_slotted =
                    self.pattern_vars.contains(&s.name) || self.mut_vars.contains(&s.name);
                self.mut_vars.insert(s.name.clone());
                if !already_slotted {
                    self.emit_synthetic(
                        body,
                        Instruction::Alloca {
                            dest: s.name.clone(),
                        },
                    );
                }
                self.emit(
                    body,
                    Instruction::Store {
                        dest: s.name.clone(),
                        value: Operand::Var(val),
                    },
                );
            }
            Stmt::Return(s) => {
                let value = s.value.as_ref().map(|v| {
                    let t = self.lower_expr(v, body);
                    Operand::Var(t)
                });
                // spec §12.3: emit deferred expressions before return
                self.emit_deferred(body);
                self.emit(body, Instruction::Return { value });
            }
            Stmt::Defer(d) => {
                // spec §12.3: mark the pre-allocated activation flag as
                // true so emit_deferred will run this expression in LIFO
                // order at every return path. Defers inside branches or
                // loops that never execute leave their flag at false.
                //
                // The flag index is allocated up front in lower_fn based
                // on `count_defer_sites_in_stmts(&f.body)`. A `defer`
                // encountered during emit_deferred re-lowering (i.e. a
                // defer syntactically inside another deferred expression)
                // would drift past the pre-scan count — caught here by
                // assert so any future grammar change that allows such
                // nesting surfaces immediately.
                let idx = self.next_defer_index;
                assert!(
                    idx < self.defer_flag_count,
                    "defer ordinal {idx} exceeds pre-scan count {} — \
                     nested defer inside a deferred expression is not supported",
                    self.defer_flag_count
                );
                self.next_defer_index += 1;
                let flag_name = format!(".defer_active_{idx}");
                self.emit(
                    body,
                    Instruction::Store {
                        dest: flag_name.clone(),
                        value: Operand::Const(Constant::Int(1)),
                    },
                );
                self.deferred_exprs.push((flag_name, d.expr.clone()));
            }
            Stmt::Break(_) => {
                // Jump to the innermost loop's exit label. The type checker
                // (E0214) already rejected `break` outside loops, so the stack
                // is guaranteed non-empty here.
                //
                // No dead label is emitted after the Jump: `range_terminates`
                // skips Label instructions when walking backwards, so if-branch
                // lowering (lower_if) correctly sees the Jump as a terminator
                // and does not append a redundant merge-branch.
                let exit = self
                    .loop_exit_stack
                    .last()
                    .expect("break without enclosing loop (should be caught by type checker)")
                    .clone();
                self.emit_synthetic(body, Instruction::Jump { label: exit });
            }
            Stmt::Continue(_) => {
                // Jump to the innermost loop's head label (condition-check for
                // while; increment section for for). E0215 guards the stack.
                let head = self
                    .loop_head_stack
                    .last()
                    .expect("continue without enclosing loop (should be caught by type checker)")
                    .clone();
                self.emit_synthetic(body, Instruction::Jump { label: head });
            }
            Stmt::Expr(s) => {
                self.lower_expr(&s.expr, body);
            }
        }
    }

    /// Lower a statement block. Returns:
    /// - `BlockTail::Value(name)` — trailing `Stmt::Expr` lowered to a
    ///   usable SSA temp (including bare-Ident arms whose value only
    ///   exists in `lower_expr`'s return, not in the MIR stream).
    /// - `BlockTail::Unit` — trailing `Stmt::Expr` is Unit-typed (a call
    ///   to a void-returning fn, or a print-family sink). The enclosing
    ///   if/match MUST NOT spill any value; the call's MIR dest is
    ///   never assigned in codegen (066-square-list E0500 class).
    /// - `BlockTail::Fallback` — no trailing `Stmt::Expr` (block ends on
    ///   a Let/Mut/Defer/Return, or is empty). Caller may fall back to
    ///   `last_temp_in_range` if it cares about value-shaped arms.
    fn lower_block_collect_tail(&mut self, stmts: &[Stmt], body: &mut Vec<MirStmt>) -> BlockTail {
        let mut tail = BlockTail::Fallback;
        for (i, stmt) in stmts.iter().enumerate() {
            if i + 1 == stmts.len() {
                if let Stmt::Expr(s) = stmt {
                    let suppress = is_unit_call_expr(&s.expr, &self.fn_return_types);
                    let val = self.lower_expr(&s.expr, body);
                    tail = if suppress {
                        BlockTail::Unit
                    } else {
                        BlockTail::Value(val)
                    };
                    continue;
                }
            }
            self.lower_stmt(stmt, body);
        }
        tail
    }

    fn lower_if(&mut self, if_expr: &IfExpr, body: &mut Vec<MirStmt>) -> String {
        let cond = self.lower_expr(&if_expr.condition, body);
        let then_label = self.fresh_label("then");
        let else_label = self.fresh_label("else");
        let end_label = self.fresh_label("if_end");

        // Allocate result slot (like match)
        let result_slot = self.fresh_temp();
        self.emit_synthetic(
            body,
            Instruction::Alloca {
                dest: result_slot.clone(),
            },
        );

        self.emit_synthetic(
            body,
            Instruction::BranchIf {
                cond: Operand::Var(cond),
                true_label: then_label.clone(),
                false_label: else_label.clone(),
            },
        );

        // Then branch
        self.emit_synthetic(body, Instruction::Label(then_label));
        let then_start = body.len();
        let then_tail = self.lower_block_collect_tail(&if_expr.then_body, body);
        if !range_terminates(body, then_start) {
            if !block_ends_with_assignment(body, then_start) {
                let last = match then_tail {
                    BlockTail::Value(v) => Some(v),
                    BlockTail::Unit => None,
                    BlockTail::Fallback => self.last_temp_in_range(body, then_start),
                };
                if let Some(last) = last {
                    self.emit(
                        body,
                        Instruction::Store {
                            dest: result_slot.clone(),
                            value: Operand::Var(last),
                        },
                    );
                }
            }
            self.emit_synthetic(
                body,
                Instruction::Jump {
                    label: end_label.clone(),
                },
            );
        }

        // Else branch
        self.emit_synthetic(body, Instruction::Label(else_label));
        let else_start = body.len();
        let else_tail: BlockTail = match &if_expr.else_body {
            Some(ElseBranch::Else(stmts)) => self.lower_block_collect_tail(stmts, body),
            Some(ElseBranch::ElseIf(inner)) => BlockTail::Value(self.lower_if(inner, body)),
            None => BlockTail::Fallback,
        };
        if !range_terminates(body, else_start) {
            if !block_ends_with_assignment(body, else_start) {
                let last = match else_tail {
                    BlockTail::Value(v) => Some(v),
                    BlockTail::Unit => None,
                    BlockTail::Fallback => self.last_temp_in_range(body, else_start),
                };
                if let Some(last) = last {
                    self.emit(
                        body,
                        Instruction::Store {
                            dest: result_slot.clone(),
                            value: Operand::Var(last),
                        },
                    );
                }
            }
            self.emit_synthetic(
                body,
                Instruction::Jump {
                    label: end_label.clone(),
                },
            );
        }

        self.emit_synthetic(body, Instruction::Label(end_label));

        let result = self.fresh_temp();
        self.emit(
            body,
            Instruction::Load {
                dest: result.clone(),
                source: result_slot,
            },
        );
        result
    }

    /// Emit all deferred expressions in LIFO order (spec §12.3).
    /// Called before every return path (explicit return, ? early return, implicit return).
    /// Note: this deliberately does NOT clear deferred_exprs — every return path
    /// (including multiple ? early returns within a single function) must emit the
    /// full set of deferred expressions. The list is cleared at lower_fn entry.
    fn emit_deferred(&mut self, body: &mut Vec<MirStmt>) {
        // Clone to avoid borrow conflict (deferred_exprs is on self).
        let entries: Vec<(String, Expr)> = self.deferred_exprs.iter().rev().cloned().collect();
        for (flag, expr) in &entries {
            // Runtime check: only execute this deferred expression if its
            // activation flag was set to true by a reached `defer` stmt.
            let tmp = self.fresh_temp();
            self.emit(
                body,
                Instruction::Load {
                    dest: tmp.clone(),
                    source: flag.clone(),
                },
            );
            let zero = self.fresh_temp();
            self.emit(
                body,
                Instruction::Const {
                    dest: zero.clone(),
                    value: Constant::Int(0),
                },
            );
            let active = self.fresh_temp();
            self.emit(
                body,
                Instruction::BinOp {
                    dest: active.clone(),
                    op: MirBinOp::NeqInt,
                    lhs: Operand::Var(tmp),
                    rhs: Operand::Var(zero),
                },
            );
            let then_lbl = self.fresh_label("defer_run");
            let skip_lbl = self.fresh_label("defer_skip");
            self.emit_synthetic(
                body,
                Instruction::BranchIf {
                    cond: Operand::Var(active),
                    true_label: then_lbl.clone(),
                    false_label: skip_lbl.clone(),
                },
            );
            self.emit_synthetic(body, Instruction::Label(then_lbl));
            self.lower_expr(expr, body);
            self.emit_synthetic(
                body,
                Instruction::Jump {
                    label: skip_lbl.clone(),
                },
            );
            self.emit_synthetic(body, Instruction::Label(skip_lbl));
        }
    }

    fn last_temp_in_range(&self, body: &[MirStmt], start: usize) -> Option<String> {
        for stmt in body[start..].iter().rev() {
            match &stmt.instr {
                Instruction::Const { dest, .. }
                | Instruction::Call {
                    dest: Some(dest), ..
                }
                | Instruction::BinOp { dest, .. }
                | Instruction::Neg { dest, .. }
                | Instruction::Not { dest, .. }
                | Instruction::Copy { dest, .. }
                | Instruction::Load { dest, .. }
                | Instruction::Phi { dest, .. }
                | Instruction::StructInit { dest, .. }
                | Instruction::FieldGet { dest, .. }
                | Instruction::AdtInit { dest, .. }
                | Instruction::AdtPayload { dest, .. }
                | Instruction::StringFormat { dest, .. }
                | Instruction::ListInit { dest, .. }
                | Instruction::ListLen { dest, .. }
                | Instruction::ListGet { dest, .. }
                | Instruction::ListGetSafe { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }

    fn last_temp_name(&self, body: &[MirStmt]) -> Option<String> {
        for stmt in body.iter().rev() {
            match &stmt.instr {
                Instruction::Const { dest, .. }
                | Instruction::Call {
                    dest: Some(dest), ..
                }
                | Instruction::BinOp { dest, .. }
                | Instruction::Neg { dest, .. }
                | Instruction::Not { dest, .. }
                | Instruction::Copy { dest, .. }
                | Instruction::Load { dest, .. }
                | Instruction::Phi { dest, .. }
                | Instruction::StructInit { dest, .. }
                | Instruction::FieldGet { dest, .. }
                | Instruction::AdtInit { dest, .. }
                | Instruction::AdtPayload { dest, .. }
                | Instruction::StringFormat { dest, .. }
                | Instruction::ListInit { dest, .. }
                | Instruction::ListLen { dest, .. }
                | Instruction::ListGet { dest, .. }
                | Instruction::ListGetSafe { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }
}

/// Convert AST binary op to MIR op, selecting Int or Float variant.
/// Outcome of `LowerCtx::lower_block_collect_tail`. Lets the caller tell
/// "block produced no usable tail value, but NOT because of lack of
/// inspection" (`Unit`) from "no Stmt::Expr tail at all, fall back to
/// MIR-scan" (`Fallback`).
#[derive(Debug, Clone)]
pub(crate) enum BlockTail {
    Value(String),
    Unit,
    Fallback,
}

/// Return true when `expr` is a function call whose callee is known to
/// return `Unit` — either via a registered entry in `fn_return_types` or
/// because the callee is a prelude sink (`println`, `panic`, etc.).
///
/// Used by `lower_block_collect_tail` to suppress propagating a
/// void-returning call's MIR dest as the enclosing block's tail value.
/// Codegen emits `call void @f(...)` for such calls (no `%dest = ...`
/// SSA assignment), so reporting the dest up to `lower_if` / `lower_match`
/// would spill an undefined SSA value into the result slot and trip
/// LLVM's `use of undefined value` verifier — the 066-square-list class
/// of E0500.
pub(crate) fn is_unit_call_expr(
    expr: &Expr,
    fn_return_types: &std::collections::HashMap<String, Ty>,
) -> bool {
    let ExprKind::Call(callee, _args) = &expr.kind else {
        return false;
    };
    match &callee.kind {
        ExprKind::Ident(fname) => {
            if matches!(
                fname.as_str(),
                "print" | "println" | "eprint" | "eprintln" | "panic"
            ) {
                return true;
            }
            matches!(fn_return_types.get(fname), Some(Ty::Unit))
        }
        // module.fn(args) — qualified name is "{module}__{fn}".
        ExprKind::FieldAccess(obj, method) => {
            if let ExprKind::Ident(module) = &obj.kind {
                let qualified = format!("{module}__{method}");
                matches!(fn_return_types.get(&qualified), Some(Ty::Unit))
            } else {
                false
            }
        }
        _ => false,
    }
}

pub(crate) fn ast_binop_to_mir(op: BinOp, is_float: bool) -> MirBinOp {
    match (op, is_float) {
        (BinOp::Add, false) => MirBinOp::AddInt,
        (BinOp::Add, true) => MirBinOp::AddFloat,
        (BinOp::Sub, false) => MirBinOp::SubInt,
        (BinOp::Sub, true) => MirBinOp::SubFloat,
        (BinOp::Mul, false) => MirBinOp::MulInt,
        (BinOp::Mul, true) => MirBinOp::MulFloat,
        (BinOp::Div, false) => MirBinOp::DivInt,
        (BinOp::Div, true) => MirBinOp::DivFloat,
        (BinOp::Rem, _) => MirBinOp::RemInt,
        (BinOp::Eq, _) => MirBinOp::EqInt,
        (BinOp::NotEq, _) => MirBinOp::NeqInt,
        (BinOp::Lt, false) => MirBinOp::LtInt,
        (BinOp::Lt, true) => MirBinOp::LtFloat,
        (BinOp::LtEq, false) => MirBinOp::LeInt,
        (BinOp::LtEq, true) => MirBinOp::LeFloat,
        (BinOp::Gt, false) => MirBinOp::GtInt,
        (BinOp::Gt, true) => MirBinOp::GtFloat,
        (BinOp::GtEq, false) => MirBinOp::GeInt,
        (BinOp::GtEq, true) => MirBinOp::GeFloat,
        (BinOp::RefEq, _) => MirBinOp::EqInt,
        (BinOp::And, _) => MirBinOp::And,
        (BinOp::Or, _) => MirBinOp::Or,
    }
}

/// Choose the wider type when two ADT variants have different types at the same
/// Returns true if the instruction range `body[start..]` already ends with a block
/// terminator (Return, Jump, or BranchIf), so that the caller can skip emitting
/// a redundant Store + Jump.
/// True if the last value-producing instruction in `body[start..]` is an
/// assignment-style `Store` — i.e. a store whose destination is a
/// user-named binding (named identifier, not a compiler-generated `_tN`
/// temp) and not the defer-activation flag reserved name `.defer_active_*`.
///
/// Used by `lower_if` / `lower_while` / match lowering to avoid spilling
/// the RHS of an assignment into the block's result slot when the block
/// actually evaluates to `Unit` (Tyra's spec: `x = e` is a statement of
/// type `Unit`, not the value of `e`). Without this check, the if's
/// result slot ends up holding whatever temp fed the Store — typically
/// an i64 from `x = x + 1` — and clashes with a sibling arm whose
/// tail is a Bool assignment (`done = true`). That mismatch surfaces in
/// LLVM codegen as E0500 (`'%_tN' defined with type 'i64' but expected 'i1'`).
pub(crate) fn block_ends_with_assignment(body: &[MirStmt], start: usize) -> bool {
    // Walk backwards skipping Jump/Label/Alloca bookkeeping to find the
    // instruction that actually expresses the block's tail value.
    for stmt in body[start..].iter().rev() {
        match &stmt.instr {
            Instruction::Jump { .. } | Instruction::Label(_) | Instruction::Alloca { .. } => {
                continue;
            }
            Instruction::Store { dest, .. } => {
                // A Store into a temp (`_t42`) is internal plumbing (e.g. a
                // nested if's result-slot propagation); it is NOT a user
                // assignment and should not Unit-ify the enclosing block.
                // Similarly, the §12.3 defer activation flags are internal.
                let is_temp = dest.starts_with("_t");
                let is_defer_flag = dest.starts_with(".defer_active_");
                return !is_temp && !is_defer_flag;
            }
            _ => return false,
        }
    }
    false
}

pub(crate) fn range_terminates(body: &[MirStmt], start: usize) -> bool {
    body[start..]
        .iter()
        .rev()
        .find(|s| !matches!(s.instr, Instruction::Alloca { .. } | Instruction::Label(_)))
        .is_some_and(|s| {
            matches!(
                s.instr,
                Instruction::Return { .. }
                    | Instruction::Jump { .. }
                    | Instruction::BranchIf { .. }
            )
        })
}

/// Walk a Stmt slice and count every `Stmt::Defer` reachable through
/// nested if/while/for/match/block scopes within the SAME function body.
/// Lambdas are skipped because their defers belong to the lambda's own
/// frame (lambdas are lowered as separate functions).
pub(crate) fn count_defer_sites_in_stmts(stmts: &[Stmt]) -> usize {
    stmts.iter().map(count_defer_sites_in_stmt).sum()
}

fn count_defer_sites_in_stmt(s: &Stmt) -> usize {
    match s {
        Stmt::Defer(_) => 1,
        Stmt::Let(l) => count_defer_sites_in_expr(&l.value),
        Stmt::Mut(m) => count_defer_sites_in_expr(&m.value),
        Stmt::Return(r) => r.value.as_ref().map(count_defer_sites_in_expr).unwrap_or(0),
        Stmt::Break(_) | Stmt::Continue(_) => 0,
        Stmt::Expr(e) => count_defer_sites_in_expr(&e.expr),
    }
}

/// Traversal order here MUST match body lowering (lower_if/lower_while/
/// lower_for/lower_match) so `next_defer_index` during lowering aligns
/// with the ordinal assigned during this pre-scan. Condition/subject
/// expressions are lowered BEFORE their bodies, so they are counted first.
///
/// Note: `defer` is a Stmt, not an Expr, so it cannot syntactically appear
/// inside a condition/subject today. The ordering still matters as an
/// invariant in case that grammar ever changes.
fn count_defer_sites_in_expr(e: &Expr) -> usize {
    match &e.kind {
        ExprKind::If(i) => {
            let mut n = count_defer_sites_in_expr(&i.condition);
            n += count_defer_sites_in_stmts(&i.then_body);
            if let Some(else_branch) = &i.else_body {
                n += count_defer_sites_in_else(else_branch);
            }
            n
        }
        ExprKind::Match(m) => {
            count_defer_sites_in_expr(&m.subject)
                + m.arms
                    .iter()
                    .map(|a| count_defer_sites_in_stmts(&a.body))
                    .sum::<usize>()
        }
        ExprKind::While(w) => {
            // Note on loop semantics: a defer inside a while body is
            // pre-allocated once; each iteration idempotently re-stores 1
            // to the same flag, so the deferred expression runs exactly
            // once at function return regardless of iteration count. This
            // is the chosen v0.1 semantics and differs from Go's
            // per-iteration defer stack. Rationale: avoids unbounded
            // accumulation in long-running loops.
            count_defer_sites_in_expr(&w.condition) + count_defer_sites_in_stmts(&w.body)
        }
        ExprKind::For(f) => {
            count_defer_sites_in_expr(&f.iter) + count_defer_sites_in_stmts(&f.body)
        }
        // Lambdas: defers inside a lambda belong to that frame; skip.
        ExprKind::Lambda(_) => 0,
        _ => 0,
    }
}

// --- Binding-name pre-scan ---
//
// Match pattern variables must be alloca'd at a program point that
// dominates every arm that might bind them AND every code site that
// reads them. The naive strategy of emitting the alloca at the match's
// entry block fails when the enclosing match is itself nested inside
// an `if` / `while` / `for` branch: the alloca sits in a non-dominating
// block, so a sibling branch that also binds the same pattern name — or
// a later `let` that shadows it — gets `use of undefined value` /
// `Instruction does not dominate all uses` from the LLVM verifier.
//
// The same failure mode applies to a user writing `let n = foo()`
// twice in the same function (e.g. once inside a `while` body, once
// after the loop): each Copy emits `%n = add i64 …, 0`, which LLVM
// rejects as `multiple definition of local value named 'n'`. Tyra's
// surface syntax treats those two `let`s as distinct scope bindings,
// but the current MIR name scheme reuses the user identifier directly.
//
// We side-step both by pre-scanning the whole function body and
// emitting an alloca in the entry block for every name that is either
// (a) introduced by a match pattern, or (b) introduced by `let`/`mut`
// more than once. `pre_alloca_pattern_vars` then finds them already in
// `pattern_vars` and no-ops, and `Stmt::Let` / `Stmt::Mut` emit a
// Store into the hoisted alloca instead of the usual Copy.

pub(crate) fn collect_pattern_bindings_in_stmts(
    stmts: &[Stmt],
    out: &mut std::collections::HashSet<String>,
) {
    for s in stmts {
        collect_pattern_bindings_in_stmt(s, out);
    }
}

/// Count every `let` / `mut` introduction of each name in the statement
/// tree. Used alongside pattern bindings to detect the "same identifier
/// bound twice in the same function" case that would otherwise emit
/// two `%name = …` SSA definitions.
pub(crate) fn collect_let_binding_counts_in_stmts(
    stmts: &[Stmt],
    out: &mut std::collections::HashMap<String, u32>,
) {
    for s in stmts {
        collect_let_binding_counts_in_stmt(s, out);
    }
}

fn collect_let_binding_counts_in_stmt(s: &Stmt, out: &mut std::collections::HashMap<String, u32>) {
    match s {
        Stmt::Let(l) => {
            *out.entry(l.name.clone()).or_insert(0) += 1;
            collect_let_binding_counts_in_expr(&l.value, out);
        }
        Stmt::Mut(m) => {
            *out.entry(m.name.clone()).or_insert(0) += 1;
            collect_let_binding_counts_in_expr(&m.value, out);
        }
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                collect_let_binding_counts_in_expr(v, out);
            }
        }
        Stmt::Expr(e) => collect_let_binding_counts_in_expr(&e.expr, out),
        Stmt::Defer(d) => collect_let_binding_counts_in_expr(&d.expr, out),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn collect_let_binding_counts_in_expr(e: &Expr, out: &mut std::collections::HashMap<String, u32>) {
    match &e.kind {
        ExprKind::If(i) => {
            collect_let_binding_counts_in_expr(&i.condition, out);
            collect_let_binding_counts_in_stmts(&i.then_body, out);
            if let Some(eb) = &i.else_body {
                collect_let_binding_counts_in_else(eb, out);
            }
        }
        ExprKind::Match(m) => {
            collect_let_binding_counts_in_expr(&m.subject, out);
            for arm in &m.arms {
                collect_let_binding_counts_in_stmts(&arm.body, out);
            }
        }
        ExprKind::While(w) => {
            collect_let_binding_counts_in_expr(&w.condition, out);
            collect_let_binding_counts_in_stmts(&w.body, out);
        }
        ExprKind::For(f) => {
            // Count the induction variable too so that two sibling
            // `for x in ...` loops over lists of the same element type
            // share one alloca slot. Without this, each loop emits
            // `Copy { dest: "x" }` which LLVM rejects as a duplicate
            // definition of `%x`. The codegen type-scan resolves the
            // alloca type from the Store instructions emitted in
            // lower/expr.rs (ExprKind::For Store path), so typed
            // iterables (String / struct / Option / etc.) hoist
            // correctly — not just the i64 case. Incompatible shadows
            // at different element types are rejected earlier by the
            // type checker, so we never reach here with a genuine
            // type conflict.
            for name in &f.bindings {
                *out.entry(name.clone()).or_insert(0) += 1;
            }
            collect_let_binding_counts_in_expr(&f.iter, out);
            collect_let_binding_counts_in_stmts(&f.body, out);
        }
        ExprKind::Lambda(_) => {}
        _ => {}
    }
}

fn collect_let_binding_counts_in_else(
    eb: &ElseBranch,
    out: &mut std::collections::HashMap<String, u32>,
) {
    match eb {
        ElseBranch::Else(stmts) => collect_let_binding_counts_in_stmts(stmts, out),
        ElseBranch::ElseIf(i) => {
            collect_let_binding_counts_in_expr(&i.condition, out);
            collect_let_binding_counts_in_stmts(&i.then_body, out);
            if let Some(inner) = &i.else_body {
                collect_let_binding_counts_in_else(inner, out);
            }
        }
    }
}

fn collect_pattern_bindings_in_stmt(s: &Stmt, out: &mut std::collections::HashSet<String>) {
    match s {
        Stmt::Let(l) => collect_pattern_bindings_in_expr(&l.value, out),
        Stmt::Mut(m) => collect_pattern_bindings_in_expr(&m.value, out),
        Stmt::Return(r) => {
            if let Some(v) = &r.value {
                collect_pattern_bindings_in_expr(v, out);
            }
        }
        Stmt::Expr(e) => collect_pattern_bindings_in_expr(&e.expr, out),
        Stmt::Defer(d) => collect_pattern_bindings_in_expr(&d.expr, out),
        Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

fn collect_pattern_bindings_in_expr(e: &Expr, out: &mut std::collections::HashSet<String>) {
    match &e.kind {
        ExprKind::If(i) => {
            collect_pattern_bindings_in_expr(&i.condition, out);
            collect_pattern_bindings_in_stmts(&i.then_body, out);
            if let Some(else_branch) = &i.else_body {
                collect_pattern_bindings_in_else(else_branch, out);
            }
        }
        ExprKind::Match(m) => {
            collect_pattern_bindings_in_expr(&m.subject, out);
            for arm in &m.arms {
                collect_pattern_bindings_in_pattern(&arm.pattern.kind, out);
                collect_pattern_bindings_in_stmts(&arm.body, out);
            }
        }
        ExprKind::While(w) => {
            collect_pattern_bindings_in_expr(&w.condition, out);
            collect_pattern_bindings_in_stmts(&w.body, out);
        }
        ExprKind::For(f) => {
            collect_pattern_bindings_in_expr(&f.iter, out);
            collect_pattern_bindings_in_stmts(&f.body, out);
        }
        // Lambdas get their own function frame; skip.
        ExprKind::Lambda(_) => {}
        _ => {}
    }
}

fn collect_pattern_bindings_in_else(eb: &ElseBranch, out: &mut std::collections::HashSet<String>) {
    match eb {
        ElseBranch::Else(stmts) => collect_pattern_bindings_in_stmts(stmts, out),
        ElseBranch::ElseIf(i) => {
            collect_pattern_bindings_in_expr(&i.condition, out);
            collect_pattern_bindings_in_stmts(&i.then_body, out);
            if let Some(inner) = &i.else_body {
                collect_pattern_bindings_in_else(inner, out);
            }
        }
    }
}

fn collect_pattern_bindings_in_pattern(
    p: &PatternKind,
    out: &mut std::collections::HashSet<String>,
) {
    if let PatternKind::Constructor(_, fields) = p {
        for pf in fields {
            match &pf.pattern.kind {
                PatternKind::Ident(name) if name != "_" => {
                    out.insert(name.clone());
                }
                PatternKind::Constructor(_, _) => {
                    collect_pattern_bindings_in_pattern(&pf.pattern.kind, out);
                }
                _ => {}
            }
        }
    }
}

fn count_defer_sites_in_else(eb: &ElseBranch) -> usize {
    match eb {
        ElseBranch::Else(stmts) => count_defer_sites_in_stmts(stmts),
        ElseBranch::ElseIf(i) => {
            let mut n = count_defer_sites_in_expr(&i.condition);
            n += count_defer_sites_in_stmts(&i.then_body);
            if let Some(inner) = &i.else_body {
                n += count_defer_sites_in_else(inner);
            }
            n
        }
    }
}
