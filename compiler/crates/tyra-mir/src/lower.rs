// AST to MIR lowering.
//
// Walks the AST and produces a flat sequence of MIR instructions.
// Expressions are flattened into named temporaries.
// Control flow is desugared into labels and branches.

use tyra_ast::*;
use tyra_types::Ty;

use crate::ir::*;
use crate::monomorphize::substitute_fn_def;

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
    /// Tracks variable/temp → struct type name mapping for correct type resolution
    var_types: std::collections::HashMap<String, String>,
    /// Tracks variables/temps known to hold Float values (for correct binop selection)
    float_vars: std::collections::HashSet<String>,
    /// Tracks variables/temps known to hold String values (for interpolation type detection)
    string_vars: std::collections::HashSet<String>,
    /// Tracks mutable local variables (use alloca/store/load instead of SSA copy)
    mut_vars: std::collections::HashSet<String>,
    /// Function return type registry: fn_name → return_type (for type inference in interpolation)
    fn_return_types: std::collections::HashMap<String, Ty>,
    /// Impl method registry: (target_type_name, method_name) → mangled_fn_name
    impl_methods: std::collections::HashMap<(String, String), String>,
    /// Imported module names for module-qualified call resolution (§13)
    imported_modules: std::collections::HashSet<String>,
    /// Current self type when lowering impl method bodies (None outside impl methods)
    self_type: Option<String>,
    /// Tracks variables/temps with generic types (Option<T>, Result<T, E>) for ADT lowering
    generic_var_types: std::collections::HashMap<String, Ty>,
    /// ADT variant field definitions: (type_name, variant_name) → [(field_name, field_type)]
    adt_variant_fields: std::collections::HashMap<(String, String), Vec<(String, Ty)>>,
    /// Return type of the function currently being lowered (for ? operator)
    current_fn_return_type: Ty,
    /// Collected ADT struct defs (monomorphized Option/Result types)
    adt_struct_defs: std::collections::HashMap<String, Vec<(String, Ty)>>,
    /// Deferred expressions for the current function (spec §12.3, LIFO execution)
    deferred_exprs: Vec<Expr>,
    /// Generic function definitions for monomorphization (§8.4).
    /// Maps fn_name → FnDef AST node for turbofish lookup.
    fn_defs: std::collections::HashMap<String, FnDef>,
    /// Monomorphization cache: mangled_name → true.
    /// Prevents duplicate generation of the same specialization.
    mono_cache: std::collections::HashSet<String>,
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
            if ty.is_option() || ty.is_result() {
                self.generic_var_types.insert(name.clone(), ty.clone());
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

            ExprKind::Ident(name) => {
                // Check for None constructor
                if name == "None" {
                    // Infer the Option<T> type from context (function return type or let binding)
                    let full_type = if self.current_fn_return_type.is_option() {
                        self.current_fn_return_type.clone()
                    } else {
                        // Fallback: Option<Int>
                        Ty::Generic("Option".into(), vec![Ty::Int])
                    };
                    self.register_adt_type(&full_type);
                    let type_name = full_type.monomorphized_name();

                    let dest = self.fresh_temp();
                    body.push(Instruction::AdtInit {
                        dest: dest.clone(),
                        type_name: type_name.clone(),
                        tag: 1,
                        fields: vec![],
                    });
                    self.generic_var_types.insert(dest.clone(), full_type);
                    self.var_types.insert(dest.clone(), type_name);
                    return dest;
                }

                if self.mut_vars.contains(name.as_str()) {
                    // Mutable local: load from alloca
                    let temp = self.fresh_temp();
                    body.push(Instruction::Load {
                        dest: temp.clone(),
                        source: name.clone(),
                    });
                    temp
                } else {
                    name.clone()
                }
            }

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
                    && self.adt_variant_fields.contains_key(&(type_name.clone(), variant_name.clone()))
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

                    // Map labeled args to field order (same logic as value constructors)
                    let max_field_count = self
                        .adt_struct_defs
                        .get(type_name)
                        .map(|f| f.len() - 1) // subtract tag field
                        .unwrap_or(vfields.len());

                    let mut field_operands = Vec::with_capacity(max_field_count);
                    let mut used_args: std::collections::HashSet<usize> =
                        std::collections::HashSet::new();

                    for (_fi, (fname, _fty)) in vfields.iter().enumerate() {
                        let labeled = args.iter().enumerate().find(|(idx, a)| {
                            !used_args.contains(idx) && a.label.as_deref() == Some(fname)
                        });
                        let resolved = if let Some((idx, a)) = labeled {
                            used_args.insert(idx);
                            Some(a)
                        } else {
                            let positional =
                                args.iter().enumerate().find(|(idx, _)| !used_args.contains(idx));
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
                            field_operands.push(Operand::Const(Constant::Int(0)));
                        }
                    }

                    // Pad with zeros for fields beyond this variant's count
                    while field_operands.len() < max_field_count {
                        field_operands.push(Operand::Const(Constant::Int(0)));
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

                // Check for .ok_or() on Option<T> (spec §12.2):
                // Converts Option<T> to Result<T, E> where E is the type of the argument.
                if let ExprKind::FieldAccess(obj, method) = &callee.kind
                    && method == "ok_or"
                    && args.len() == 1
                {
                    // Determine if receiver is Option<T>
                    let opt_type = self.infer_expr_type(obj)
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
                        let obj_val = self.lower_expr(obj, body);
                        let err_arg = self.lower_expr(&args[0].value, body);

                        // Infer err type from the argument expression, variable
                        // tracking, or the enclosing function's return type
                        let err_type = self.infer_expr_type(&args[0].value)
                            .or_else(|| {
                                self.var_types.get(&err_arg).map(|n| Ty::Named(n.clone()))
                            })
                            .or_else(|| {
                                self.current_fn_return_type.result_err_type().cloned()
                            })
                            .unwrap_or(Ty::Named("Error".into()));

                        let inner_t = oty.option_inner().cloned().unwrap_or(Ty::Int);
                        let result_type = Ty::Generic(
                            "Result".into(),
                            vec![inner_t, err_type],
                        );
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
                            fields: vec![
                                Operand::Var(payload),
                                Operand::Const(Constant::Int(0)),
                            ],
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
                            fields: vec![
                                Operand::Const(Constant::Int(0)),
                                Operand::Var(err_arg),
                            ],
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
                            // Not an impl method — fall through
                        }
                    }
                }

                // Check for module-qualified call: math.square() → math__square() (§13)
                if let ExprKind::FieldAccess(obj, fn_name) = &callee.kind {
                    if let ExprKind::Ident(module_name) = &obj.kind {
                        if self.imported_modules.contains(module_name.as_str()) {
                            let qualified_name = format!("{module_name}__{fn_name}");
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
                                func: qualified_name,
                                args: arg_operands,
                            });
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
                    func: func_name.clone(),
                    args: arg_operands,
                });

                // Track generic return types from function signatures
                if let Some(ret_ty) = self.fn_return_types.get(&func_name).cloned() {
                    if ret_ty.is_option() || ret_ty.is_result() {
                        self.register_adt_type(&ret_ty);
                        let mono = ret_ty.monomorphized_name();
                        self.generic_var_types.insert(dest.clone(), ret_ty);
                        self.var_types.insert(dest.clone(), mono);
                    }
                }

                dest
            }

            ExprKind::Assign(lhs, rhs) => {
                let val = self.lower_expr(rhs, body);
                match &lhs.kind {
                    ExprKind::Ident(name) => {
                        if self.mut_vars.contains(name.as_str()) {
                            // Mutable local: store to alloca
                            body.push(Instruction::Store {
                                dest: name.clone(),
                                value: Operand::Var(val.clone()),
                            });
                        } else {
                            body.push(Instruction::Copy {
                                dest: name.clone(),
                                source: val.clone(),
                            });
                        }
                    }
                    ExprKind::FieldAccess(obj, field) => {
                        // Field mutation: obj.field = val
                        if let ExprKind::Ident(obj_name) = &obj.kind {
                            if self.mut_vars.contains(obj_name.as_str()) {
                                self.lower_field_assign(
                                    obj_name, obj, field, &val, body,
                                );
                            }
                        }
                    }
                    _ => {}
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
                // ? operator: extract value on success, early-return on failure
                let inner_val = self.lower_expr(inner, body);

                // Determine the ADT type of the inner expression
                let inner_type = self
                    .generic_var_types
                    .get(&inner_val)
                    .cloned()
                    .unwrap_or(self.current_fn_return_type.clone());
                let type_name = inner_type.monomorphized_name();

                // Extract tag
                let tag = self.fresh_temp();
                body.push(Instruction::AdtTag {
                    dest: tag.clone(),
                    obj: Operand::Var(inner_val.clone()),
                    type_name: type_name.clone(),
                });

                // Check if failure (tag != 0 means None/Err)
                let zero = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: zero.clone(),
                    value: Constant::Int(0),
                });
                let is_ok = self.fresh_temp();
                body.push(Instruction::BinOp {
                    dest: is_ok.clone(),
                    op: MirBinOp::EqInt,
                    lhs: Operand::Var(tag),
                    rhs: Operand::Var(zero),
                });

                let ok_label = self.fresh_label("propagate_ok");
                let fail_label = self.fresh_label("propagate_fail");

                body.push(Instruction::BranchIf {
                    cond: Operand::Var(is_ok),
                    true_label: ok_label.clone(),
                    false_label: fail_label.clone(),
                });

                // Failure path: return None/Err from current function
                body.push(Instruction::Label(fail_label));
                if inner_type.is_result() {
                    // For Result: extract err_value and re-wrap as Err.
                    // spec §12.2: If inner error type E != enclosing error type F,
                    // convert via Into<F>: `return Err(e.into())`.
                    let ret_type = &self.current_fn_return_type.clone();
                    self.register_adt_type(ret_type);
                    let ret_type_name = ret_type.monomorphized_name();
                    let err_val = self.fresh_temp();
                    body.push(Instruction::AdtPayload {
                        dest: err_val.clone(),
                        obj: Operand::Var(inner_val.clone()),
                        type_name: type_name.clone(),
                        field_index: 2, // err_value field for Result
                    });

                    // Apply Into<F> conversion if error types differ (spec §12.2)
                    let final_err_val =
                        if let (Some(inner_err), Some(ret_err)) =
                            (inner_type.result_err_type(), ret_type.result_err_type())
                        {
                            if inner_err != ret_err {
                                let inner_err_name = inner_err.monomorphized_name();
                                let into_key =
                                    (inner_err_name.clone(), "into".to_string());
                                if let Some(mangled) = self.impl_methods.get(&into_key).cloned() {
                                    // Call E__into(err_val) to convert error type
                                    let converted = self.fresh_temp();
                                    body.push(Instruction::Call {
                                        dest: Some(converted.clone()),
                                        func: mangled,
                                        args: vec![Operand::Var(err_val.clone())],
                                    });
                                    converted
                                } else {
                                    eprintln!(
                                        "warning: ? operator on Result<_, {}> in function returning Result<_, {}> — no Into<{}> impl found for {}",
                                        inner_err.display_name(),
                                        ret_err.display_name(),
                                        ret_err.display_name(),
                                        inner_err.display_name(),
                                    );
                                    err_val.clone()
                                }
                            } else {
                                // Into<T> for T: identity, no conversion needed
                                err_val.clone()
                            }
                        } else {
                            err_val.clone()
                        };

                    let ret_err = self.fresh_temp();
                    body.push(Instruction::AdtInit {
                        dest: ret_err.clone(),
                        type_name: ret_type_name,
                        tag: 1,
                        fields: vec![
                            Operand::Const(Constant::Int(0)),
                            Operand::Var(final_err_val),
                        ],
                    });
                    // spec §12.3: emit deferred expressions before early return
                    self.emit_deferred(body);
                    body.push(Instruction::Return {
                        value: Some(Operand::Var(ret_err)),
                    });
                } else {
                    // For Option: return None
                    let ret_type = &self.current_fn_return_type.clone();
                    self.register_adt_type(ret_type);
                    let ret_type_name = ret_type.monomorphized_name();
                    let none_val = self.fresh_temp();
                    body.push(Instruction::AdtInit {
                        dest: none_val.clone(),
                        type_name: ret_type_name,
                        tag: 1,
                        fields: vec![],
                    });
                    // spec §12.3: emit deferred expressions before early return
                    self.emit_deferred(body);
                    body.push(Instruction::Return {
                        value: Some(Operand::Var(none_val)),
                    });
                }

                // Success path: extract ok/some payload (field 1)
                body.push(Instruction::Label(ok_label));
                let payload = self.fresh_temp();
                body.push(Instruction::AdtPayload {
                    dest: payload.clone(),
                    obj: Operand::Var(inner_val),
                    type_name,
                    field_index: 1,
                });
                // Track the extracted payload type for downstream type inference
                let payload_type = if inner_type.is_option() {
                    inner_type.option_inner().cloned()
                } else {
                    inner_type.result_ok_type().cloned()
                };
                if let Some(ref pt) = payload_type {
                    match pt {
                        Ty::String => { self.string_vars.insert(payload.clone()); }
                        Ty::Float => { self.float_vars.insert(payload.clone()); }
                        Ty::Named(n) => { self.var_types.insert(payload.clone(), n.clone()); }
                        Ty::Generic(_, _) => {
                            self.generic_var_types.insert(payload.clone(), pt.clone());
                            self.var_types.insert(payload.clone(), pt.monomorphized_name());
                        }
                        _ => {}
                    }
                }
                payload
            }

            ExprKind::Await(inner) => {
                // .await: simplified, just lower the inner expression
                self.lower_expr(inner, body)
            }

            ExprKind::Spawn(inner) => self.lower_expr(inner, body),

            ExprKind::FieldAccess(obj, field) => {
                // Check if this is an ADT constructor: Color.Red or Payment.Cash
                if let ExprKind::Ident(type_name) = &obj.kind
                    && let Some(&tag) = self.variant_tags.get(&(type_name.clone(), field.clone()))
                {
                    // If this ADT has a struct def (has payload variants), emit AdtInit
                    if self.adt_struct_defs.contains_key(type_name) {
                        let max_field_count = self.adt_struct_defs[type_name].len() - 1;
                        let fields = vec![Operand::Const(Constant::Int(0)); max_field_count];
                        let dest = self.fresh_temp();
                        body.push(Instruction::AdtInit {
                            dest: dest.clone(),
                            type_name: type_name.clone(),
                            tag,
                            fields,
                        });
                        self.var_types.insert(dest.clone(), type_name.clone());
                        return dest;
                    }
                    // Pure unit-only ADT: emit tag constant directly
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

            ExprKind::Lambda(_) => {
                let dest = self.fresh_temp();
                body.push(Instruction::Const {
                    dest: dest.clone(),
                    value: Constant::Unit,
                });
                dest
            }

            ExprKind::TurbofishCall(callee, type_args, args) => {
                // spec §8.4: turbofish call — monomorphize generic function
                if let ExprKind::Ident(fn_name) = &callee.kind {
                    let concrete_types: Vec<Ty> = type_args
                        .iter()
                        .map(Ty::from_type_expr)
                        .collect();

                    // Monomorphize and get mangled name
                    let mangled = self.monomorphize(fn_name, &concrete_types);

                    if let Some(func_name) = mangled {
                        // Lower arguments
                        let arg_operands: Vec<Operand> = args
                            .iter()
                            .map(|a| {
                                let t = self.lower_expr(&a.value, body);
                                Operand::Var(t)
                            })
                            .collect();

                        let dest = self.fresh_temp();
                        // Infer return type for type tracking
                        let ret_ty = self.fn_return_types.get(&func_name).cloned();
                        body.push(Instruction::Call {
                            dest: Some(dest.clone()),
                            func: func_name,
                            args: arg_operands,
                        });
                        // Track result type
                        if let Some(ref ty) = ret_ty {
                            match ty {
                                Ty::String => { self.string_vars.insert(dest.clone()); }
                                Ty::Float => { self.float_vars.insert(dest.clone()); }
                                Ty::Named(n) => { self.var_types.insert(dest.clone(), n.clone()); }
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
                // Fallback: unresolved turbofish — no generic function found
                eprintln!("warning: unresolved turbofish call (no matching generic function)");
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
                // Build a printf-style format string and collect args.
                // Type detection determines the format specifier per expression.
                let mut format_str = String::new();
                let mut format_args: Vec<Operand> = Vec::new();

                for part in parts {
                    match part {
                        StringPart::Lit(s) => {
                            // Escape '%' for printf format strings
                            format_str.push_str(&s.replace('%', "%%"));
                        }
                        StringPart::Expr(e) => {
                            let is_float = self.is_float_expr(e);
                            let is_string = self.is_string_expr(e);
                            let val = self.lower_expr(e, body);

                            if is_string {
                                format_str.push_str("%s");
                            } else if is_float {
                                format_str.push_str("%g");
                            } else {
                                format_str.push_str("%ld");
                            }
                            format_args.push(Operand::Var(val));
                        }
                    }
                }

                let format_ref = self.intern_string(&format_str);
                let dest = self.fresh_temp();
                body.push(Instruction::StringFormat {
                    dest: dest.clone(),
                    format_ref,
                    args: format_args,
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

        // Pre-allocate pattern-bound variables to avoid SSA dominance issues.
        // When multiple arms bind the same name (e.g., Dog(name) + Cat(name)),
        // the alloca must dominate all uses across all arms.
        for arm in &m.arms {
            if let PatternKind::Constructor(_, fields) = &arm.pattern.kind {
                for pf in fields {
                    if !self.mut_vars.contains(&pf.field_name) {
                        body.push(Instruction::Alloca {
                            dest: pf.field_name.clone(),
                        });
                        self.mut_vars.insert(pf.field_name.clone());
                    }
                }
            }
        }

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
                    // Check if this is an Option/Result variant (Some/None/Ok/Err)
                    let prelude_tag = match variant_name.as_str() {
                        "Some" | "Ok" => Some(0i64),
                        "None" | "Err" => Some(1i64),
                        _ => None,
                    };

                    if let Some(tag) = prelude_tag {
                        // Option/Result ADT: extract tag from tagged struct
                        let subject_type_name = self
                            .generic_var_types
                            .get(&subject)
                            .map(|t| t.monomorphized_name())
                            .or_else(|| self.var_types.get(&subject).cloned())
                            .unwrap_or_else(|| {
                                // BUG: subject type not tracked. This indicates a gap in
                                // generic_var_types / var_types tracking. Fall back to
                                // the function return type if it's an Option/Result.
                                if self.current_fn_return_type.is_option()
                                    || self.current_fn_return_type.is_result()
                                {
                                    self.current_fn_return_type.monomorphized_name()
                                } else {
                                    panic!(
                                        "BUG: cannot determine ADT type for match subject '{subject}'"
                                    )
                                }
                            });

                        let tag_val = self.fresh_temp();
                        body.push(Instruction::AdtTag {
                            dest: tag_val.clone(),
                            obj: Operand::Var(subject.clone()),
                            type_name: subject_type_name,
                        });
                        let lit = self.fresh_temp();
                        body.push(Instruction::Const {
                            dest: lit.clone(),
                            value: Constant::Int(tag),
                        });
                        let cond = self.fresh_temp();
                        body.push(Instruction::BinOp {
                            dest: cond.clone(),
                            op: MirBinOp::EqInt,
                            lhs: Operand::Var(tag_val),
                            rhs: Operand::Var(lit),
                        });
                        body.push(Instruction::BranchIf {
                            cond: Operand::Var(cond),
                            true_label: arm_label.clone(),
                            false_label: next_label.clone(),
                        });
                    } else {
                        // User-defined ADT: look up tag from variant_tags.
                        // Use subject type name for disambiguation when available.
                        let subject_type_name = self.var_types.get(&subject).cloned();
                        let tag = if let Some(ref stn) = subject_type_name {
                            self.variant_tags
                                .get(&(stn.clone(), variant_name.clone()))
                                .copied()
                        } else {
                            // Fallback: search by variant name only (ambiguous)
                            self.variant_tags
                                .iter()
                                .find(|((_, vn), _)| vn == variant_name)
                                .map(|(_, &t)| t)
                        };

                        if let Some(tag) = tag {
                            let has_struct = subject_type_name
                                .as_ref()
                                .map(|n| self.adt_struct_defs.contains_key(n))
                                .unwrap_or(false);

                            if has_struct {
                                // Struct-based ADT: extract tag via AdtTag
                                let stn = subject_type_name.unwrap();
                                let tag_val = self.fresh_temp();
                                body.push(Instruction::AdtTag {
                                    dest: tag_val.clone(),
                                    obj: Operand::Var(subject.clone()),
                                    type_name: stn,
                                });
                                let lit = self.fresh_temp();
                                body.push(Instruction::Const {
                                    dest: lit.clone(),
                                    value: Constant::Int(tag),
                                });
                                let cond = self.fresh_temp();
                                body.push(Instruction::BinOp {
                                    dest: cond.clone(),
                                    op: MirBinOp::EqInt,
                                    lhs: Operand::Var(tag_val),
                                    rhs: Operand::Var(lit),
                                });
                                body.push(Instruction::BranchIf {
                                    cond: Operand::Var(cond),
                                    true_label: arm_label.clone(),
                                    false_label: next_label.clone(),
                                });
                            } else {
                                // Unit-only ADT: subject is plain integer tag
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
                            }
                        } else {
                            // Unknown constructor — fall through (treat as wildcard)
                            body.push(Instruction::Jump {
                                label: arm_label.clone(),
                            });
                        }
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

            // Track arm body start BEFORE pattern bindings so that
            // pattern-bound variables (Copy instructions) are included in
            // last_temp_in_range when the arm body just references the bound variable.
            let arm_body_start = body.len();

            // Bind constructor payload variables: when Some(x) → x = payload
            if let PatternKind::Constructor(variant_name, fields) = &arm.pattern.kind {
                let is_prelude = matches!(variant_name.as_str(), "Some" | "Ok" | "Err");
                if is_prelude && !fields.is_empty()
                    && fields[0].field_name != "_"
                {
                    let subject_type_name = self
                        .generic_var_types
                        .get(&subject)
                        .map(|t| t.monomorphized_name())
                        .or_else(|| self.var_types.get(&subject).cloned())
                        .unwrap_or_else(|| {
                            if self.current_fn_return_type.is_option()
                                || self.current_fn_return_type.is_result()
                            {
                                self.current_fn_return_type.monomorphized_name()
                            } else {
                                panic!(
                                    "BUG: cannot determine ADT type for match subject '{subject}'"
                                )
                            }
                        });

                    // Extract payload from ADT and bind to the first field variable
                    // For Option: Some=field 1. For Result: Ok=field 1, Err=field 2.
                    let field_index = if variant_name == "Err" { 2 } else { 1 };
                    let payload = self.fresh_temp();
                    body.push(Instruction::AdtPayload {
                        dest: payload.clone(),
                        obj: Operand::Var(subject.clone()),
                        type_name: subject_type_name.clone(),
                        field_index,
                    });

                    // Store into the pre-allocated alloca for this variable
                    let bind_name = &fields[0].field_name;
                    body.push(Instruction::Store {
                        dest: bind_name.clone(),
                        value: Operand::Var(payload),
                    });

                    // Track the type of the bound variable
                    if let Some(subject_ty) = self.generic_var_types.get(&subject) {
                        if let Some(inner) = subject_ty.option_inner() {
                            match inner {
                                Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                _ => {}
                            }
                        } else if variant_name == "Ok" {
                            if let Some(ok_ty) = subject_ty.result_ok_type() {
                                match ok_ty {
                                    Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                    Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                    _ => {}
                                }
                            }
                        } else if variant_name == "Err" {
                            if let Some(err_ty) = subject_ty.result_err_type() {
                                match err_ty {
                                    Ty::String => { self.string_vars.insert(bind_name.clone()); }
                                    Ty::Float => { self.float_vars.insert(bind_name.clone()); }
                                    _ => {}
                                }
                            }
                        }
                    }
                } else if !fields.is_empty() {
                    // User-defined ADT: extract payload fields by position
                    let subject_type_name = self.var_types.get(&subject).cloned();
                    if let Some(stn) = subject_type_name {
                        // Look up variant field definitions using subject type name
                        let vfields: Option<Vec<(String, Ty)>> = self
                            .adt_variant_fields
                            .get(&(stn.clone(), variant_name.clone()))
                            .cloned();

                        if let Some(vfields) = vfields {
                            for (fi, pf) in fields.iter().enumerate() {
                                // Skip wildcard bindings
                                if pf.field_name == "_" {
                                    continue;
                                }
                                let field_index = (fi + 1) as u32; // +1 for tag at field 0
                                let payload = self.fresh_temp();
                                body.push(Instruction::AdtPayload {
                                    dest: payload.clone(),
                                    obj: Operand::Var(subject.clone()),
                                    type_name: stn.clone(),
                                    field_index,
                                });
                                // Store into the pre-allocated alloca for this variable
                                body.push(Instruction::Store {
                                    dest: pf.field_name.clone(),
                                    value: Operand::Var(payload.clone()),
                                });
                                // Track field type
                                if let Some((_, fty)) = vfields.get(fi) {
                                    match fty {
                                        Ty::String => {
                                            self.string_vars.insert(pf.field_name.clone());
                                        }
                                        Ty::Float => {
                                            self.float_vars.insert(pf.field_name.clone());
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

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
            ExprKind::Call(callee, _) => self.call_returns_type(callee, &Ty::Float),
            _ => false,
        }
    }

    /// Check if an expression produces a String value.
    /// Used to select format specifiers in string interpolation.
    fn is_string_expr(&self, expr: &Expr) -> bool {
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
            _ => false,
        }
    }

    /// Check if a function/method call returns a specific type.
    fn call_returns_type(&self, callee: &Expr, expected: &Ty) -> bool {
        match &callee.kind {
            ExprKind::Ident(name) => self
                .fn_return_types
                .get(name.as_str())
                .map_or(false, |ty| ty == expected),
            ExprKind::FieldAccess(obj, method) => {
                // Check impl method return type
                if let ImplMethodResult::Resolved(mangled) =
                    self.resolve_impl_method(obj, method)
                {
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
    fn expr_struct_type(&self, expr: &Expr) -> Option<String> {
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
                        if let Some(Ty::Named(type_name)) = self.fn_return_types.get(&mangled)
                        {
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

    /// Resolve the struct type (value or data) of an expression.
    /// Returns (type_name, field_defs) if the expression is a known struct-typed binding.
    fn resolve_struct_type(&self, expr: &Expr) -> Option<(String, Vec<(String, Ty)>)> {
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

    /// Lower a field assignment: `obj.field = val`.
    /// Loads the struct, replaces the target field, stores back.
    fn lower_field_assign(
        &mut self,
        obj_name: &str,
        obj_expr: &Expr,
        field: &str,
        val: &str,
        body: &mut Vec<Instruction>,
    ) {
        if let Some((type_name, field_defs)) = self.resolve_struct_type(obj_expr) {
            // Field mutation is only allowed on data types (§8.6).
            // Value types are immutable — use copy() instead.
            if !self.data_types.contains(&type_name) {
                return;
            }
            if let Some(field_idx) = field_defs.iter().position(|(n, _)| n == field) {
                // Load current struct value
                let current = self.fresh_temp();
                body.push(Instruction::Load {
                    dest: current.clone(),
                    source: obj_name.to_string(),
                });

                // Build new struct: extract all fields, replace the target
                let mut field_operands = Vec::with_capacity(field_defs.len());
                for (i, _) in field_defs.iter().enumerate() {
                    if i == field_idx {
                        field_operands.push(Operand::Var(val.to_string()));
                    } else {
                        let extracted = self.fresh_temp();
                        body.push(Instruction::FieldGet {
                            dest: extracted.clone(),
                            obj: Operand::Var(current.clone()),
                            type_name: type_name.clone(),
                            field_index: i as u32,
                        });
                        field_operands.push(Operand::Var(extracted));
                    }
                }

                let new_struct = self.fresh_temp();
                body.push(Instruction::StructInit {
                    dest: new_struct.clone(),
                    type_name: type_name.clone(),
                    fields: field_operands,
                });

                // Store back to the mutable variable
                body.push(Instruction::Store {
                    dest: obj_name.to_string(),
                    value: Operand::Var(new_struct),
                });
            }
        }
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

    /// Register an ADT struct def for a generic type (Option<T>, Result<T, E>).
    /// Creates a monomorphized StructDef if not already registered.
    fn register_adt_type(&mut self, ty: &Ty) {
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
        }
    }

    /// Infer the full generic type of a call expression like Some(x), Ok(x), Err(e).
    /// Returns None if not a prelude constructor.
    fn infer_adt_call_type(&self, func_name: &str, arg_type: &Ty) -> Option<Ty> {
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
                    Some(Ty::Generic(
                        "Result".into(),
                        vec![Ty::Named("Value".into()), arg_type.clone()],
                    ))
                }
            }
            _ => None,
        }
    }

    /// Infer the type of an expression for ADT type tracking.
    fn infer_expr_type(&self, expr: &Expr) -> Option<Ty> {
        match &expr.kind {
            ExprKind::IntLit(_) => Some(Ty::Int),
            ExprKind::FloatLit(_) => Some(Ty::Float),
            ExprKind::StringLit(_) | ExprKind::StringInterp(_) => Some(Ty::String),
            ExprKind::BoolLit(_) => Some(Ty::Bool),
            ExprKind::Ident(name) => {
                if self.float_vars.contains(name) {
                    Some(Ty::Float)
                } else if self.string_vars.contains(name) {
                    Some(Ty::String)
                } else if self.generic_var_types.contains_key(name) {
                    self.generic_var_types.get(name).cloned()
                } else {
                    // Cannot determine type from tracking alone; caller should
                    // handle None (e.g., by falling back to function return type).
                    None
                }
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
                        if let Some(adt_ty) = self.infer_expr_type(&first_arg.value)
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
                    // ADT payload constructor: TypeName.Variant(args)
                    if let ExprKind::Ident(type_name) = &obj.kind {
                        if self.variant_tags.contains_key(&(type_name.clone(), variant.clone())) {
                            return Some(Ty::Named(type_name.clone()));
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
                    if self.variant_tags.contains_key(&(type_name.clone(), field.clone())) {
                        return Some(Ty::Named(type_name.clone()));
                    }
                }
                None
            }
            _ => None,
        }
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
                | Instruction::StringFormat { dest, .. } => return Some(dest.clone()),
                _ => continue,
            }
        }
        None
    }

    /// Monomorphize a generic function with concrete type arguments (§8.4).
    /// Returns the mangled function name. If not yet monomorphized, creates and
    /// lowers a specialized copy of the function with type parameters substituted.
    fn monomorphize(
        &mut self,
        fn_name: &str,
        type_args: &[Ty],
    ) -> Option<String> {
        // Generate mangled name: fn_name__Type1__Type2
        let type_suffix: Vec<String> = type_args
            .iter()
            .map(|t| t.monomorphized_name())
            .collect();
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
                | Instruction::StringFormat { dest, .. } => return Some(dest.clone()),
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
