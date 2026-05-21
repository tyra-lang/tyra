// Call expression lowering — extracted from expr.rs.
//
// Contains the `lower_call` method which handles all ExprKind::Call
// variants: constructors, method calls, module-qualified calls, etc.
#![allow(clippy::collapsible_if, clippy::collapsible_else_if)]
#![allow(clippy::unnecessary_map_or)]

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

impl super::LowerCtx {
    /// Lower a call expression, returning the name of the temporary holding the result.
    pub(super) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Arg],
        body: &mut Vec<Instruction>,
    ) -> String {
        // Check for Option/Result constructors: Some(x), Ok(x), Err(e)
        if let ExprKind::Ident(ctor_name) = &callee.kind
            && matches!(ctor_name.as_str(), "Some" | "Ok" | "Err")
            && args.len() == 1
        {
            let arg_val = self.lower_expr(&args[0].value, body);
            let arg_type = self.infer_expr_type(&args[0].value).unwrap_or(Ty::Int);
            let tag = if ctor_name == "Err" { 1i64 } else { 0i64 };

            let full_type = self
                .infer_adt_call_type(ctor_name, &arg_type)
                .unwrap_or_else(|| Ty::Generic("Option".into(), vec![arg_type]));
            self.register_adt_type(&full_type);
            let type_name = full_type.monomorphized_name();

            // Build fields vector based on constructor type
            let fields = match ctor_name.as_str() {
                "Some" => vec![Operand::Var(arg_val)],
                "Ok" => vec![Operand::Var(arg_val), Operand::Const(Constant::Int(0))],
                "Err" => vec![Operand::Const(Constant::Int(0)), Operand::Var(arg_val)],
                _ => vec![Operand::Var(arg_val)],
            };

            let dest = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: dest.clone(),
                type_name: type_name.clone(),
                tag,
                fields,
            });
            self.generic_var_types.insert(dest.clone(), full_type);
            self.var_types.insert(dest.clone(), type_name);
            return dest;
        }

        // Check for qualified ADT constructor: Payment.Card(last4: "1234")
        if let ExprKind::FieldAccess(obj, variant_name) = &callee.kind
            && let ExprKind::Ident(type_name) = &obj.kind
            && self
                .adt_variant_fields
                .contains_key(&(type_name.clone(), variant_name.clone()))
        {
            let vfields = self
                .adt_variant_fields
                .get(&(type_name.clone(), variant_name.clone()))
                .cloned()
                .unwrap_or_default();
            let tag = self
                .variant_tags
                .get(&(type_name.clone(), variant_name.clone()))
                .copied()
                .unwrap_or(0);

            // Per-variant slot layout: total payload slots = struct len - 1 (tag).
            // This variant's fields start at variant_offset (1-based struct field index).
            let max_field_count = self
                .adt_struct_defs
                .get(type_name)
                .map(|f| f.len() - 1) // total payload slots (excluding tag)
                .unwrap_or(vfields.len());
            // variant_field_offsets is populated for all user-defined ADT variants in
            // TypeDef processing. Option/Result/List constructors (Ok/Err/Some/None)
            // are handled by earlier branches in lower_call and never reach this path,
            // so the fallback is only a safety net for unregistered edge cases.
            let variant_offset = self
                .variant_field_offsets
                .get(&(type_name.clone(), variant_name.clone()))
                .copied()
                .unwrap_or(1);

            // Fill all payload slots with zeroinitializer, then overwrite this variant's slots.
            let mut field_operands = vec![Operand::Const(Constant::Int(0)); max_field_count];

            let mut used_args: std::collections::HashSet<usize> = std::collections::HashSet::new();

            for (fi, (fname, _fty)) in vfields.iter().enumerate() {
                let slot = variant_offset - 1 + fi; // 0-based index into field_operands
                let labeled = args
                    .iter()
                    .enumerate()
                    .find(|(idx, a)| !used_args.contains(idx) && a.label.as_deref() == Some(fname));
                let resolved = if let Some((idx, a)) = labeled {
                    used_args.insert(idx);
                    Some(a)
                } else {
                    let positional = args
                        .iter()
                        .enumerate()
                        .find(|(idx, _)| !used_args.contains(idx));
                    if let Some((idx, a)) = positional {
                        used_args.insert(idx);
                        Some(a)
                    } else {
                        None
                    }
                };
                if let Some(a) = resolved {
                    let val = self.lower_expr(&a.value, body);
                    if slot < field_operands.len() {
                        field_operands[slot] = Operand::Var(val);
                    }
                }
            }

            let dest = self.fresh_temp();
            body.push(Instruction::AdtInit {
                dest: dest.clone(),
                type_name: type_name.clone(),
                tag,
                fields: field_operands,
            });
            self.var_types.insert(dest.clone(), type_name.clone());
            return dest;
        }

        // Check for value type constructor: Point(x: 3.0, y: 4.0)
        if let ExprKind::Ident(name) = &callee.kind
            && self.struct_fields.contains_key(name)
        {
            let field_defs = self.struct_fields[name].clone();
            // Map labeled args to declaration order.
            // If args have labels, match by label name.
            // If no labels, assume positional order.
            let mut field_operands = Vec::with_capacity(field_defs.len());
            let mut used_args: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for (fname, fty) in &field_defs {
                // First try label match
                let labeled = args
                    .iter()
                    .enumerate()
                    .find(|(idx, a)| !used_args.contains(idx) && a.label.as_deref() == Some(fname));
                let resolved = if let Some((idx, a)) = labeled {
                    used_args.insert(idx);
                    Some(a)
                } else {
                    // Positional fallback: next unused arg
                    let positional = args
                        .iter()
                        .enumerate()
                        .find(|(idx, _)| !used_args.contains(idx));
                    if let Some((idx, a)) = positional {
                        used_args.insert(idx);
                        Some(a)
                    } else {
                        None
                    }
                };
                if let Some(a) = resolved {
                    // Propagate the declared field type so that a bare
                    // `None` / `Some(x)` / `Ok(x)` / `Err(e)` argument
                    // (whose type isn't observable from the expression
                    // alone) is lowered as the expected Option<T> /
                    // Result<T, E> rather than falling back to Option<Int>.
                    // Uses `current_fn_return_type` as the one-slot hint,
                    // matching the existing Ident "None" lookup path
                    // (lower/expr.rs lower_expr ExprKind::Ident).
                    let saved = self.current_fn_return_type.clone();
                    if fty.is_option() || fty.is_result() {
                        self.current_fn_return_type = fty.clone();
                        // Register the generic type so its monomorphized
                        // struct definition exists when the field store
                        // emits.
                        self.register_adt_type(fty);
                    }
                    let val = self.lower_expr(&a.value, body);
                    self.current_fn_return_type = saved;
                    field_operands.push(Operand::Var(val));
                } else {
                    // Missing field — emit unit as placeholder
                    field_operands.push(Operand::Const(Constant::Unit));
                }
            }
            let dest = self.fresh_temp();
            body.push(Instruction::StructInit {
                dest: dest.clone(),
                type_name: name.clone(),
                fields: field_operands,
            });
            return dest;
        }

        // §17.3.6 Map<String, Int> method dispatch.
        // - m.get(k)         → Option<Int> via MapGetOption (uses runtime
        //                      presence flag to disambiguate `0 / Some(0)` /
        //                      `i64::MIN / None`).
        // - m.contains_key(k) → Bool
        if let ExprKind::FieldAccess(obj, method) = &callee.kind {
            if matches!(method.as_str(), "get" | "contains_key")
                && args.len() == 1
                && self.infer_map_type(obj).is_some()
            {
                let obj_val = self.lower_expr(obj, body);
                let handle = self.fresh_temp();
                body.push(Instruction::FieldGet {
                    dest: handle.clone(),
                    obj: Operand::Var(obj_val),
                    type_name: "Map__String__Int".into(),
                    field_index: 0,
                });
                self.string_vars.insert(handle.clone());
                let key_val = self.lower_expr(&args[0].value, body);
                match method.as_str() {
                    "contains_key" => {
                        let dest = self.fresh_temp();
                        body.push(Instruction::Call {
                            dest: Some(dest.clone()),
                            func: "__map_contains_string_int".into(),
                            args: vec![Operand::Var(handle), Operand::Var(key_val)],
                        });
                        return dest;
                    }
                    "get" => {
                        let opt_ty = Ty::Generic("Option".into(), vec![Ty::Int]);
                        self.register_adt_type(&opt_ty);
                        let dest = self.fresh_temp();
                        body.push(Instruction::MapGetOption {
                            dest: dest.clone(),
                            handle: Operand::Var(handle),
                            key: Operand::Var(key_val),
                        });
                        self.generic_var_types.insert(dest.clone(), opt_ty.clone());
                        self.var_types
                            .insert(dest.clone(), opt_ty.monomorphized_name());
                        return dest;
                    }
                    _ => {}
                }
            }
        }

        // Check for .len() on List<T> (spec §11)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "len"
            && args.is_empty()
        {
            if let Some(_list_ty) = self.infer_list_type(obj) {
                let obj_val = self.lower_expr(obj, body);
                let dest = self.fresh_temp();
                body.push(Instruction::ListLen {
                    dest: dest.clone(),
                    list: Operand::Var(obj_val),
                });
                return dest;
            }
        }

        // Check for .get(index) on List<T> (spec §11)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "get"
            && args.len() == 1
        {
            if let Some(list_ty) = self.infer_list_type(obj) {
                let elem_type = list_ty.list_elem().cloned().unwrap_or(Ty::Int);
                let obj_val = self.lower_expr(obj, body);
                let idx_val = self.lower_expr(&args[0].value, body);

                let option_type = Ty::Generic("Option".into(), vec![elem_type.clone()]);
                self.register_adt_type(&option_type);
                let option_type_name = option_type.monomorphized_name();

                let dest = self.fresh_temp();
                body.push(Instruction::ListGetSafe {
                    dest: dest.clone(),
                    list: Operand::Var(obj_val),
                    index: Operand::Var(idx_val),
                    elem_type,
                });

                // Track the result as Option<T>
                self.generic_var_types.insert(dest.clone(), option_type);
                self.var_types.insert(dest.clone(), option_type_name);
                return dest;
            }
        }

        // Check for .ok_or() on Option<T> (spec §12.2):
        // Converts Option<T> to Result<T, E> where E is the type of the argument.
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "ok_or"
            && args.len() == 1
        {
            // Lower receiver first so chained calls (e.g., .get().ok_or()) are tracked
            let obj_val = self.lower_expr(obj, body);

            // Determine if receiver is Option<T> (check lowered temp in generic_var_types)
            let opt_type = self
                .generic_var_types
                .get(&obj_val)
                .cloned()
                .or_else(|| self.infer_expr_type(obj))
                .or_else(|| {
                    if let ExprKind::Ident(name) = &obj.kind {
                        self.generic_var_types.get(name).cloned()
                    } else {
                        None
                    }
                });
            if let Some(ref oty) = opt_type
                && oty.is_option()
            {
                let err_arg = self.lower_expr(&args[0].value, body);

                // Infer err type from the argument expression, variable
                // tracking, or the enclosing function's return type
                let err_type = self
                    .infer_expr_type(&args[0].value)
                    .or_else(|| self.var_types.get(&err_arg).map(|n| Ty::Named(n.clone())))
                    .or_else(|| self.current_fn_return_type.result_err_type().cloned())
                    .unwrap_or(Ty::Named("Error".into()));

                let inner_t = oty.option_inner().cloned().unwrap_or(Ty::Int);
                let result_type = Ty::Generic("Result".into(), vec![inner_t, err_type]);
                self.register_adt_type(&result_type);
                let result_type_name = result_type.monomorphized_name();
                let opt_type_name = oty.monomorphized_name();

                // Extract tag from Option
                let tag = self.fresh_temp();
                body.push(Instruction::AdtTag {
                    dest: tag.clone(),
                    obj: Operand::Var(obj_val.clone()),
                    type_name: opt_type_name.clone(),
                });

                // Check: tag == 0 means Some
                let zero = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: zero.clone(),
                    value: Constant::Int(0),
                });
                let is_some = self.fresh_temp();
                body.push(Instruction::BinOp {
                    dest: is_some.clone(),
                    op: MirBinOp::EqInt,
                    lhs: Operand::Var(tag),
                    rhs: Operand::Var(zero),
                });

                let some_label = self.fresh_label("ok_or_some");
                let none_label = self.fresh_label("ok_or_none");
                let end_label = self.fresh_label("ok_or_end");

                // Allocate result slot
                let result_slot = self.fresh_temp();
                body.push(Instruction::Alloca {
                    dest: result_slot.clone(),
                });

                body.push(Instruction::BranchIf {
                    cond: Operand::Var(is_some),
                    true_label: some_label.clone(),
                    false_label: none_label.clone(),
                });

                // Some path: Ok(payload)
                body.push(Instruction::Label(some_label));
                let payload = self.fresh_temp();
                body.push(Instruction::AdtPayload {
                    dest: payload.clone(),
                    obj: Operand::Var(obj_val),
                    type_name: opt_type_name,
                    field_index: 1,
                });
                let ok_val = self.fresh_temp();
                body.push(Instruction::AdtInit {
                    dest: ok_val.clone(),
                    type_name: result_type_name.clone(),
                    tag: 0,
                    fields: vec![Operand::Var(payload), Operand::Const(Constant::Int(0))],
                });
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(ok_val),
                });
                body.push(Instruction::Jump {
                    label: end_label.clone(),
                });

                // None path: Err(err_arg)
                body.push(Instruction::Label(none_label));
                let err_val = self.fresh_temp();
                body.push(Instruction::AdtInit {
                    dest: err_val.clone(),
                    type_name: result_type_name,
                    tag: 1,
                    fields: vec![Operand::Const(Constant::Int(0)), Operand::Var(err_arg)],
                });
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(err_val),
                });
                body.push(Instruction::Jump {
                    label: end_label.clone(),
                });

                body.push(Instruction::Label(end_label));
                let result_val = self.fresh_temp();
                body.push(Instruction::Load {
                    dest: result_val.clone(),
                    source: result_slot,
                });
                // Track the result as a generic type for downstream ? operator
                self.generic_var_types
                    .insert(result_val.clone(), result_type.clone());
                self.var_types
                    .insert(result_val.clone(), result_type.monomorphized_name());
                return result_val;
            }
            // Receiver was lowered but is not Option — treat as generic method call.
            // Use obj_val directly to avoid double-lowering.
            let arg_operands: Vec<Operand> = args
                .iter()
                .map(|a| {
                    let t = self.lower_expr(&a.value, body);
                    Operand::Var(t)
                })
                .collect();
            let dest = self.fresh_temp();
            let mangled = format!("{obj_val}.ok_or");
            body.push(Instruction::Call {
                dest: Some(dest.clone()),
                func: mangled,
                args: arg_operands,
            });
            return dest;
        }

        // Check for .copy() on value types only (§8.6)
        // copy() is NOT available on data types.
        if let ExprKind::FieldAccess(obj, method) = &callee.kind
            && method == "copy"
        {
            if let Some((type_name, field_defs)) = self.resolve_struct_type(obj) {
                if !self.data_types.contains(&type_name) {
                    let obj_val = self.lower_expr(obj, body);
                    return self.lower_copy(&obj_val, &type_name, &field_defs, args, body);
                }
            }
            // Not a value type — fall through to method call or generic call
        }

        // Check for impl trait method call: p.method() → Type__method(p)
        // (§8.7 static dispatch)
        if let ExprKind::FieldAccess(obj, method) = &callee.kind {
            match self.resolve_impl_method(obj, method) {
                super::ImplMethodResult::Resolved(mangled_name) => {
                    // M11 phase 2 safety gate: `AppServer__get` / `_post`
                    // pass the handler through as a ptr, but the stdlib
                    // types it as `String` (Tyra lacks a first-class Fn
                    // type in v0.1). Without this check, a user writing
                    // `app.get("/p", "literal")` or `app.get("/p", some_str)`
                    // would type-check and then call through a non-
                    // function pointer at request time (UB).
                    //
                    // Gate conditions (all must hold):
                    //   (a) the caller imported `http.server` — prevents
                    //       false positives on user types named
                    //       AppServer that happen to have a two-string
                    //       `get`/`post` method. NOTE: a user who
                    //       imports `http.server` AND defines their own
                    //       `impl X for AppServer fn get(...)` will see
                    //       the mangled name collide (`AppServer__get`)
                    //       and their method will share this gate;
                    //       advise against reusing the name.
                    //   (b) the handler argument is a bare Ident.
                    //   (c) that Ident resolves to a top-level function
                    //       name AND is NOT shadowed by a local binding
                    //       recorded in `local_binding_names`
                    //       (let / mut / pattern / for-loop / params).
                    //
                    // KNOWN LIMITATION: `fn_return_types` also contains
                    // intrinsic names (`__http_server_new` etc.). A
                    // direct pass like `app.get("/p", __http_server_new)`
                    // would pass this gate textually; the resolver's
                    // rule that `__*` identifiers are stdlib-only (see
                    // PRELUDE_FUNCTIONS) is what keeps user code from
                    // reaching the call site. If that rule is ever
                    // relaxed, this gate needs a "user-defined fn only"
                    // predicate.
                    if (mangled_name == "AppServer__get" || mangled_name == "AppServer__post")
                        && args.len() == 2
                        && self.imported_modules.contains("http.server")
                    {
                        let handler_expr = &args[1].value;
                        let is_valid_fn = match &handler_expr.kind {
                            ExprKind::Ident(name) => {
                                // Known top-level function AND not shadowed
                                // by any local binding (`let` / `mut` /
                                // pattern / for-loop induction) recorded in
                                // `local_binding_names`. The single-set
                                // check replaces the earlier seven type-
                                // keyed maps so Int / Bool / Unit shadows
                                // can't slip through.
                                self.fn_return_types.contains_key(name)
                                    && !self.local_binding_names.contains(name.as_str())
                            }
                            _ => false,
                        };
                        if !is_valid_fn {
                            panic!(
                                "http.server {}() handler must be a top-level \
                                 function name, not an arbitrary expression \
                                 or a shadowing local. Tyra v0.1 lacks a \
                                 first-class Fn type, so the stdlib types the \
                                 handler slot as `String`; anything other \
                                 than a bare function identifier here would \
                                 produce undefined behavior at dispatch time.",
                                if mangled_name == "AppServer__get" {
                                    "get"
                                } else {
                                    "post"
                                }
                            );
                        }
                    }
                    let self_val = self.lower_expr(obj, body);
                    let mut arg_operands = vec![Operand::Var(self_val)];
                    for a in args {
                        let t = self.lower_expr(&a.value, body);
                        arg_operands.push(Operand::Var(t));
                    }
                    let dest = self.fresh_temp();
                    let ret_ty = self.fn_return_types.get(&mangled_name).cloned();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: mangled_name,
                        args: arg_operands,
                    });
                    if let Some(ref ty) = ret_ty {
                        match ty {
                            Ty::String => {
                                self.string_vars.insert(dest.clone());
                            }
                            Ty::Float => {
                                self.float_vars.insert(dest.clone());
                            }
                            Ty::Named(n) => {
                                self.var_types.insert(dest.clone(), n.clone());
                            }
                            Ty::Generic(_, _) => {
                                self.generic_var_types.insert(dest.clone(), ty.clone());
                                self.var_types.insert(dest.clone(), ty.monomorphized_name());
                            }
                            _ => {}
                        }
                    }
                    return dest;
                }
                super::ImplMethodResult::Ambiguous => {
                    // Multiple impls define this method but type can't be resolved.
                    // Emit a call to a clearly-invalid name to produce a linker error
                    // rather than silently generating broken IR.
                    // TODO: Emit proper diagnostic via tyra-diagnostics.
                    let self_val = self.lower_expr(obj, body);
                    let dest = self.fresh_temp();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: format!("__unresolved_method_{method}"),
                        args: vec![Operand::Var(self_val)],
                    });
                    return dest;
                }
                super::ImplMethodResult::NotFound => {
                    // Not an impl method — fall through
                }
            }
        }

        // Special case: tasks.join_all(list) (§17.1, M9). If the list elements
        // are live Task<T> handles (tracked via task_result_types), lower to a
        // JoinAll instruction so codegen can await every handle in order and
        // return a List<T> of unboxed results. Otherwise (tasks are plain
        // values from the async-as-sync stub path) fall back to identity.
        if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind
            && let ExprKind::Ident(module_name) = &obj.kind
            && self.imported_modules.contains(module_name.as_str())
            && (fn_name == "join_all" || fn_name == "select")
            && args.len() == 1
        {
            let list_expr = &args[0].value;
            // Inspect the list literal elements to recover their task result
            // type. For non-literal arguments this best-effort lookup falls
            // back to the identity path.
            if let ExprKind::ListLit(elements) = &list_expr.kind {
                // Lower the whole list first (consumes elements).
                // But we need task_result_types of each element, which we
                // can only get by inspecting *before* the list is built.
                let elem_task_ty = elements.first().and_then(|e| {
                    if let ExprKind::Ident(name) = &e.kind {
                        self.task_result_types.get(name).cloned()
                    } else {
                        None
                    }
                });
                if let Some(elem_ty) = elem_task_ty {
                    let list_temp = self.lower_expr(list_expr, body);
                    let dest = self.fresh_temp();
                    if fn_name == "join_all" {
                        let list_ty = Ty::Generic("List".into(), vec![elem_ty.clone()]);
                        self.register_adt_type(&list_ty);
                        body.push(Instruction::JoinAll {
                            dest: dest.clone(),
                            list: Operand::Var(list_temp),
                            elem_type: elem_ty.clone(),
                        });
                        self.generic_var_types.insert(dest.clone(), list_ty.clone());
                        self.var_types
                            .insert(dest.clone(), list_ty.monomorphized_name());
                    } else {
                        // tasks.select(tasks) -> Task<T>. The dest is an i64
                        // task handle; register task_result_types so a
                        // downstream .await unboxes T. Mirror join_all by
                        // also recording a var_types entry so downstream
                        // passes that query type by temp name find a
                        // meaningful string rather than None.
                        let task_ty = Ty::Generic("Task".into(), vec![elem_ty.clone()]);
                        body.push(Instruction::Select {
                            dest: dest.clone(),
                            list: Operand::Var(list_temp),
                            elem_type: elem_ty.clone(),
                        });
                        self.task_result_types.insert(dest.clone(), elem_ty.clone());
                        self.var_types
                            .insert(dest.clone(), task_ty.monomorphized_name());
                    }
                    return dest;
                }
            }
            // Non-literal list argument (e.g. `tasks.select(my_list)`): we
            // cannot recover `Task<T>`'s T from the elements, so the special
            // lowering would emit either a silent identity (returning the
            // list itself) or an unawaitable handle. Both are miscompiles.
            // v0.1 requires the arg to be a list literal of task handles;
            // reject at lowering time with a clear message.
            //
            // TODO: when task type inference flows through list-typed vars
            // (e.g. `let ts: List<Task<Int>> = [...]` → lookup Task<Int>
            // from the declared type), remove this restriction.
            if fn_name == "select" {
                panic!(
                    "tasks.select in v0.1 requires a list literal of task \
                     handles, e.g. `tasks.select([a, b, c])`. Dynamic lists \
                     (`tasks.select(my_list)`) are not yet supported — \
                     bind the spawns to locals and pass a literal."
                );
            }
            return self.lower_expr(list_expr, body);
        }

        // §17.3.5 polymorphic-method redirect: the v0.1 `list` module exposes
        // `len`/`get` as `List<Int>`-typed wrappers that delegate to the
        // polymorphic `xs.len()` / `xs.get(i)` method. When the model writes
        // `list.len(words)` with `words: List<String>` (e.g. from
        // `string.split_whitespace`), the wrapper's `List<Int>` param makes
        // LLVM reject the call as type-mismatched (E0500). Redirect to the
        // element-type-agnostic `Instruction::ListLen` / `ListGetSafe`.
        // `contains` / `index_of` stay List<Int>-only (they go through
        // `__list_int_*` intrinsics, no polymorphic codegen yet).
        if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind {
            if let ExprKind::Ident(module_name) = &obj.kind {
                if module_name == "list"
                    && self.imported_modules.contains("list")
                    && matches!(fn_name.as_str(), "len" | "get" | "push")
                    && !args.is_empty()
                {
                    let first = &args[0].value;
                    let elem_is_int = match &first.kind {
                        ExprKind::Ident(name) => self
                            .generic_var_types
                            .get(name)
                            .map(|t| {
                                matches!(t, Ty::Generic(n, ta)
                                if n == "List" && matches!(ta.first(), Some(Ty::Int)))
                            })
                            .unwrap_or(true),
                        _ => true,
                    };
                    if !elem_is_int {
                        match fn_name.as_str() {
                            "push" if args.len() == 2 => {
                                let elem_type = if let ExprKind::Ident(name) = &first.kind {
                                    self.generic_var_types
                                        .get(name)
                                        .and_then(|t| t.list_elem().cloned())
                                        .unwrap_or(Ty::Int)
                                } else {
                                    Ty::Int
                                };
                                let list_val = self.lower_expr(first, body);
                                let elem_val = self.lower_expr(&args[1].value, body);
                                let list_ty = Ty::Generic("List".into(), vec![elem_type.clone()]);
                                self.register_adt_type(&list_ty);
                                let dest = self.fresh_temp();
                                body.push(Instruction::ListPush {
                                    dest: dest.clone(),
                                    list: Operand::Var(list_val),
                                    elem: Operand::Var(elem_val),
                                    elem_type,
                                });
                                self.generic_var_types.insert(dest.clone(), list_ty.clone());
                                self.var_types
                                    .insert(dest.clone(), list_ty.monomorphized_name());
                                return dest;
                            }
                            "len" if args.len() == 1 => {
                                let obj_val = self.lower_expr(first, body);
                                let dest = self.fresh_temp();
                                body.push(Instruction::ListLen {
                                    dest: dest.clone(),
                                    list: Operand::Var(obj_val),
                                });
                                return dest;
                            }
                            "get" if args.len() == 2 => {
                                let elem_type = if let ExprKind::Ident(name) = &first.kind {
                                    self.generic_var_types
                                        .get(name)
                                        .and_then(|t| t.list_elem().cloned())
                                        .unwrap_or(Ty::Int)
                                } else {
                                    Ty::Int
                                };
                                let obj_val = self.lower_expr(first, body);
                                let idx_val = self.lower_expr(&args[1].value, body);
                                let option_type =
                                    Ty::Generic("Option".into(), vec![elem_type.clone()]);
                                self.register_adt_type(&option_type);
                                let dest = self.fresh_temp();
                                body.push(Instruction::ListGetSafe {
                                    dest: dest.clone(),
                                    list: Operand::Var(obj_val),
                                    index: Operand::Var(idx_val),
                                    elem_type,
                                });
                                self.generic_var_types
                                    .insert(dest.clone(), option_type.clone());
                                self.var_types
                                    .insert(dest.clone(), option_type.monomorphized_name());
                                return dest;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Check for module-qualified call: math.square() → math__square() (§13)
        if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind {
            if let ExprKind::Ident(module_name) = &obj.kind {
                if self.imported_modules.contains(module_name.as_str()) {
                    let qualified_name = format!("{module_name}__{fn_name}");

                    // Module-qualified struct constructor: server.Response(fields) → StructInit
                    if fn_name.chars().next().map_or(false, |c| c.is_uppercase()) {
                        if let Some(field_defs) = self.struct_fields.get(fn_name.as_str()).cloned()
                        {
                            let mut field_operands = Vec::with_capacity(field_defs.len());
                            let mut used_args: std::collections::HashSet<usize> =
                                std::collections::HashSet::new();
                            for (fname, _fty) in &field_defs {
                                let labeled = args.iter().enumerate().find(|(idx, a)| {
                                    !used_args.contains(idx) && a.label.as_deref() == Some(fname)
                                });
                                let resolved = if let Some((idx, a)) = labeled {
                                    used_args.insert(idx);
                                    Some(a)
                                } else {
                                    let positional = args
                                        .iter()
                                        .enumerate()
                                        .find(|(idx, _)| !used_args.contains(idx));
                                    if let Some((idx, a)) = positional {
                                        used_args.insert(idx);
                                        Some(a)
                                    } else {
                                        None
                                    }
                                };
                                if let Some(a) = resolved {
                                    let val = self.lower_expr(&a.value, body);
                                    field_operands.push(Operand::Var(val));
                                } else {
                                    field_operands.push(Operand::Const(Constant::Unit));
                                }
                            }
                            let dest = self.fresh_temp();
                            body.push(Instruction::StructInit {
                                dest: dest.clone(),
                                type_name: fn_name.clone(),
                                fields: field_operands,
                            });
                            self.var_types.insert(dest.clone(), fn_name.clone());
                            return dest;
                        }
                    }
                    // Reject unknown module-qualified functions early. Without
                    // this check, an undefined call (e.g. `list.bogus(xs)`,
                    // or a hallucinated method like `xs.unwrap_value()` that
                    // routes through this branch) silently emits an LLVM
                    // call to an undefined symbol and surfaces as a generic
                    // E0500 clang failure. The driver catches the panic and
                    // reports it cleanly.
                    if !self.fn_return_types.contains_key(&qualified_name) {
                        panic!(
                            "[E0204] unknown function `{module_name}.{fn_name}`: \
                             no exported function with that name in module `{module_name}`. \
                             Check spelling and the module's `export fn` declarations."
                        );
                    }
                    let arg_operands: Vec<Operand> = args
                        .iter()
                        .map(|a| {
                            let t = self.lower_expr(&a.value, body);
                            Operand::Var(t)
                        })
                        .collect();
                    let dest = self.fresh_temp();
                    // Track return type from fn_return_types
                    let ret_ty = self.fn_return_types.get(&qualified_name).cloned();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: qualified_name,
                        args: arg_operands,
                    });
                    if let Some(ref ty) = ret_ty {
                        match ty {
                            Ty::String => {
                                self.string_vars.insert(dest.clone());
                            }
                            Ty::Float => {
                                self.float_vars.insert(dest.clone());
                            }
                            Ty::Named(n) => {
                                self.var_types.insert(dest.clone(), n.clone());
                            }
                            Ty::Generic(_, _) => {
                                self.generic_var_types.insert(dest.clone(), ty.clone());
                                self.var_types.insert(dest.clone(), ty.monomorphized_name());
                            }
                            _ => {}
                        }
                    }
                    return dest;
                }
            }
        }

        // Special case: print/println/eprint/eprintln with StringInterp argument.
        // Emit separate print calls for each segment.
        if let ExprKind::Ident(fname) = &callee.kind
            && matches!(fname.as_str(), "print" | "println" | "eprint" | "eprintln")
            && args.len() == 1
            && let ExprKind::StringInterp(parts) = &args[0].value.kind
        {
            let is_println = fname == "println" || fname == "eprintln";
            for part in parts {
                match part {
                    StringPart::Lit(s) => {
                        let idx = self.intern_string(s);
                        let str_temp = self.fresh_temp();
                        body.push(Instruction::Const {
                            dest: str_temp.clone(),
                            value: Constant::StringRef(idx),
                        });
                        body.push(Instruction::Call {
                            dest: None,
                            func: "print".into(),
                            args: vec![Operand::Var(str_temp)],
                        });
                    }
                    StringPart::Expr(e) => {
                        let val = self.lower_expr(e, body);
                        body.push(Instruction::Call {
                            dest: None,
                            func: "print".into(),
                            args: vec![Operand::Var(val)],
                        });
                    }
                }
            }
            // Add newline for println/eprintln
            if is_println {
                let nl_idx = self.intern_string("\n");
                let nl_temp = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: nl_temp.clone(),
                    value: Constant::StringRef(nl_idx),
                });
                body.push(Instruction::Call {
                    dest: None,
                    func: "print".into(),
                    args: vec![Operand::Var(nl_temp)],
                });
            }
            let dest = self.fresh_temp();
            body.push(Instruction::Const {
                dest: dest.clone(),
                value: Constant::Unit,
            });
            return dest;
        }

        // String value method auto-dispatch: `s.byte_at(i)` rewrites to
        // `string.byte_at(s, i)` when `s` is a String value and a matching
        // `string__<method>` exists in the stdlib. The model frequently
        // reaches for method syntax on String values even though Tyra v0.1
        // exposes string operations only as module functions; this auto-
        // rewrite makes the model's mental model work and turns what would
        // otherwise be a "@s.method" untyped call (E0500 downstream) into
        // a typed call with a tracked return type.
        if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind {
            let qualified = format!("string__{fn_name}");
            // Fall back to the always-linked intrinsic (__string_len etc.) when
            // `import string` is absent so the module-qualified wrapper hasn't
            // been registered. This lets `s.len()` work without requiring the
            // import, matching model expectations.
            let intrinsic = format!("__string_{fn_name}");
            let actual_fn = if self.fn_return_types.contains_key(&qualified) {
                Some(qualified.clone())
            } else if self.fn_return_types.contains_key(&intrinsic) {
                Some(intrinsic)
            } else {
                None
            };
            if let Some(resolved_fn) = actual_fn {
                if self.is_string_expr(obj) {
                    let recv_temp = self.lower_expr(obj, body);
                    let mut arg_operands = vec![Operand::Var(recv_temp)];
                    for a in args {
                        let t = self.lower_expr(&a.value, body);
                        arg_operands.push(Operand::Var(t));
                    }
                    let dest = self.fresh_temp();
                    let ret_ty = self.fn_return_types.get(&resolved_fn).cloned();
                    body.push(Instruction::Call {
                        dest: Some(dest.clone()),
                        func: resolved_fn,
                        args: arg_operands,
                    });
                    if let Some(ref ty) = ret_ty {
                        match ty {
                            Ty::String => {
                                self.string_vars.insert(dest.clone());
                            }
                            Ty::Float => {
                                self.float_vars.insert(dest.clone());
                            }
                            Ty::Named(n) => {
                                self.var_types.insert(dest.clone(), n.clone());
                            }
                            Ty::Generic(_, _) => {
                                self.register_adt_type(ty);
                                self.generic_var_types.insert(dest.clone(), ty.clone());
                                self.var_types.insert(dest.clone(), ty.monomorphized_name());
                            }
                            _ => {}
                        }
                    }
                    return dest;
                }
            } else if self.is_string_expr(obj) {
                // String receiver but no `string__<method>` / `__string_<method>`
                // exists.  Without this guard, the call falls through to the
                // generic fallback below and emits a bogus `Call { func:
                // "<recv>.<method>" }` whose return value is an untyped i64.
                // When the AI then matches the result with `Some(_) / None`
                // patterns (a common shape for `s.get(i)`), the lowerer emits
                // `extractvalue %struct.Option__Int <i64>` and codegen fails
                // with an opaque LLVM E0500.  Reject early with E0204 so the
                // user (or the AI on retry) sees a real diagnostic pointing
                // at the missing string method.
                panic!(
                    "[E0204] unknown string method `{fn_name}`: no `string.{fn_name}` \
                     in stdlib and no `__string_{fn_name}` intrinsic. \
                     Tyra v0.1 exposes string operations only as module functions; \
                     check the import and the available `string.*` exports."
                );
            }
        }

        // Resolve callee to a name first so the closure check below covers
        // all callee shapes (Ident, returned temps, parenthesised exprs, …).
        let callee_name = match &callee.kind {
            ExprKind::Ident(name) => name.clone(),
            ExprKind::FieldAccess(obj, method) => {
                let obj_name = self.lower_expr(obj, body);
                format!("{obj_name}.{method}")
            }
            _ => self.lower_expr(callee, body),
        };

        // Indirect call through a closure fat pointer (ADR-0011).
        // Works for any callee expression, not just simple identifiers.
        if self.closure_vars.contains(callee_name.as_str()) {
            if let Some(tyra_types::Ty::Fn(param_types, ret_box)) =
                self.closure_fn_types.get(callee_name.as_str()).cloned()
            {
                let fat_ptr = Operand::Var(callee_name.clone());
                let arg_operands: Vec<Operand> = args
                    .iter()
                    .map(|a| {
                        let t = self.lower_expr(&a.value, body);
                        Operand::Var(t)
                    })
                    .collect();
                let dest = self.fresh_temp();
                let return_type = *ret_box;
                body.push(Instruction::IndirectCall {
                    dest: Some(dest.clone()),
                    fat_ptr,
                    args: arg_operands,
                    param_types,
                    return_type: return_type.clone(),
                });
                if return_type.is_option() || return_type.is_result() || return_type.is_list() {
                    self.register_adt_type(&return_type);
                    let mono = return_type.monomorphized_name();
                    self.generic_var_types.insert(dest.clone(), return_type);
                    self.var_types.insert(dest.clone(), mono);
                } else if matches!(return_type, tyra_types::Ty::Fn(..)) {
                    // Closure returning another closure: propagate fat-pointer
                    // type so the next call site also emits IndirectCall.
                    self.closure_vars.insert(dest.clone());
                    self.closure_fn_types.insert(dest.clone(), return_type);
                }
                return dest;
            }
        }

        let func_name = callee_name;

        let arg_operands: Vec<Operand> = args
            .iter()
            .map(|a| {
                let t = self.lower_expr(&a.value, body);
                Operand::Var(t)
            })
            .collect();

        let dest = self.fresh_temp();
        body.push(Instruction::Call {
            dest: Some(dest.clone()),
            func: func_name.clone(),
            args: arg_operands,
        });

        // Track generic return types from function signatures so downstream
        // method dispatch (e.g. `xs.get(i)` on a `List<T>` returned by a
        // user function) can see the correct type via `infer_list_type`.
        // Without the `is_list()` branch, `let nums = parse_ints(s)` leaves
        // `nums` untyped in `generic_var_types`, and `.get()` on it falls
        // through to the raw-call path that emits `@nums.get` as a
        // literal function name — LLVM then rejects the cross-type call.
        if let Some(ret_ty) = self.fn_return_types.get(&func_name).cloned() {
            if ret_ty.is_option() || ret_ty.is_result() || ret_ty.is_list() {
                self.register_adt_type(&ret_ty);
                let mono = ret_ty.monomorphized_name();
                self.generic_var_types.insert(dest.clone(), ret_ty);
                self.var_types.insert(dest.clone(), mono);
            } else if matches!(ret_ty, tyra_types::Ty::Fn(..)) {
                // The call returns a first-class function value: mark it as
                // closure-valued so downstream call sites emit IndirectCall
                // (ADR-0011 §Decision 1 — uniform fat pointer, Fix 3).
                self.closure_vars.insert(dest.clone());
                self.closure_fn_types.insert(dest.clone(), ret_ty);
            }
        }

        dest
    }
}
