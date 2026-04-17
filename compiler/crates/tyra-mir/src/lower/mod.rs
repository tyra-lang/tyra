// AST to MIR lowering.
//
// Walks the AST and produces a flat sequence of MIR instructions.
// Expressions are flattened into named temporaries.
// Control flow is desugared into labels and branches.

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
                    // Track max field count across all variants for struct layout
                    let mut max_fields: Vec<(String, Ty)> = Vec::new();

                    for (i, variant) in variants.iter().enumerate() {
                        ctx.variant_tags
                            .insert((t.name.clone(), variant.name.clone()), i as i64);

                        // Collect variant field definitions for payload lowering
                        let vfields: Vec<(String, Ty)> = variant
                            .fields
                            .iter()
                            .map(|f| (f.name.clone(), Ty::from_type_expr(&f.type_annotation)))
                            .collect();
                        ctx.adt_variant_fields.insert(
                            (t.name.clone(), variant.name.clone()),
                            vfields.clone(),
                        );

                        // Extend max_fields to cover all variant fields.
                        // When variants have different types at the same position,
                        // use the "widest" type to avoid LLVM type mismatches.
                        for (j, (fname, fty)) in vfields.iter().enumerate() {
                            if j >= max_fields.len() {
                                max_fields.push((fname.clone(), fty.clone()));
                            } else if max_fields[j].1 != *fty {
                                // Type conflict at this position: use the wider type.
                                // On 64-bit: ptr and i64 are both 8 bytes; String (ptr)
                                // is the safest common representation.
                                max_fields[j] = (fname.clone(), wider_type(&max_fields[j].1, fty));
                            }
                        }
                    }

                    // Register struct def for ADTs with payload fields
                    if !max_fields.is_empty() {
                        let mut struct_fields = vec![("tag".into(), Ty::Int)];
                        struct_fields.extend(max_fields);
                        ctx.adt_struct_defs
                            .insert(t.name.clone(), struct_fields);
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

            // Register built-in module types
            let module_key: String = imp.path.join(".");
            if module_key == "core.sys" {
                // sys.args() -> List<String>
                let list_string = Ty::Generic("List".into(), vec![Ty::String]);
                ctx.register_adt_type(&list_string);
                ctx.fn_return_types
                    .insert("sys__args".into(), list_string);
            }
        }
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
        ctx.deferred_exprs.clear();
        let mut body = Vec::new();
        for item in &file.items {
            if let Item::Stmt(s) = item {
                ctx.lower_stmt(s, &mut body);
            }
        }
        // spec §12.3: emit deferred expressions before implicit main return
        ctx.emit_deferred(&mut body);
        body.push(Instruction::Return { value: None });

        ctx.functions.push(Function {
            name: "main".into(),
            params: vec![],
            return_type: Ty::Unit,
            body,
            is_main: true,
        });
    }

    let mut struct_defs: Vec<crate::ir::StructDef> = ctx
        .struct_fields
        .iter()
        .map(|(name, fields)| crate::ir::StructDef {
            name: name.clone(),
            fields: fields.clone(),
        })
        .collect();

    // Add ADT struct defs (monomorphized Option/Result types)
    for (name, fields) in &ctx.adt_struct_defs {
        struct_defs.push(crate::ir::StructDef {
            name: name.clone(),
            fields: fields.clone(),
        });
    }

    Program {
        functions: ctx.functions,
        string_constants: ctx.string_constants,
        struct_defs,
    }
}

