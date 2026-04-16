// AST to MIR lowering.
//
// Walks the AST and produces a flat sequence of MIR instructions.
// Expressions are flattened into named temporaries.
// Control flow is desugared into labels and branches.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;

/// Lower a source file to MIR.
pub fn lower(file: &SourceFile) -> Program {
    let mut ctx = LowerCtx::new();

    let has_explicit_main = file
        .items
        .iter()
        .any(|item| matches!(item, Item::FnDef(f) if f.name == "main"));

    let has_top_level_stmts = file.items.iter().any(|item| matches!(item, Item::Stmt(_)));

    // ADR-0006 Rule 2: fn main and top-level statements are mutually exclusive.
    // This should already be caught by the parser/resolver, but we enforce it here
    // defensively to avoid producing invalid MIR with duplicate main functions.
    assert!(
        !(has_explicit_main && has_top_level_stmts),
        "BUG: fn main and top-level statements both present (ADR-0006 Rule 2 violation)"
    );

    // Collect type definitions for ADT tag assignment and value field info
    for item in &file.items {
        match item {
            Item::TypeDef(t) => {
                if let TypeDefKind::Adt(variants) = &t.kind {
                    for (i, variant) in variants.iter().enumerate() {
                        ctx.variant_tags
                            .insert((t.name.clone(), variant.name.clone()), i as i64);
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

    // Collect impl block methods for method dispatch (§8.7)
    for item in &file.items {
        if let Item::ImplDef(impl_def) = item {
            if let TypeExprKind::Named(target_name) = &impl_def.target_type.kind {
                for method in &impl_def.methods {
                    let mangled = format!("{target_name}__{}", method.name);
                    ctx.impl_methods.insert(
                        (target_name.clone(), method.name.clone()),
                        mangled,
                    );
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

    // Lower top-level statements into an implicit main (§6.1)
    if has_top_level_stmts {
        let mut body = Vec::new();
        for item in &file.items {
            if let Item::Stmt(s) = item {
                ctx.lower_stmt(s, &mut body);
            }
        }
        body.push(Instruction::Return { value: None });

        ctx.functions.push(Function {
            name: "main".into(),
            params: vec![],
            return_type: Ty::Unit,
            body,
            is_main: true,
        });
    }

    let struct_defs = ctx
        .struct_fields
        .iter()
        .map(|(name, fields)| crate::ir::StructDef {
            name: name.clone(),
            fields: fields.clone(),
        })
        .collect();

    Program {
        functions: ctx.functions,
        string_constants: ctx.string_constants,
        struct_defs,
    }
}

struct LowerCtx {
    functions: Vec<Function>,
    string_constants: Vec<String>,
    temp_counter: u32,
    label_counter: u32,
    /// ADT variant tag map: (type_name, variant_name) -> tag index
    variant_tags: std::collections::HashMap<(String, String), i64>,
    /// Struct field info for value and data types: type_name -> list of (field_name, field_type)
    struct_fields: std::collections::HashMap<String, Vec<(String, Ty)>>,
    /// Set of type names that are data types (reference semantics, §8.6).
    /// Used to prevent copy() on data types and for future mut field handling.
    data_types: std::collections::HashSet<String>,
    /// Tracks variables/temps known to hold Float values (for correct binop selection)
    float_vars: std::collections::HashSet<String>,
    /// Impl method registry: (target_type_name, method_name) → mangled_fn_name
    impl_methods: std::collections::HashMap<(String, String), String>,
    /// Current self type when lowering impl method bodies (None outside impl methods)
    self_type: Option<String>,
}

/// Result of resolving an impl method call.
enum ImplMethodResult {
    /// Resolved to a mangled function name.
    Resolved(String),
    /// Multiple impls define this method; can't disambiguate without type info.
    Ambiguous,
    /// No impl found for this method name.
    NotFound,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            functions: Vec::new(),
            string_constants: Vec::new(),
            temp_counter: 0,
            label_counter: 0,
            variant_tags: std::collections::HashMap::new(),
            struct_fields: std::collections::HashMap::new(),
            data_types: std::collections::HashSet::new(),
            float_vars: std::collections::HashSet::new(),
            impl_methods: std::collections::HashMap::new(),
            self_type: None,
        }
    }

    fn fresh_temp(&mut self) -> String {
        let t = format!("_t{}", self.temp_counter);
        self.temp_counter += 1;
        t
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
        self.float_vars.clear();

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

        let mut body = Vec::new();
        for stmt in &f.body {
            self.lower_stmt(stmt, &mut body);
        }

        // If last instruction isn't a return, add implicit return
        if !matches!(body.last(), Some(Instruction::Return { .. })) {
            if return_type == Ty::Unit {
                body.push(Instruction::Return { value: None });
            } else if let Some(last_temp) = self.last_temp_name(&body) {
                body.push(Instruction::Return {
                    value: Some(Operand::Var(last_temp)),
                });
            } else {
                body.push(Instruction::Return { value: None });
            }
        }

        Function {
            name: f.name.clone(),
            params,
            return_type,
            body,
            is_main: false,
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt, body: &mut Vec<Instruction>) {
        match stmt {
            Stmt::Let(s) => {
                let is_float = self.is_float_expr(&s.value);
                let val = self.lower_expr(&s.value, body);
                if is_float {
                    self.float_vars.insert(s.name.clone());
                }
                body.push(Instruction::Copy {
                    dest: s.name.clone(),
                    source: val,
                });
            }
            Stmt::Mut(s) => {
                let is_float = self.is_float_expr(&s.value);
                let val = self.lower_expr(&s.value, body);
                if is_float {
                    self.float_vars.insert(s.name.clone());
                }
                body.push(Instruction::Copy {
                    dest: s.name.clone(),
                    source: val,
                });
            }
            Stmt::Return(s) => {
                let value = s.value.as_ref().map(|v| {
                    let t = self.lower_expr(v, body);
                    Operand::Var(t)
                });
                body.push(Instruction::Return { value });
            }
            Stmt::Defer(_) => {
                // defer lowering: deferred to later milestone
                // For now, the deferred expression is simply ignored in MIR
            }
            Stmt::Expr(s) => {
                self.lower_expr(&s.expr, body);
            }
        }
    }

    /// Lower an expression, returning the name of the temporary holding the result.
    fn lower_expr(&mut self, expr: &Expr, body: &mut Vec<Instruction>) -> String {
        match &expr.kind {
            ExprKind::IntLit(n) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Int(*n),
                });
                dest
            }
            ExprKind::FloatLit(f) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Float(*f),
                });
                dest
            }
            ExprKind::StringLit(s) => {
                let idx = self.intern_string(s);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::StringRef(idx),
                });
                dest
            }
            ExprKind::BoolLit(b) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Bool(*b),
                });
                dest
            }
            ExprKind::UnitLit => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Ident(name) => name.clone(),

            ExprKind::BinaryOp(lhs, op, rhs) => {
                let l = self.lower_expr(lhs, body);
                let r = self.lower_expr(rhs, body);
                let dest = self.fresh_temp();
                let is_float = self.is_float_expr(lhs) || self.is_float_expr(rhs);
                let mir_op = ast_binop_to_mir(*op, is_float);
                body.push(Instruction::BinOp {
                    dest: dest.clone(),
                    op: mir_op,
                    lhs: Operand::Var(l),
                    rhs: Operand::Var(r),
                });
                dest
            }

            ExprKind::UnaryOp(op, operand) => {
                let val = self.lower_expr(operand, body);
                let dest = self.fresh_temp();
                match op {
                    UnaryOp::Neg => {
                        body.push(Instruction::Neg {
                            dest: dest.clone(),
                            operand: Operand::Var(val),
                        });
                    }
                    UnaryOp::Not => {
                        body.push(Instruction::Not {
                            dest: dest.clone(),
                            operand: Operand::Var(val),
                        });
                    }
                }
                dest
            }

            ExprKind::Call(callee, args) => {
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
                    for (fname, _fty) in &field_defs {
                        // First try label match
                        let labeled = args.iter().enumerate().find(|(idx, a)| {
                            !used_args.contains(idx) && a.label.as_deref() == Some(fname)
                        });
                        let resolved = if let Some((idx, a)) = labeled {
                            used_args.insert(idx);
                            Some(a)
                        } else {
                            // Positional fallback: next unused arg
                            let positional = args.iter().enumerate().find(|(idx, _)| {
                                !used_args.contains(idx)
                            });
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

                // Check for .copy() on value types only (§8.6)
                // copy() is NOT available on data types.
                if let ExprKind::FieldAccess(obj, method) = &callee.kind
                    && method == "copy"
                {
                    if let Some((type_name, field_defs)) = self.resolve_struct_type(obj) {
                        if !self.data_types.contains(&type_name) {
                            let obj_val = self.lower_expr(obj, body);
                            return self.lower_copy(
                                &obj_val, &type_name, &field_defs, args, body,
                            );
                        }
                    }
                    // Not a value type — fall through to method call or generic call
                }

                // Check for impl trait method call: p.method() → Type__method(p)
                // (§8.7 static dispatch)
                if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                    match self.resolve_impl_method(obj, method) {
                        ImplMethodResult::Resolved(mangled_name) => {
                            let self_val = self.lower_expr(obj, body);
                            let mut arg_operands = vec![Operand::Var(self_val)];
                            for a in args {
                                let t = self.lower_expr(&a.value, body);
                                arg_operands.push(Operand::Var(t));
                            }
                            let dest = self.fresh_temp();
                            body.push(Instruction::Call {
                                dest: Some(dest.clone()),
                                func: mangled_name,
                                args: arg_operands,
                            });
                            return dest;
                        }
                        ImplMethodResult::Ambiguous => {
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
                        ImplMethodResult::NotFound => {
                            // Not an impl method — fall through to generic call
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

                let func_name = match &callee.kind {
                    ExprKind::Ident(name) => name.clone(),
                    ExprKind::FieldAccess(obj, method) => {
                        let obj_name = self.lower_expr(obj, body);
                        format!("{obj_name}.{method}")
                    }
                    _ => self.lower_expr(callee, body),
                };

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
                    func: func_name,
                    args: arg_operands,
                });
                dest
            }

            ExprKind::Assign(lhs, rhs) => {
                let val = self.lower_expr(rhs, body);
                if let ExprKind::Ident(name) = &lhs.kind {
                    body.push(Instruction::Copy {
                        dest: name.clone(),
                        source: val.clone(),
                    });
                }
                val
            }

            ExprKind::If(if_expr) => self.lower_if(if_expr, body),

            ExprKind::Match(m) => self.lower_match(m, body),

            ExprKind::For(f) => {
                let iter_val = self.lower_expr(&f.iter, body);
                // Simplified: lower body once (no actual iteration in MIR yet)
                body.push(Instruction::Copy {
                    dest: f.binding.clone(),
                    source: iter_val,
                });
                for stmt in &f.body {
                    self.lower_stmt(stmt, body);
                }
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::While(w) => {
                let loop_label = self.fresh_label("while");
                let end_label = self.fresh_label("while_end");

                body.push(Instruction::Label(loop_label.clone()));
                let cond = self.lower_expr(&w.condition, body);
                body.push(Instruction::BranchIf {
                    cond: Operand::Var(cond),
                    true_label: format!("{loop_label}_body"),
                    false_label: end_label.clone(),
                });
                body.push(Instruction::Label(format!("{loop_label}_body")));
                for stmt in &w.body {
                    self.lower_stmt(stmt, body);
                }
                body.push(Instruction::Jump { label: loop_label });
                body.push(Instruction::Label(end_label));

                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Propagate(inner) => {
                // ? operator: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Await(inner) => {
                // .await: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Spawn(inner) => self.lower_expr(inner, body),

            ExprKind::FieldAccess(obj, field) => {
                // Check if this is an ADT constructor: Color.Red → tag constant
                if let ExprKind::Ident(type_name) = &obj.kind
                    && let Some(&tag) = self.variant_tags.get(&(type_name.clone(), field.clone()))
                {
                    let dest = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: dest.clone(),
                        value: Constant::Int(tag),
                    });
                    return dest;
                }

                let obj_val = self.lower_expr(obj, body);

                // Value type field access: emit FieldGet instruction
                if let Some((type_name, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some(idx) = field_defs.iter().position(|(n, _)| n == field) {
                        let dest = self.fresh_temp();
                        body.push(Instruction::FieldGet {
                            dest: dest.clone(),
                            obj: Operand::Var(obj_val),
                            type_name,
                            field_index: idx as u32,
                        });
                        return dest;
                    }
                }

                // General field access (data types, methods)
                // TODO: emit proper GEP instruction for data type struct field access
                let dest = self.fresh_temp();
                body.push(Instruction::Copy {
                    dest: dest.clone(),
                    source: format!("{obj_val}.{field}"),
                });
                dest
            }

            ExprKind::Index(obj, idx) => {
                self.lower_expr(obj, body);
                self.lower_expr(idx, body);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::Lambda(_) | ExprKind::TurbofishCall(_, _, _) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::ListLit(items) => {
                for item in items {
                    self.lower_expr(item, body);
                }
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::MapLit(entries) => {
                for (k, v) in entries {
                    self.lower_expr(k, body);
                    self.lower_expr(v, body);
                }
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::StringInterp(parts) => {
                // TODO: String interpolation requires runtime string concatenation.
                // Current implementation only includes literal parts and evaluates
                // (but discards) interpolated expressions. Full implementation needs
                // a runtime concat/format function.
                let mut combined = String::new();
                for part in parts {
                    match part {
                        StringPart::Lit(s) => combined.push_str(s),
                        StringPart::Expr(e) => {
                            // Evaluate for side effects, but result is discarded
                            self.lower_expr(e, body);
                        }
                    }
                }
                let idx = self.intern_string(&combined);
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::StringRef(idx),
                });
                dest
            }
        }
    }

    fn lower_if(&mut self, if_expr: &IfExpr, body: &mut Vec<Instruction>) -> String {
        let cond = self.lower_expr(&if_expr.condition, body);
        let then_label = self.fresh_label("then");
        let else_label = self.fresh_label("else");
        let end_label = self.fresh_label("if_end");

        body.push(Instruction::BranchIf {
            cond: Operand::Var(cond),
            true_label: then_label.clone(),
            false_label: else_label.clone(),
        });

        // Then branch
        body.push(Instruction::Label(then_label));
        for stmt in &if_expr.then_body {
            self.lower_stmt(stmt, body);
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        // Else branch
        body.push(Instruction::Label(else_label));
        match &if_expr.else_body {
            Some(ElseBranch::Else(stmts)) => {
                for stmt in stmts {
                    self.lower_stmt(stmt, body);
                }
            }
            Some(ElseBranch::ElseIf(inner)) => {
                self.lower_if(inner, body);
            }
            None => {}
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        body.push(Instruction::Label(end_label));

        let dest = self.fresh_temp();
        body.push(Instruction::Const {
            dest: dest.clone(),
            value: Constant::Unit,
        });
        dest
    }

    /// Lower a match expression into a chain of conditional branches.
    /// Uses alloca + store + load pattern for the result to avoid SSA dominance issues.
    fn lower_match(&mut self, m: &MatchExpr, body: &mut Vec<Instruction>) -> String {
        let subject = self.lower_expr(&m.subject, body);
        let end_label = self.fresh_label("match_end");

        // Allocate stack slot for match result
        let result_slot = self.fresh_temp();
        body.push(Instruction::Alloca {
            dest: result_slot.clone(),
        });

        // Pre-generate all labels
        let arm_labels: Vec<String> = (0..m.arms.len())
            .map(|i| self.fresh_label(&format!("arm_{i}")))
            .collect();
        let next_labels: Vec<String> = (0..m.arms.len())
            .map(|i| {
                if i + 1 < m.arms.len() {
                    self.fresh_label(&format!("next_{i}"))
                } else {
                    end_label.clone()
                }
            })
            .collect();

        for (i, arm) in m.arms.iter().enumerate() {
            let arm_label = &arm_labels[i];
            let next_label = &next_labels[i];

            // Generate pattern test
            match &arm.pattern.kind {
                PatternKind::Wildcard | PatternKind::Ident(_) => {
                    body.push(Instruction::Jump {
                        label: arm_label.clone(),
                    });
                }
                PatternKind::IntLit(n) => {
                    let lit = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: lit.clone(),
                        value: Constant::Int(*n),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::EqInt,
                        lhs: Operand::Var(subject.clone()),
                        rhs: Operand::Var(lit),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: arm_label.clone(),
                        false_label: next_label.clone(),
                    });
                }
                PatternKind::BoolLit(b) => {
                    let lit = self.fresh_temp();
                    body.push(Instruction::Const {
                        dest: lit.clone(),
                        value: Constant::Bool(*b),
                    });
                    let cond = self.fresh_temp();
                    body.push(Instruction::BinOp {
                        dest: cond.clone(),
                        op: MirBinOp::EqInt,
                        lhs: Operand::Var(subject.clone()),
                        rhs: Operand::Var(lit),
                    });
                    body.push(Instruction::BranchIf {
                        cond: Operand::Var(cond),
                        true_label: arm_label.clone(),
                        false_label: next_label.clone(),
                    });
                }
                PatternKind::StringLit(_) | PatternKind::FloatLit(_) => {
                    body.push(Instruction::Jump {
                        label: arm_label.clone(),
                    });
                }
                PatternKind::Constructor(variant_name, _) => {
                    // Look up tag for this variant across all ADT types.
                    // KNOWN LIMITATION: If two ADTs share a variant name, this
                    // picks the first match non-deterministically. The spec (§8.5)
                    // says the correct variant is determined by the match subject's
                    // type, but type info is not yet threaded through MIR lowering.
                    // TODO: Thread subject type info for correct ADT disambiguation.
                    let tag = self
                        .variant_tags
                        .iter()
                        .find(|((_, vn), _)| vn == variant_name)
                        .map(|(_, &t)| t);

                    if let Some(tag) = tag {
                        let lit = self.fresh_temp();
                        body.push(Instruction::Const {
                            dest: lit.clone(),
                            value: Constant::Int(tag),
                        });
                        let cond = self.fresh_temp();
                        body.push(Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqInt,
                            lhs: Operand::Var(subject.clone()),
                            rhs: Operand::Var(lit),
                        });
                        body.push(Instruction::BranchIf {
                            cond: Operand::Var(cond),
                            true_label: arm_label.clone(),
                            false_label: next_label.clone(),
                        });
                    } else {
                        // Unknown constructor — fall through (treat as wildcard)
                        body.push(Instruction::Jump {
                            label: arm_label.clone(),
                        });
                    }
                }
            }

            // Arm body
            body.push(Instruction::Label(arm_label.clone()));

            if let PatternKind::Ident(name) = &arm.pattern.kind {
                body.push(Instruction::Copy {
                    dest: name.clone(),
                    source: subject.clone(),
                });
            }

            // Track arm body start to find the last temp from THIS arm only
            let arm_body_start = body.len();
            for stmt in &arm.body {
                self.lower_stmt(stmt, body);
            }

            // Store arm result into the alloca'd slot (scan only this arm's instructions)
            if let Some(last) = self.last_temp_in_range(body, arm_body_start) {
                body.push(Instruction::Store {
                    dest: result_slot.clone(),
                    value: Operand::Var(last),
                });
            }

            body.push(Instruction::Jump {
                label: end_label.clone(),
            });

            // Next arm label
            if i + 1 < m.arms.len() {
                body.push(Instruction::Label(next_label.clone()));
            }
        }

        body.push(Instruction::Label(end_label));

        // Load the result from the alloca'd slot
        let result = self.fresh_temp();
        body.push(Instruction::Load {
            dest: result.clone(),
            source: result_slot,
        });
        result
    }

    /// Check if an expression produces a Float value.
    /// Used to select between Int and Float MIR binary operations.
    fn is_float_expr(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::FloatLit(_) => true,
            ExprKind::Ident(name) => self.float_vars.contains(name),
            ExprKind::FieldAccess(obj, field) => {
                // Check if field access on a value type yields Float
                if let Some((_type_name, field_defs)) = self.resolve_struct_type(obj) {
                    if let Some((_, ty)) = field_defs.iter().find(|(n, _)| n == field) {
                        return *ty == Ty::Float;
                    }
                }
                false
            }
            ExprKind::BinaryOp(lhs, _op, rhs) => {
                self.is_float_expr(lhs) || self.is_float_expr(rhs)
            }
            ExprKind::UnaryOp(_, inner) => self.is_float_expr(inner),
            _ => false,
        }
    }

    /// Resolve the struct type (value or data) of an expression.
    /// Returns (type_name, field_defs) if the expression is a known struct-typed binding.
    fn resolve_struct_type(&self, expr: &Expr) -> Option<(String, Vec<(String, Ty)>)> {
        // For identifiers, check if any value type has matching fields.
        // In the absence of full type tracking, we check all value types.
        // TODO: Thread proper type information from the type checker.
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

                // Heuristic: if there's exactly one value type with >0 fields, use it.
                // With multiple value types, we can't disambiguate without type info.
                let value_types: Vec<_> = self.struct_fields.iter().collect();
                if value_types.len() == 1 {
                    let (tn, fields) = value_types[0];
                    return Some((tn.clone(), fields.clone()));
                }
                // Multiple value types: best-effort heuristic
                // TODO: Proper type tracking would solve this.
                if value_types.len() > 1 {
                    let (tn, fields) = value_types[0];
                    return Some((tn.clone(), fields.clone()));
                }
                None
            }
            _ => None,
        }
    }

    /// Resolve a method call to a mangled impl function name.
    /// Tries type-specific lookup first, falls back to method-name-only if unambiguous.
    fn resolve_impl_method(&self, obj: &Expr, method: &str) -> ImplMethodResult {
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

    /// Lower a .copy() call on a value type.
    /// Extracts all fields from the original, overrides specified fields, builds new struct.
    fn lower_copy(
        &mut self,
        obj_val: &str,
        type_name: &str,
        field_defs: &[(String, Ty)],
        args: &[Arg],
        body: &mut Vec<Instruction>,
    ) -> String {
        // If no args, return the original (value types are immutable, copy is identity)
        if args.is_empty() {
            return obj_val.to_string();
        }

        // Build override map: field_name → lowered operand
        let mut overrides: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for arg in args {
            if let Some(label) = &arg.label {
                let val = self.lower_expr(&arg.value, body);
                overrides.insert(label.clone(), val);
            }
        }

        // For each field: use override if present, otherwise extract from original
        let mut field_operands = Vec::with_capacity(field_defs.len());
        for (i, (fname, _fty)) in field_defs.iter().enumerate() {
            if let Some(override_val) = overrides.get(fname) {
                field_operands.push(Operand::Var(override_val.clone()));
            } else {
                // Extract original field value
                let extracted = self.fresh_temp();
                body.push(Instruction::FieldGet {
                    dest: extracted.clone(),
                    obj: Operand::Var(obj_val.to_string()),
                    type_name: type_name.to_string(),
                    field_index: i as u32,
                });
                field_operands.push(Operand::Var(extracted));
            }
        }

        let dest = self.fresh_temp();
        body.push(Instruction::StructInit {
            dest: dest.clone(),
            type_name: type_name.to_string(),
            fields: field_operands,
        });
        dest
    }

    /// Find the last temp-producing instruction in body[start..].
    fn last_temp_in_range(&self, body: &[Instruction], start: usize) -> Option<String> {
        for inst in body[start..].iter().rev() {
            match inst {
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
                | Instruction::FieldGet { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }

    fn last_temp_name(&self, body: &[Instruction]) -> Option<String> {
        for inst in body.iter().rev() {
            match inst {
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
                | Instruction::FieldGet { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }
}

/// Convert AST binary op to MIR op, selecting Int or Float variant.
fn ast_binop_to_mir(op: BinOp, is_float: bool) -> MirBinOp {
    match (op, is_float) {
        (BinOp::Add, false) => MirBinOp::AddInt,
        (BinOp::Add, true) => MirBinOp::AddFloat,
        (BinOp::Sub, false) => MirBinOp::SubInt,
        (BinOp::Sub, true) => MirBinOp::SubFloat,
        (BinOp::Mul, false) => MirBinOp::MulInt,
        (BinOp::Mul, true) => MirBinOp::MulFloat,
        (BinOp::Div, false) => MirBinOp::DivInt,
        (BinOp::Div, true) => MirBinOp::DivFloat,
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
