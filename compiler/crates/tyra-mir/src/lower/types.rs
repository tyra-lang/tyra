// Type inference helpers for MIR lowering.
//
// Extracted from mod.rs to keep file sizes manageable.
// These methods inspect AST expressions to determine their types,
// resolve struct/impl information, and lower value-type operations.
#![allow(clippy::collapsible_if, clippy::collapsible_else_if)]
#![allow(clippy::unnecessary_map_or, clippy::manual_map)]

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

use super::ImplMethodResult;

impl super::LowerCtx<'_> {
    /// Check if an expression produces a Float value.
    /// Used to select between Int and Float MIR binary operations.
    pub(super) fn is_float_expr(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::FloatLit(_) => true,
            ExprKind::Ident(name) => self.float_vars.contains(name),
            ExprKind::FieldAccess(obj, field) => {
                if let Some((_type_name, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some((_, ty)) = field_defs.iter().find(|(n, _)| n == field) {
                        return *ty == Ty::Float;
                    }
                }
                false
            }
            ExprKind::BinaryOp(lhs, _op, rhs) => self.is_float_expr(lhs) || self.is_float_expr(rhs),
            ExprKind::UnaryOp(_, inner) => self.is_float_expr(inner),
            ExprKind::Call(callee, _) => self.call_returns_type(callee, &Ty::Float),
            _ => false,
        }
    }

    /// Check if an expression produces a String value.
    /// Used to select format specifiers in string interpolation.
    pub(super) fn is_string_expr(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::StringLit(_) => true,
            ExprKind::StringInterp(_) => true,
            ExprKind::Ident(name) => self.string_vars.contains(name),
            ExprKind::FieldAccess(obj, field) => {
                if let Some((_type_name, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some((_, ty)) = field_defs.iter().find(|(n, _)| n == field) {
                        return *ty == Ty::String;
                    }
                }
                false
            }
            ExprKind::Call(callee, _) => self.call_returns_type(callee, &Ty::String),
            ExprKind::Match(m) => {
                !m.arms.is_empty()
                    && m.arms.iter().all(|arm| match arm.body.last() {
                        Some(Stmt::Expr(es)) => self.is_string_expr(&es.expr),
                        _ => false,
                    })
            }
            // list[i] on a List<String> yields String
            ExprKind::Index(obj, _) => {
                if let Some(Ty::Generic(_, args)) = self.infer_list_type(obj) {
                    return args.first() == Some(&Ty::String);
                }
                false
            }
            // if/else expression: String when both arms produce String
            ExprKind::If(i) => self.if_expr_is_string(i),
            _ => false,
        }
    }

    /// Check if an if-expression produces String from both branches.
    fn if_expr_is_string(&self, i: &IfExpr) -> bool {
        let then_is_str = i.then_body.last().map_or(
            false,
            |s| matches!(s, Stmt::Expr(es) if self.is_string_expr(&es.expr)),
        );
        let else_is_str = match &i.else_body {
            Some(ElseBranch::Else(stmts)) => stmts.last().map_or(
                false,
                |s| matches!(s, Stmt::Expr(es) if self.is_string_expr(&es.expr)),
            ),
            Some(ElseBranch::ElseIf(inner)) => self.if_expr_is_string(inner),
            None => false,
        };
        then_is_str && else_is_str
    }

    /// Check if a function/method call returns a specific type.
    fn call_returns_type(&self, callee: &Expr, expected: &Ty) -> bool {
        match &callee.kind {
            ExprKind::Ident(name) => self
                .fn_return_types
                .get(name.as_str())
                .map_or(false, |ty| ty == expected),
            ExprKind::FieldAccess(obj, method) => {
                // Module-qualified function call: `string.substring(...)`,
                // `list.get(...)`, etc. The call-site lowering (call.rs
                // ~line 676) resolves these via `{module}__{method}` in
                // fn_return_types. Mirror that here so expression-shape
                // predicates (is_string_expr / is_float_expr) see the
                // correct return type and the binop lowering picks the
                // right Eq variant. Without this, `string.substring(...)
                // == string.substring(...)` falls through to EqInt and
                // LLVM rejects the `icmp eq i64` on ptr operands (E0500).
                if let ExprKind::Ident(module_name) = &obj.kind {
                    if self.imported_modules.contains(module_name.as_str()) {
                        let qualified = format!("{module_name}__{method}");
                        if let Some(ty) = self.fn_return_types.get(&qualified) {
                            return ty == expected;
                        }
                    }
                }
                // Check impl method return type
                if let ImplMethodResult::Resolved(mangled) = self.resolve_impl_method(obj, method) {
                    return self
                        .fn_return_types
                        .get(mangled.as_str())
                        .map_or(false, |ty| ty == expected);
                }
                false
            }
            _ => false,
        }
    }

    /// Determine the struct type name of an expression (for var_types tracking).
    /// Returns the type name if the expression is a struct constructor call or copy().
    pub(super) fn expr_struct_type(&self, expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Call(callee, _) => {
                if let ExprKind::Ident(name) = &callee.kind {
                    // Constructor call: Point(x: 3.0, y: 4.0)
                    if self.struct_fields.contains_key(name) {
                        return Some(name.clone());
                    }
                    // Regular function call: check return type
                    if let Some(Ty::Named(type_name)) = self.fn_return_types.get(name) {
                        if self.struct_fields.contains_key(type_name) {
                            return Some(type_name.clone());
                        }
                    }
                }
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    // copy() call: p.copy(x: 1.0)
                    if method == "copy" {
                        return self.resolve_struct_type(obj).map(|(tn, _)| tn);
                    }
                    // impl method call: check return type
                    if let ImplMethodResult::Resolved(mangled) =
                        self.resolve_impl_method(obj, method)
                    {
                        if let Some(Ty::Named(type_name)) = self.fn_return_types.get(&mangled) {
                            if self.struct_fields.contains_key(type_name) {
                                return Some(type_name.clone());
                            }
                        }
                    }
                }
                None
            }
            ExprKind::Ident(name) => self.var_types.get(name).cloned(),
            _ => None,
        }
    }

    /// Lower a binary operation on value types (§8.6 ability derivation).
    /// Returns Some(dest) if both operands are the same Named type, None otherwise.
    pub(super) fn lower_value_type_binop(
        &mut self,
        l: &str,
        r: &str,
        op: BinOp,
        lhs_expr: &Expr,
        rhs_expr: &Expr,
        body: &mut Vec<MirStmt>,
    ) -> Option<String> {
        // Both operands must be the same named type
        let l_type = self.resolve_struct_type(lhs_expr);
        let r_type = self.resolve_struct_type(rhs_expr);
        let (type_name, fields) = match (&l_type, &r_type) {
            (Some((ln, lf)), Some((rn, _))) if ln == rn => (ln.clone(), lf.clone()),
            _ => return None,
        };

        match op {
            // Eq/NotEq: compare all fields (spec §8.6: auto-derives if all fields have Eq)
            BinOp::Eq | BinOp::NotEq => {
                // Float fields block Eq derivation (ADR-0002)
                if fields.iter().any(|(_, ty)| *ty == Ty::Float) {
                    return None; // Fall through to default (will error or use EqInt)
                }

                let mut field_conds = Vec::new();
                for (i, (_, field_ty)) in fields.iter().enumerate() {
                    let l_field = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::FieldGet {
                            dest: l_field.clone(),
                            obj: Operand::Var(l.to_string()),
                            type_name: type_name.clone(),
                            field_index: i as u32,
                        },
                    );
                    let r_field = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::FieldGet {
                            dest: r_field.clone(),
                            obj: Operand::Var(r.to_string()),
                            type_name: type_name.clone(),
                            field_index: i as u32,
                        },
                    );
                    let field_eq = self.fresh_temp();
                    let field_op = if *field_ty == Ty::String {
                        MirBinOp::EqString
                    } else {
                        MirBinOp::EqInt
                    };
                    self.emit(
                        body,
                        Instruction::BinOp {
                            dest: field_eq.clone(),
                            op: field_op,
                            lhs: Operand::Var(l_field),
                            rhs: Operand::Var(r_field),
                        },
                    );
                    field_conds.push(field_eq);
                }

                // Empty struct: always equal
                if field_conds.is_empty() {
                    let dest = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::Const {
                            dest: dest.clone(),
                            value: if op == BinOp::Eq {
                                Constant::Bool(true)
                            } else {
                                Constant::Bool(false)
                            },
                        },
                    );
                    return Some(dest);
                }

                // AND all field comparisons together
                let mut result = field_conds[0].clone();
                for cond in &field_conds[1..] {
                    let combined = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::BinOp {
                            dest: combined.clone(),
                            op: MirBinOp::And,
                            lhs: Operand::Var(result),
                            rhs: Operand::Var(cond.clone()),
                        },
                    );
                    result = combined;
                }

                // For NotEq: negate the result
                if op == BinOp::NotEq {
                    let negated = self.fresh_temp();
                    self.emit(
                        body,
                        Instruction::Not {
                            dest: negated.clone(),
                            operand: Operand::Var(result),
                        },
                    );
                    result = negated;
                }

                Some(result)
            }

            // Ord: only for single-field value types (spec §8.6)
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
                if fields.len() != 1 {
                    return None; // Multi-field values don't derive Ord
                }
                let (_, field_ty) = &fields[0];

                let l_field = self.fresh_temp();
                self.emit(
                    body,
                    Instruction::FieldGet {
                        dest: l_field.clone(),
                        obj: Operand::Var(l.to_string()),
                        type_name: type_name.clone(),
                        field_index: 0,
                    },
                );
                let r_field = self.fresh_temp();
                self.emit(
                    body,
                    Instruction::FieldGet {
                        dest: r_field.clone(),
                        obj: Operand::Var(r.to_string()),
                        type_name: type_name.clone(),
                        field_index: 0,
                    },
                );

                let is_float = *field_ty == Ty::Float;
                let mir_op = super::ast_binop_to_mir(op, is_float);
                let dest = self.fresh_temp();
                self.emit(
                    body,
                    Instruction::BinOp {
                        dest: dest.clone(),
                        op: mir_op,
                        lhs: Operand::Var(l_field),
                        rhs: Operand::Var(r_field),
                    },
                );
                Some(dest)
            }

            _ => None,
        }
    }

    /// Resolve the struct type (value or data) of an expression.
    /// Returns (type_name, field_defs) if the expression is a known struct-typed binding.
    pub(super) fn resolve_struct_type(&self, expr: &Expr) -> Option<(String, Vec<(String, Ty)>)> {
        match &expr.kind {
            ExprKind::Ident(name) => {
                // If this is `self` in an impl method, return the known self type
                if name == "self" {
                    if let Some(type_name) = &self.self_type {
                        if let Some(fields) = self.struct_fields.get(type_name) {
                            return Some((type_name.clone(), fields.clone()));
                        }
                    }
                }

                // Check var_types for tracked variable types
                if let Some(type_name) = self.var_types.get(name) {
                    if let Some(fields) = self.struct_fields.get(type_name) {
                        return Some((type_name.clone(), fields.clone()));
                    }
                }

                None
            }
            ExprKind::FieldAccess(obj, field) => {
                // Chained field access: resolve inner object, then look up field type
                if let Some((_parent_type, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some((_, Ty::Named(field_type_name))) =
                        field_defs.iter().find(|(n, _)| n == field)
                    {
                        if let Some(inner_fields) = self.struct_fields.get(field_type_name) {
                            return Some((field_type_name.clone(), inner_fields.clone()));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Resolve a method call to a mangled impl function name.
    /// Tries type-specific lookup first, falls back to method-name-only if unambiguous.
    pub(super) fn resolve_impl_method(&self, obj: &Expr, method: &str) -> ImplMethodResult {
        // Try type-specific lookup
        if let Some((type_name, _)) = self.resolve_struct_type(obj) {
            let key = (type_name, method.to_string());
            if let Some(mangled) = self.impl_methods.get(&key) {
                return ImplMethodResult::Resolved(mangled.clone());
            }
        }

        // Fall back: if exactly one impl has this method, use it
        let matches: Vec<_> = self
            .impl_methods
            .iter()
            .filter(|((_, mn), _)| mn == method)
            .collect();
        if matches.len() == 1 {
            return ImplMethodResult::Resolved(matches[0].1.clone());
        }
        if matches.len() > 1 {
            return ImplMethodResult::Ambiguous;
        }

        ImplMethodResult::NotFound
    }

    /// Infer if an expression is a Map<K, V> type from variable tracking.
    /// Returns the full generic Ty so callers can dispatch on K/V.
    pub(super) fn infer_map_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::Ident(name) => self
                .generic_var_types
                .get(name)
                .filter(|t| matches!(t, Ty::Generic(n, _) if n == "Map"))
                .cloned(),
            _ => None,
        }
    }

    /// Returns the full LinkedMap<K,V> Ty for type inference (ADR-0019).
    pub(super) fn infer_linked_map_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::Ident(name) => self
                .generic_var_types
                .get(name)
                .filter(|t| t.is_linked_map())
                .cloned(),
            ExprKind::Call(callee, args) => {
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    // lm.insert(k, v) returns LinkedMap<K,V>: recurse to unwrap chain.
                    if method == "insert" {
                        return self.infer_linked_map_type(obj);
                    }
                    // LinkedMap.new() as a receiver
                    if let ExprKind::Ident(mod_name) = &obj.kind
                        && mod_name == "LinkedMap"
                        && method == "new"
                        && args.is_empty()
                    {
                        let (k_ty, v_ty) = self
                            .binding_type_hint
                            .as_ref()
                            .and_then(|h| h.linked_map_kv())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .unwrap_or((Ty::String, Ty::Int));
                        return Some(Ty::Generic("LinkedMap".into(), vec![k_ty, v_ty]));
                    }
                }
                if let ExprKind::Ident(func_name) = &callee.kind {
                    if let Some(ret_ty) = self.fn_return_types.get(func_name.as_str()) {
                        if ret_ty.is_linked_map() {
                            return Some(ret_ty.clone());
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Returns the full LinkedSet<T> Ty for type inference (ADR-0019).
    pub(super) fn infer_linked_set_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::Ident(name) => self
                .generic_var_types
                .get(name)
                .filter(|t| t.is_linked_set())
                .cloned(),
            ExprKind::Call(callee, args) => {
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    // ls.insert(x) returns LinkedSet<T>: recurse to unwrap chain.
                    if method == "insert" {
                        return self.infer_linked_set_type(obj);
                    }
                    // LinkedSet.new() as a receiver
                    if let ExprKind::Ident(mod_name) = &obj.kind
                        && mod_name == "LinkedSet"
                        && method == "new"
                        && args.is_empty()
                    {
                        let elem_ty = self
                            .binding_type_hint
                            .as_ref()
                            .and_then(|h| h.linked_set_elem())
                            .or_else(|| self.current_fn_return_type.linked_set_elem())
                            .cloned()
                            .unwrap_or(Ty::Int);
                        return Some(Ty::Generic("LinkedSet".into(), vec![elem_ty]));
                    }
                }
                if let ExprKind::Ident(func_name) = &callee.kind {
                    if let Some(ret_ty) = self.fn_return_types.get(func_name.as_str()) {
                        if ret_ty.is_linked_set() {
                            return Some(ret_ty.clone());
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn infer_set_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::Ident(name) => self
                .generic_var_types
                .get(name)
                .filter(|t| t.is_set())
                .cloned(),
            ExprKind::Call(callee, args) => {
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    // s.insert(x) returns Set<T>: recurse to unwrap any chain depth.
                    if method == "insert" {
                        return self.infer_set_type(obj);
                    }
                    // set.new() as a receiver — resolve T using the same priority
                    // as the set.new() handler in lower_call.
                    if let ExprKind::Ident(mod_name) = &obj.kind
                        && mod_name == "set"
                        && method == "new"
                        && args.is_empty()
                    {
                        let elem_ty = self
                            .binding_type_hint
                            .as_ref()
                            .and_then(|h| h.set_elem())
                            .or_else(|| self.current_fn_return_type.set_elem())
                            .cloned()
                            .unwrap_or(Ty::Int);
                        return Some(Ty::Generic("Set".into(), vec![elem_ty]));
                    }
                }
                // General call: look up declared return type to cover
                // make_set().contains(x) / make_set().insert(y) / make_set().len().
                if let ExprKind::Ident(func_name) = &callee.kind {
                    if let Some(ret_ty) = self.fn_return_types.get(func_name.as_str()) {
                        if ret_ty.is_set() {
                            return Some(ret_ty.clone());
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Infer if an expression is a List<T> type from variable tracking.
    pub(super) fn infer_list_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::Ident(name) => self
                .generic_var_types
                .get(name)
                .filter(|t| t.is_list())
                .cloned(),
            ExprKind::ListLit(items) => {
                let elem_ty = items
                    .first()
                    .and_then(|e| self.infer_expr_type(e))
                    .unwrap_or(Ty::Int);
                Some(Ty::Generic("List".into(), vec![elem_ty]))
            }
            _ => None,
        }
    }

    /// Infer the type of an expression for ADT type tracking.
    pub(super) fn infer_expr_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::IntLit(_) => Some(Ty::Int),
            ExprKind::FloatLit(_) => Some(Ty::Float),
            ExprKind::StringLit(_) | ExprKind::StringInterp(_) => Some(Ty::String),
            ExprKind::BoolLit(_) => Some(Ty::Bool),
            ExprKind::UnitLit => Some(Ty::Unit),
            ExprKind::Ident(name) => {
                if self.float_vars.contains(name) {
                    Some(Ty::Float)
                } else if self.string_vars.contains(name) {
                    Some(Ty::String)
                } else if self.generic_var_types.contains_key(name) {
                    self.generic_var_types.get(name).cloned()
                } else if let Some(type_name) = self.var_types.get(name) {
                    // value / data type binding (e.g. `acc: Account`).
                    // Used by `.copy()` inference so `Ok(acc.copy(...))`
                    // constructs Result<Account, E> rather than defaulting
                    // to Result<Int, E>.
                    Some(Ty::Named(type_name.clone()))
                } else {
                    // Cannot determine type from tracking alone; caller should
                    // handle None (e.g., by falling back to function return type).
                    None
                }
            }
            ExprKind::ListLit(items) => {
                let elem_ty = items
                    .first()
                    .and_then(|e| self.infer_expr_type(e))
                    .unwrap_or(Ty::Int);
                Some(Ty::Generic("List".into(), vec![elem_ty]))
            }
            ExprKind::BinaryOp(lhs, _, _) => {
                if self.is_float_expr(lhs) {
                    Some(Ty::Float)
                } else {
                    Some(Ty::Int)
                }
            }
            ExprKind::Call(callee, args) => {
                if let ExprKind::Ident(fname) = &callee.kind {
                    // Check if it's a prelude ADT constructor: Some(x), Ok(x), Err(e)
                    if let Some(first_arg) = args.first() {
                        if let Some(adt_ty) = self
                            .infer_expr_type(&first_arg.value)
                            .and_then(|arg_ty| self.infer_adt_call_type(fname, &arg_ty))
                        {
                            return Some(adt_ty);
                        }
                    }
                    // ADT constructor with args: TypeName(field: val)
                    if self.struct_fields.contains_key(fname.as_str())
                        || self.adt_struct_defs.contains_key(fname.as_str())
                    {
                        return Some(Ty::Named(fname.clone()));
                    }
                    self.fn_return_types.get(fname).cloned()
                } else if let ExprKind::FieldAccess(obj, variant) = &callee.kind {
                    if let ExprKind::Ident(type_or_module) = &obj.kind {
                        // ADT payload constructor: TypeName.Variant(args)
                        if self
                            .variant_tags
                            .contains_key(&(type_or_module.clone(), variant.clone()))
                        {
                            return Some(Ty::Named(type_or_module.clone()));
                        }
                        // Module-qualified struct constructor: module.TypeName(args)
                        if self.struct_fields.contains_key(variant.as_str()) {
                            return Some(Ty::Named(variant.clone()));
                        }
                    }
                    // .copy() on a value type preserves the receiver's type
                    // (§8.6). Without this, expressions like
                    // `Ok(acc.copy(balance: acc.balance + amount))` fall
                    // through to the Ty::Int fallback in the Ok/Err
                    // constructor lowering and produce a bogus
                    // `Result<Int, E>` — failing LLVM type check at the
                    // insertvalue step.
                    if variant == "copy" {
                        return self.infer_expr_type(obj);
                    }
                    // set.new() — return Set<T> inferred from context so
                    // Some(set.new()) in `-> Option<Set<T>>` builds the
                    // correct Option__Set__T struct, not Option__Int.
                    if let ExprKind::Ident(mod_name) = &obj.kind
                        && mod_name == "set"
                        && variant == "new"
                    {
                        let set_ty = self
                            .binding_type_hint
                            .as_ref()
                            .and_then(|h| peel_to_set(h))
                            .or_else(|| peel_to_set(&self.current_fn_return_type))
                            .cloned()
                            .unwrap_or_else(|| Ty::Generic("Set".into(), vec![Ty::Int]));
                        return Some(set_ty);
                    }
                    // Module-qualified function call: `string.substring(...)`,
                    // `list.get(...)`, etc. Same logic as call_returns_type
                    // above — without this, the constructor inference at
                    // call.rs (Some/Ok/Err) falls back to Ty::Int and
                    // builds an Option<Int> / Result<Int, _> when the
                    // payload is actually a String / List / etc.
                    if let ExprKind::Ident(module_name) = &obj.kind {
                        if self.imported_modules.contains(module_name.as_str()) {
                            let qualified = format!("{module_name}__{variant}");
                            if let Some(ty) = self.fn_return_types.get(&qualified) {
                                return Some(ty.clone());
                            }
                        }
                    }
                    // String value method call: `s.byte_at(i)` etc. Mirrors
                    // the auto-rewrite in call.rs so the inferred return
                    // type matches the rewritten call's return type.
                    let qualified = format!("string__{variant}");
                    if self.is_string_expr(obj)
                        && let Some(ty) = self.fn_return_types.get(&qualified)
                    {
                        return Some(ty.clone());
                    }
                    // Instance method call on a variable with a known generic
                    // type (Map/Set/List).  Without this, assert.eq dispatch
                    // falls back to Ty::Int for Bool-returning methods like
                    // contains_key/contains, producing an i1/i64 LLVM mismatch.
                    if let ExprKind::Ident(var_name) = &obj.kind {
                        if let Some(Ty::Generic(gname, gargs)) =
                            self.generic_var_types.get(var_name.as_str())
                        {
                            match (gname.as_str(), variant.as_str()) {
                                ("Map", "contains_key")
                                | ("Set", "contains")
                                | ("LinkedMap", "contains_key")
                                | ("LinkedSet", "contains") => {
                                    return Some(Ty::Bool);
                                }
                                ("Map", "len")
                                | ("Set", "len")
                                | ("List", "len")
                                | ("LinkedMap", "len")
                                | ("LinkedSet", "len") => {
                                    return Some(Ty::Int);
                                }
                                ("Map", "get") | ("LinkedMap", "get") => {
                                    let val_ty = gargs.get(1).cloned().unwrap_or(Ty::Int);
                                    return Some(Ty::Generic("Option".into(), vec![val_ty]));
                                }
                                ("List", "get") => {
                                    let elem_ty = gargs.first().cloned().unwrap_or(Ty::Int);
                                    return Some(Ty::Generic("Option".into(), vec![elem_ty]));
                                }
                                _ => {}
                            }
                        }
                    }
                    None
                } else {
                    None
                }
            }
            ExprKind::FieldAccess(obj, field) => {
                // ADT unit variant constructor: TypeName.Variant
                if let ExprKind::Ident(type_name) = &obj.kind {
                    if self
                        .variant_tags
                        .contains_key(&(type_name.clone(), field.clone()))
                    {
                        return Some(Ty::Named(type_name.clone()));
                    }
                }
                // Struct field access: look up var_types → struct_fields
                if let ExprKind::Ident(var_name) = &obj.kind {
                    // Special case: `self` in impl methods uses self_type
                    let struct_name = if var_name == "self" {
                        self.self_type.as_deref().and_then(|sn| {
                            if self.struct_fields.contains_key(sn) {
                                Some(sn)
                            } else {
                                None
                            }
                        })
                    } else {
                        self.var_types.get(var_name.as_str()).map(|s| s.as_str())
                    };
                    if let Some(sname) = struct_name {
                        if let Some(fields) = self.struct_fields.get(sname) {
                            if let Some((_, fty)) = fields.iter().find(|(n, _)| n == field) {
                                return Some(fty.clone());
                            }
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
}

/// Walk through Option/Result/List wrappers to extract an inner `Set<T>` type.
/// Returns `Some(&Set<T>)` if found, `None` otherwise.
fn peel_to_set(ty: &Ty) -> Option<&Ty> {
    if ty.is_set() {
        return Some(ty);
    }
    if let Ty::Generic(name, args) = ty {
        match name.as_str() {
            "Option" if args.len() == 1 => return peel_to_set(&args[0]),
            "Result" if args.len() == 2 => return peel_to_set(&args[0]),
            "List" if args.len() == 1 => return peel_to_set(&args[0]),
            _ => {}
        }
    }
    None
}