pub(crate) struct LowerCtx {
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
    /// Function return type registry: fn_name → return_type (for type inference in interpolation)
    pub(crate) fn_return_types: std::collections::HashMap<String, Ty>,
    /// Impl method registry: (target_type_name, method_name) → mangled_fn_name
    pub(crate) impl_methods: std::collections::HashMap<(String, String), String>,
    /// Imported module names for module-qualified call resolution (§13)
    pub(crate) imported_modules: std::collections::HashSet<String>,
    /// Current self type when lowering impl method bodies (None outside impl methods)
    pub(crate) self_type: Option<String>,
    /// Tracks variables/temps with generic types (Option<T>, Result<T, E>) for ADT lowering
    pub(crate) generic_var_types: std::collections::HashMap<String, Ty>,
    /// ADT variant field definitions: (type_name, variant_name) → [(field_name, field_type)]
    pub(crate) adt_variant_fields: std::collections::HashMap<(String, String), Vec<(String, Ty)>>,
    /// Return type of the function currently being lowered (for ? operator)
    pub(crate) current_fn_return_type: Ty,
    /// Collected ADT struct defs (monomorphized Option/Result types)
    pub(crate) adt_struct_defs: std::collections::HashMap<String, Vec<(String, Ty)>>,
    /// Deferred expressions for the current function (spec §12.3, LIFO execution)
    pub(crate) deferred_exprs: Vec<Expr>,
    /// Generic function definitions for monomorphization (§8.4).
    pub(crate) fn_defs: std::collections::HashMap<String, FnDef>,
    /// Monomorphization cache: mangled_name → true.
    pub(crate) mono_cache: std::collections::HashSet<String>,
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
            var_types: std::collections::HashMap::new(),
            float_vars: std::collections::HashSet::new(),
            string_vars: std::collections::HashSet::new(),
            mut_vars: std::collections::HashSet::new(),
            fn_return_types: std::collections::HashMap::new(),
            imported_modules: std::collections::HashSet::new(),
            impl_methods: std::collections::HashMap::new(),
            self_type: None,
            generic_var_types: std::collections::HashMap::new(),
            adt_variant_fields: std::collections::HashMap::new(),
            current_fn_return_type: Ty::Unit,
            adt_struct_defs: std::collections::HashMap::new(),
            deferred_exprs: Vec::new(),
            fn_defs: std::collections::HashMap::new(),
            mono_cache: std::collections::HashSet::new(),
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
        self.var_types.clear();
        self.float_vars.clear();
        self.string_vars.clear();
        self.mut_vars.clear();
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

        // Register parameter types for correct type resolution
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

        let mut body = Vec::new();
        let mut last_expr_result = None;
        for stmt in &f.body {
            // Track the result of expression statements for implicit return
            if let Stmt::Expr(s) = stmt {
                last_expr_result = Some(self.lower_expr(&s.expr, &mut body));
            } else {
                last_expr_result = None;
                self.lower_stmt(stmt, &mut body);
            }
        }

        // If last instruction isn't a return, add implicit return
        if !matches!(body.last(), Some(Instruction::Return { .. })) {
            // spec §12.3: emit deferred expressions before implicit return
            self.emit_deferred(&mut body);
            if return_type == Ty::Unit {
                body.push(Instruction::Return { value: None });
            } else if let Some(last_temp) = self.last_temp_name(&body) {
                body.push(Instruction::Return {
                    value: Some(Operand::Var(last_temp)),
                });
            } else if let Some(expr_val) = last_expr_result {
                // Last expression was a simple variable reference (no instruction generated)
                body.push(Instruction::Return {
                    value: Some(Operand::Var(expr_val)),
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
                let is_string = self.is_string_expr(&s.value);
                let struct_type = self.expr_struct_type(&s.value);
                let val = self.lower_expr(&s.value, body);
                // Track types from AST analysis
                if is_float || self.float_vars.contains(&val) {
                    self.float_vars.insert(s.name.clone());
                }
                if is_string || self.string_vars.contains(&val) {
                    self.string_vars.insert(s.name.clone());
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
                    self.var_types.insert(s.name.clone(), gt.monomorphized_name());
                }
                body.push(Instruction::Copy {
                    dest: s.name.clone(),
                    source: val,
                });
            }
            Stmt::Mut(s) => {
                let is_float = self.is_float_expr(&s.value);
                let is_string = self.is_string_expr(&s.value);
                let struct_type = self.expr_struct_type(&s.value);
                let val = self.lower_expr(&s.value, body);
                if is_float {
                    self.float_vars.insert(s.name.clone());
                }
                if is_string {
                    self.string_vars.insert(s.name.clone());
                }
                if let Some(stype) = struct_type {
                    self.var_types.insert(s.name.clone(), stype);
                }
                // Mutable locals use alloca+store for SSA-compatible mutation
                body.push(Instruction::Alloca {
                    dest: s.name.clone(),
                });
                body.push(Instruction::Store {
                    dest: s.name.clone(),
                    value: Operand::Var(val),
                });
                self.mut_vars.insert(s.name.clone());
            }
            Stmt::Return(s) => {
                let value = s.value.as_ref().map(|v| {
                    let t = self.lower_expr(v, body);
                    Operand::Var(t)
                });
                // spec §12.3: emit deferred expressions before return
                self.emit_deferred(body);
                body.push(Instruction::Return { value });
            }
            Stmt::Defer(d) => {
                // spec §12.3: collect deferred expressions; they are emitted
                // in LIFO order before every return path.
                self.deferred_exprs.push(d.expr.clone());
            }
            Stmt::Expr(s) => {
                self.lower_expr(&s.expr, body);
            }
        }
    }

    fn lower_if(&mut self, if_expr: &IfExpr, body: &mut Vec<Instruction>) -> String {
        let cond = self.lower_expr(&if_expr.condition, body);
        let then_label = self.fresh_label("then");
        let else_label = self.fresh_label("else");
        let end_label = self.fresh_label("if_end");

        // Allocate result slot (like match)
        let result_slot = self.fresh_temp();
        body.push(Instruction::Alloca {
            dest: result_slot.clone(),
        });

        body.push(Instruction::BranchIf {
            cond: Operand::Var(cond),
            true_label: then_label.clone(),
            false_label: else_label.clone(),
        });

        // Then branch
        body.push(Instruction::Label(then_label));
        let then_start = body.len();
        for stmt in &if_expr.then_body {
            self.lower_stmt(stmt, body);
        }
        if let Some(last) = self.last_temp_in_range(body, then_start) {
            body.push(Instruction::Store {
                dest: result_slot.clone(),
                value: Operand::Var(last),
            });
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        // Else branch
        body.push(Instruction::Label(else_label));
        let else_start = body.len();
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
        if let Some(last) = self.last_temp_in_range(body, else_start) {
            body.push(Instruction::Store {
                dest: result_slot.clone(),
                value: Operand::Var(last),
            });
        }
        body.push(Instruction::Jump {
            label: end_label.clone(),
        });

        body.push(Instruction::Label(end_label));

        let result = self.fresh_temp();
        body.push(Instruction::Load {
            dest: result.clone(),
            source: result_slot,
        });
        result
    }

    /// Emit all deferred expressions in LIFO order (spec §12.3).
    /// Called before every return path (explicit return, ? early return, implicit return).
    /// Note: this deliberately does NOT clear deferred_exprs — every return path
    /// (including multiple ? early returns within a single function) must emit the
    /// full set of deferred expressions. The list is cleared at lower_fn entry.
    fn emit_deferred(&mut self, body: &mut Vec<Instruction>) {
        // Clone to avoid borrow conflict (deferred_exprs is on self)
        let exprs: Vec<Expr> = self.deferred_exprs.iter().rev().cloned().collect();
        for expr in &exprs {
            self.lower_expr(expr, body);
        }
    }

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
/// field position. On 64-bit platforms, String (ptr) and Int (i64) are both 8 bytes.
/// When types differ, prefer String (ptr) as the safe common representation.
fn wider_type(a: &Ty, b: &Ty) -> Ty {
    // Same type: no conflict
    if a == b {
        return a.clone();
    }
    // String (ptr) is the safest fallback for mixed types
    if matches!(a, Ty::String) || matches!(b, Ty::String) {
        return Ty::String;
    }
    // Float (double) vs Int: use Float (8 bytes, superset representation)
    if matches!(a, Ty::Float) || matches!(b, Ty::Float) {
        return Ty::Float;
    }
    // Default: keep the first type (both i64-compatible)
    a.clone()
}
