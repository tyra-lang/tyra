// Type checker: walks the AST and verifies type correctness.
//
// Current scope (Milestone 1a):
// - Literal type inference (Int, Float, String, Bool, Unit)
// - Arithmetic, comparison, and logical operator type checking
// - Function call argument type checking (count only for now; full type
//   checking of prelude signatures requires stdlib type info)
// - let/mut binding type annotation verification
// - Assignment mutability checking
//
// Deferred to later milestones:
// - Generics and type parameter inference
// - Ability auto-derivation (Eq, Hash, Ord, Debug)
// - Trait resolution
// - ? operator type verification (Result/Option return type checking)
// - Into trait handling
//
// spec reference: §8 (type system), §10.1 (operators), §12.2 (?)

use std::collections::{HashMap, HashSet};

use tyra_ast::*;
use tyra_diagnostics::{Diagnostic, Label, Report, Span};

use crate::ty::{Ability, Ty, types_compatible};

/// Map from source span → inferred type, used by the LSP hover handler.
pub type TypeIndex = HashMap<Span, Ty>;

/// Type environment: holds all state used during type checking.
///
/// Three distinct concerns live here for convenience — all are read from the
/// same `infer_expr` call site, so a single struct avoids plumbing three
/// references through every helper. Future split candidate when one of these
/// grows (tracked as a suggestion in the review, non-blocking):
///
/// - **Lexical bindings (`bindings`)**: scoped name → type map, push/pop with
///   block scopes. The traditional "env" part.
/// - **Module registries (`adt_variants`, `trait_methods`, `trait_impls`,
///   `type_abilities`, `user_defined_types`)**: built once during
///   `collect_top_level_types`, then read-only. Act as a TypeCtx.
/// - **Call-site context (`return_type_stack`)**: push/pop around fn bodies
///   for `return` / `?` checks.
#[derive(Debug)]
pub struct TypeEnv {
    bindings: Vec<HashMap<String, Ty>>,
    /// Per-scope set of `let`-bound (immutable) variable names. Parallel to
    /// `bindings`. Assignment to these names is rejected with E0206.
    let_bindings: Vec<HashSet<String>>,
    /// ADT variant names keyed by type name (§10.3 exhaustiveness).
    /// - User-defined: `type Color = | Red | Green | Blue` → "Color" → ["Red", "Green", "Blue"]
    /// - Prelude: "Option" → ["Some", "None"], "Result" → ["Ok", "Err"]
    adt_variants: HashMap<String, Vec<String>>,
    /// Stack of enclosing function return types (for `return` stmt and `?` operator checks).
    /// Top of stack = innermost enclosing function. Empty when not inside any fn body.
    return_type_stack: Vec<Ty>,
    /// Nesting depth of while/for loops; non-zero means `break` is valid.
    loop_depth: u32,
    /// Trait name → required method names (§8.7).
    /// Populated by register_trait from TraitDef definitions.
    trait_methods: HashMap<String, Vec<String>>,
    /// (trait_name, type_name) → Vec<method_name> for impls.
    /// Used to verify trait method bodies and to check that a type has an impl
    /// for a given trait (e.g. Stringable).
    trait_impls: HashMap<(String, String), Vec<String>>,
    /// Registered `impl Into<To> for From` pairs (§12.2). Used by `?` on
    /// Result to verify the error-type conversion is available.
    /// `Into<T> for T` is auto-provided (identity) and not stored here.
    into_impls: HashSet<(String, String)>,
    /// Named type → abilities granted to that type (§8 auto-derivation rules).
    /// Primitives are seeded in register_prelude; user types are computed from
    /// their fields/variants in collect_top_level_types.
    type_abilities: HashMap<String, HashSet<Ability>>,
    /// User-defined type names (value/data/ADT). Used to distinguish user types
    /// from primitives for checks like Stringable impl requirement (E0501).
    user_defined_types: HashSet<String>,
    /// Span → Ty map populated during type checking for LSP hover support.
    pub(crate) type_index: TypeIndex,
    /// Stack of outer-scope `mut` binding name sets — one entry per lambda
    /// nesting level. Non-empty when inside a lambda body. Used to detect
    /// E0402 (assignment to an outer `mut` binding from inside a closure).
    lambda_outer_muts: Vec<HashSet<String>>,
    /// Scope depth (`bindings.len()`) at each lambda entry point.
    /// Paired 1-to-1 with lambda_outer_muts; used to detect inner shadows
    /// (ADR-0011 §3: binding-identity-based check, not name-string-based).
    lambda_entry_depths: Vec<usize>,
}

impl TypeEnv {
    pub fn new() -> Self {
        Self {
            bindings: vec![HashMap::new()],
            let_bindings: vec![HashSet::new()],
            adt_variants: HashMap::new(),
            return_type_stack: Vec::new(),
            loop_depth: 0,
            trait_methods: HashMap::new(),
            trait_impls: HashMap::new(),
            into_impls: HashSet::new(),
            type_abilities: HashMap::new(),
            user_defined_types: HashSet::new(),
            type_index: HashMap::new(),
            lambda_outer_muts: Vec::new(),
            lambda_entry_depths: Vec::new(),
        }
    }

    /// Record the inferred type for a span (for LSP hover).
    /// Uses entry API to prefer the first (outermost) type recorded for a span.
    pub fn record_type(&mut self, span: Span, ty: Ty) {
        self.type_index.entry(span).or_insert(ty);
    }

    pub fn push_return_type(&mut self, ty: Ty) {
        self.return_type_stack.push(ty);
    }

    pub fn pop_return_type(&mut self) {
        self.return_type_stack.pop();
    }

    pub fn current_return_type(&self) -> Option<&Ty> {
        self.return_type_stack.last()
    }

    pub fn enter_loop(&mut self) {
        self.loop_depth += 1;
    }

    pub fn exit_loop(&mut self) {
        self.loop_depth = self.loop_depth.saturating_sub(1);
    }

    pub fn in_loop(&self) -> bool {
        self.loop_depth > 0
    }

    pub fn push(&mut self) {
        self.bindings.push(HashMap::new());
        self.let_bindings.push(HashSet::new());
    }

    pub fn pop(&mut self) {
        self.bindings.pop();
        self.let_bindings.pop();
    }

    pub fn define(&mut self, name: String, ty: Ty) {
        self.bindings.last_mut().unwrap().insert(name, ty);
    }

    pub fn define_let(&mut self, name: String, ty: Ty) {
        self.bindings.last_mut().unwrap().insert(name.clone(), ty);
        self.let_bindings.last_mut().unwrap().insert(name);
    }

    /// Returns true if `name` resolves to a `let` (immutable) binding
    /// in the innermost scope where it is defined.
    pub fn is_let_bound(&self, name: &str) -> bool {
        for (bindings, lets) in self.bindings.iter().zip(self.let_bindings.iter()).rev() {
            if bindings.contains_key(name) {
                return lets.contains(name);
            }
        }
        false
    }

    pub fn lookup(&self, name: &str) -> Option<&Ty> {
        for scope in self.bindings.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    /// Enter a lambda body: snapshot all current `mut`-bound names from every
    /// enclosing scope and push them onto the lambda_outer_muts stack (E0402).
    pub fn enter_lambda_scope(&mut self) {
        // Record the scope depth so is_lambda_outer_mut can distinguish outer
        // bindings from inner shadows (ADR-0011 §3: binding-identity semantics).
        self.lambda_entry_depths.push(self.bindings.len());
        let mut outer_muts: HashSet<String> = HashSet::new();
        for (bindings, lets) in self.bindings.iter().zip(self.let_bindings.iter()) {
            for name in bindings.keys() {
                if !lets.contains(name.as_str()) {
                    outer_muts.insert(name.clone());
                }
            }
        }
        self.lambda_outer_muts.push(outer_muts);
    }

    /// Exit a lambda body: pop the outer-mut snapshot (E0402).
    pub fn exit_lambda_scope(&mut self) {
        self.lambda_outer_muts.pop();
        self.lambda_entry_depths.pop();
    }

    /// Returns true when inside a lambda and `name` is a `mut` binding from
    /// an **enclosing** scope — assignment would violate spec §9.4 (E0402).
    ///
    /// Uses the scope-depth recorded at lambda entry to handle shadowing:
    /// if the lambda body introduces its own `mut x`, scopes at depth
    /// `>= entry_depth` will contain `x` — it is a local binding, not
    /// an outer capture, so this returns false (no E0402).
    pub fn is_lambda_outer_mut(&self, name: &str) -> bool {
        let (Some(outer_muts), Some(&entry_depth)) = (
            self.lambda_outer_muts.last(),
            self.lambda_entry_depths.last(),
        ) else {
            return false;
        };
        if !outer_muts.contains(name) {
            return false;
        }
        // If `name` is defined in any scope introduced inside the lambda,
        // it is a local binding (inner shadow) — not an outer capture.
        !self.bindings[entry_depth..]
            .iter()
            .any(|s| s.contains_key(name))
    }

    /// Register an ADT with its variant names for exhaustiveness checking.
    pub fn register_adt(&mut self, type_name: String, variants: Vec<String>) {
        self.adt_variants.insert(type_name, variants);
    }

    /// Get variant names for an ADT type, if registered.
    pub fn adt_variants(&self, type_name: &str) -> Option<&Vec<String>> {
        self.adt_variants.get(type_name)
    }

    /// Register a trait and its required method names (§8.7).
    pub fn register_trait(&mut self, trait_name: String, methods: Vec<String>) {
        self.trait_methods.insert(trait_name, methods);
    }

    /// Get required method names for a trait, if registered.
    pub fn trait_methods(&self, trait_name: &str) -> Option<&Vec<String>> {
        self.trait_methods.get(trait_name)
    }

    /// Register an impl (trait, type) → methods implemented.
    pub fn register_impl(&mut self, trait_name: String, type_name: String, methods: Vec<String>) {
        self.trait_impls.insert((trait_name, type_name), methods);
    }

    /// Check if a type implements a given trait.
    pub fn has_trait_impl(&self, trait_name: &str, type_name: &str) -> bool {
        self.trait_impls
            .contains_key(&(trait_name.to_string(), type_name.to_string()))
    }

    /// Register `impl Into<to> for from` (§12.2).
    pub fn register_into_impl(&mut self, from: String, to: String) {
        self.into_impls.insert((from, to));
    }

    /// `from` converts to `to` via `Into`? Returns true for the auto-provided
    /// identity (`Into<T> for T`) as well as any explicitly registered impl.
    pub fn has_into_conversion(&self, from: &str, to: &str) -> bool {
        from == to
            || self
                .into_impls
                .contains(&(from.to_string(), to.to_string()))
    }

    /// Register the ability set for a named type (§8 auto-derivation).
    pub fn register_type_abilities(&mut self, type_name: String, abilities: HashSet<Ability>) {
        self.type_abilities.insert(type_name, abilities);
    }

    /// Mark a name as a user-defined type (value/data/ADT).
    pub fn register_user_type(&mut self, type_name: String) {
        self.user_defined_types.insert(type_name);
    }

    /// Check whether a name refers to a user-defined type.
    pub fn is_user_defined_type(&self, type_name: &str) -> bool {
        self.user_defined_types.contains(type_name)
    }

    /// Check whether a Ty has a given ability. Handles primitives inline
    /// and dispatches to type_abilities for named/generic types.
    pub fn ty_has_ability(&self, ty: &Ty, ability: Ability) -> bool {
        match ty {
            Ty::Int | Ty::Rune | Ty::Bytes => matches!(
                ability,
                Ability::Eq | Ability::Hash | Ability::Ord | Ability::Debug
            ),
            Ty::String => matches!(
                ability,
                Ability::Eq | Ability::Hash | Ability::Ord | Ability::Debug
            ),
            Ty::Bool | Ty::Unit => matches!(
                ability,
                Ability::Eq | Ability::Hash | Ability::Ord | Ability::Debug
            ),
            // Float: ADR-0002 — NO Eq, hence no Hash or Ord either.
            Ty::Float => matches!(ability, Ability::Debug),
            Ty::Never => true, // bottom type satisfies anything (vacuously)
            Ty::Error => true, // cascade: don't double-report
            Ty::Named(name) => self
                .type_abilities
                .get(name.as_str())
                .map(|a| a.contains(&ability))
                .unwrap_or(false),
            // Prelude ADTs Option<T> / Result<T, E>: §8.6 structural derivation.
            // Ability X holds iff every type argument has X. ADTs never auto-derive
            // Ord (§8.5), so Ord is always false regardless of arguments.
            Ty::Generic(name, args) if name == "Option" || name == "Result" => {
                ability != Ability::Ord && args.iter().all(|a| self.ty_has_ability(a, ability))
            }
            Ty::Generic(name, _) => self
                .type_abilities
                .get(name.as_str())
                .map(|a| a.contains(&ability))
                .unwrap_or(false),
            _ => false,
        }
    }
}

impl Default for TypeEnv {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-check a source file. Returns a map from source spans to inferred types
/// for use by the LSP hover handler.
pub fn check(file: &SourceFile, report: &mut Report) -> TypeIndex {
    let mut env = TypeEnv::new();
    register_prelude(&mut env);
    collect_top_level_types(&file.items, &mut env);

    for item in &file.items {
        check_item(item, &mut env, report);
    }
    env.type_index
}

/// Register prelude function types.
fn register_prelude(env: &mut TypeEnv) {
    // print/println accept any Debug type: fn<T: Debug>(T) -> Unit
    // Since generics are not yet implemented, we use Ty::Error as the parameter
    // type to accept any argument without type mismatch errors.
    // TODO: Replace with proper generic constraint checking when generics are implemented.
    for name in &["print", "println", "eprint", "eprintln"] {
        env.define(
            name.to_string(),
            Ty::Fn(vec![Ty::Error], Box::new(Ty::Unit)),
        );
    }
    env.define(
        "panic".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Never)),
    );
    // parse::<T>(str) -> Option<T>: generic, use Error as escape hatch
    env.define(
        "parse".to_string(),
        Ty::Fn(vec![Ty::Error], Box::new(Ty::Error)),
    );
    // M10 phase 1: fs stdlib intrinsics. See stdlib/fs.tyra.
    env.define(
        "__fs_read_raw".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::String)),
    );
    env.define("__fs_errno".to_string(), Ty::Fn(vec![], Box::new(Ty::Int)));
    env.define(
        "__fs_errmsg".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    env.define(
        "__fs_write_raw".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Unit)),
    );
    env.define(
        "__fs_exists".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Bool)),
    );
    // M11 phase 1: http client intrinsics. See stdlib/http/client.tyra.
    env.define(
        "__http_get".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__http_status".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Int)),
    );
    env.define(
        "__http_body".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::String)),
    );
    env.define(
        "__http_errno".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__http_errmsg".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    // M11 phase 2: http server intrinsics.
    // Server handles, handler function pointers, and the opaque AppServer
    // ADT are all represented as Int (i64) at the intrinsic boundary.
    // handler is typed `String` (ptr) — codegen resolves fn-ident args
    // directly to the LLVM symbol.
    env.define(
        "__http_server_new".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__http_server_route".to_string(),
        Ty::Fn(
            vec![Ty::Int, Ty::String, Ty::String, Ty::String],
            Box::new(Ty::Unit),
        ),
    );
    env.define(
        "__http_server_listen".to_string(),
        Ty::Fn(vec![Ty::Int, Ty::Int], Box::new(Ty::Int)),
    );
    // M10 phase 2: json stdlib intrinsics. See stdlib/json.tyra.
    // Handles are opaque Int values (0 = error / absent).
    env.define(
        "__json_parse".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__json_err_msg".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    env.define(
        "__json_err_line".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__json_err_col".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__json_kind".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::String)),
    );
    env.define(
        "__json_is_string".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool)),
    );
    env.define(
        "__json_is_int".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool)),
    );
    env.define(
        "__json_is_bool".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool)),
    );
    env.define(
        "__json_str".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::String)),
    );
    env.define(
        "__json_int".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Int)),
    );
    env.define(
        "__json_bool".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Bool)),
    );
    env.define(
        "__json_get".to_string(),
        Ty::Fn(vec![Ty::Int, Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__json_at".to_string(),
        Ty::Fn(vec![Ty::Int, Ty::Int], Box::new(Ty::Int)),
    );
    // stdin intrinsics. See stdlib/io.tyra.
    env.define(
        "__io_read_line".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    env.define(
        "__io_read_to_end".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    env.define("__io_eof".to_string(), Ty::Fn(vec![], Box::new(Ty::Bool)));
    // §17.3.4: string stdlib intrinsics. See stdlib/string.tyra.
    env.define(
        "__string_len".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__string_is_empty".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Bool)),
    );
    env.define(
        "__string_trim".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::String)),
    );
    env.define(
        "__string_to_upper".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::String)),
    );
    env.define(
        "__string_to_lower".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::String)),
    );
    env.define(
        "__string_contains".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Bool)),
    );
    env.define(
        "__string_starts_with".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Bool)),
    );
    env.define(
        "__string_ends_with".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Bool)),
    );
    env.define(
        "__string_parse_int".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__string_parse_errno".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__string_byte_at".to_string(),
        Ty::Fn(vec![Ty::String, Ty::Int], Box::new(Ty::Int)),
    );
    env.define(
        "__string_substring".to_string(),
        Ty::Fn(vec![Ty::String, Ty::Int, Ty::Int], Box::new(Ty::String)),
    );
    env.define(
        "__string_reverse".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::String)),
    );
    env.define(
        "__string_from_byte".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::String)),
    );
    let list_string = Ty::Generic("List".into(), vec![Ty::String]);
    env.define(
        "__string_split_whitespace".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(list_string.clone())),
    );
    env.define(
        "__string_split".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(list_string.clone())),
    );
    // §17.3.x: float stdlib intrinsics. See stdlib/float.tyra.
    env.define(
        "__float_eq".to_string(),
        Ty::Fn(vec![Ty::Float, Ty::Float], Box::new(Ty::Bool)),
    );
    env.define(
        "__float_approx_eq".to_string(),
        Ty::Fn(vec![Ty::Float, Ty::Float, Ty::Float], Box::new(Ty::Bool)),
    );
    env.define(
        "__float_abs".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_floor".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_ceil".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_round".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_min".to_string(),
        Ty::Fn(vec![Ty::Float, Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_max".to_string(),
        Ty::Fn(vec![Ty::Float, Ty::Float], Box::new(Ty::Float)),
    );
    env.define(
        "__float_to_string".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::String)),
    );
    env.define(
        "__float_parse".to_string(),
        Ty::Fn(vec![Ty::String], Box::new(Ty::Float)),
    );
    env.define(
        "__float_parse_errno".to_string(),
        Ty::Fn(vec![], Box::new(Ty::Int)),
    );
    env.define(
        "__float_from_int".to_string(),
        Ty::Fn(vec![Ty::Int], Box::new(Ty::Float)),
    );
    env.define(
        "__float_to_int".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Int)),
    );
    env.define(
        "__float_is_nan".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Bool)),
    );
    env.define(
        "__float_is_infinite".to_string(),
        Ty::Fn(vec![Ty::Float], Box::new(Ty::Bool)),
    );
    // §17.3.6 Map intrinsics (Map<String, Int> only). The "handle" is a
    // raw pointer; we surface it as Ty::String here since v0.1 has no
    // dedicated Ty::Ptr (mirrors the List<T> data-pointer convention).
    env.define(
        "__map_new_string_int".to_string(),
        Ty::Fn(vec![], Box::new(Ty::String)),
    );
    env.define(
        "__map_insert_string_int".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String, Ty::Int], Box::new(Ty::String)),
    );
    env.define(
        "__map_get_string_int".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Int)),
    );
    env.define(
        "__map_contains_string_int".to_string(),
        Ty::Fn(vec![Ty::String, Ty::String], Box::new(Ty::Bool)),
    );

    // §17.3.5: list stdlib intrinsics (List<Int> only). See stdlib/list.tyra.
    let list_int = Ty::Generic("List".into(), vec![Ty::Int]);
    let opt_int = Ty::Generic("Option".into(), vec![Ty::Int]);
    env.define(
        "__list_int_push".to_string(),
        Ty::Fn(vec![list_int.clone(), Ty::Int], Box::new(list_int.clone())),
    );
    env.define(
        "__list_int_sum".to_string(),
        Ty::Fn(vec![list_int.clone()], Box::new(Ty::Int)),
    );
    env.define(
        "__list_int_max".to_string(),
        Ty::Fn(vec![list_int.clone()], Box::new(opt_int.clone())),
    );
    env.define(
        "__list_int_min".to_string(),
        Ty::Fn(vec![list_int.clone()], Box::new(opt_int.clone())),
    );
    env.define(
        "__list_int_contains".to_string(),
        Ty::Fn(vec![list_int.clone(), Ty::Int], Box::new(Ty::Bool)),
    );
    env.define(
        "__list_int_index_of".to_string(),
        Ty::Fn(vec![list_int, Ty::Int], Box::new(opt_int)),
    );

    // Prelude ADTs for §10.3 exhaustiveness checking.
    env.register_adt("Option".into(), vec!["Some".into(), "None".into()]);
    env.register_adt("Result".into(), vec!["Ok".into(), "Err".into()]);

    // Prelude traits (§8.7, §17.1).
    //
    // Only traits whose implementations are user-facing are registered here.
    // That means:
    // - Stringable: requires an explicit `impl Stringable for T` per §8.7.
    //
    // Not registered (intentionally):
    // - Eq / Hash / Ord / Debug — auto-derived per §8.5/§8.6; enforcing an
    //   explicit impl for them would conflict with the derivation rules.
    // - Into — conversion trait is resolved at MIR lowering (§12.2 `?`),
    //   not via impl registry.
    env.register_trait("Stringable".into(), vec!["to_string".into()]);
}

/// Collect abilities from a set of fields by conjunction: the type has ability X
/// iff every field's type has ability X.
/// Empty input: returns the full candidate set (vacuous truth). Callers must
/// strip abilities that are structurally disallowed (e.g. Ord on empty value,
/// Ord on ADT/data) themselves.
fn conjunct_field_abilities(field_tys: &[Ty], env: &TypeEnv) -> HashSet<Ability> {
    let candidates = [Ability::Eq, Ability::Hash, Ability::Ord, Ability::Debug];
    candidates
        .into_iter()
        .filter(|a| field_tys.iter().all(|t| env.ty_has_ability(t, *a)))
        .collect()
}

/// Collect top-level function signatures and type definitions.
/// Runs in two passes so that ability derivation (pass 2) can see every
/// user-defined type regardless of source order — forward references across
/// `value`/`data`/`type` are allowed.
fn collect_top_level_types(items: &[Item], env: &mut TypeEnv) {
    // Pass 1: register names only (fn signatures, ADT variants, user type marker,
    // trait method lists, impl method maps). Ability sets are intentionally left
    // empty or left to pass 2.
    for item in items {
        match item {
            Item::FnDef(f) => {
                let param_tys: Vec<Ty> = f
                    .params
                    .iter()
                    .map(|p| Ty::from_type_expr(&p.type_annotation))
                    .collect();
                let ret_ty = f
                    .return_type
                    .as_ref()
                    .map(Ty::from_type_expr)
                    .unwrap_or(Ty::Unit);
                env.define(f.name.clone(), Ty::Fn(param_tys, Box::new(ret_ty)));
            }
            Item::TypeDef(t) => {
                if let TypeDefKind::Adt(variants) = &t.kind {
                    let names: Vec<String> = variants.iter().map(|v| v.name.clone()).collect();
                    env.register_adt(t.name.clone(), names);
                    env.register_user_type(t.name.clone());
                }
            }
            Item::ValueDef(v) => {
                env.register_user_type(v.name.clone());
            }
            Item::DataDef(d) => {
                env.register_user_type(d.name.clone());
            }
            Item::TraitDef(t) => {
                let methods: Vec<String> = t.methods.iter().map(|m| m.name.clone()).collect();
                env.register_trait(t.name.clone(), methods);
            }
            Item::ImplDef(i) => {
                let methods: Vec<String> = i.methods.iter().map(|m| m.name.clone()).collect();
                if let Some(target_name) = type_expr_name(&i.target_type) {
                    env.register_impl(i.trait_name.clone(), target_name.clone(), methods);
                    // §12.2: track `impl Into<To> for From` so `?` on Result
                    // can verify the error conversion exists.
                    if i.trait_name == "Into"
                        && i.trait_type_args.len() == 1
                        && let Some(to_name) = type_expr_name(&i.trait_type_args[0])
                    {
                        env.register_into_impl(target_name, to_name);
                    }
                }
            }
            _ => {}
        }
    }

    // Pass 2: ability auto-derivation (§8.5/§8.6).
    // Seeded optimistically: every user type starts with the full ability set,
    // then each pass conjunction-shrinks based on fields/variants. Sets only
    // shrink, so the fixed point is reached in at most |types| * |abilities|
    // iterations — in practice 1–2 passes.
    let all_abilities: HashSet<Ability> =
        [Ability::Eq, Ability::Hash, Ability::Ord, Ability::Debug]
            .into_iter()
            .collect();
    for name in env.user_defined_types.clone() {
        env.register_type_abilities(name, all_abilities.clone());
    }

    loop {
        let mut changed = false;
        for item in items {
            let (name, new_abilities) = match item {
                Item::TypeDef(t) => {
                    if let TypeDefKind::Adt(variants) = &t.kind {
                        let field_tys: Vec<Ty> = variants
                            .iter()
                            .flat_map(|v| {
                                v.fields
                                    .iter()
                                    .map(|f| Ty::from_type_expr(&f.type_annotation))
                            })
                            .collect();
                        let mut abilities = conjunct_field_abilities(&field_tys, env);
                        abilities.remove(&Ability::Ord); // §8.5: ADTs never auto-derive Ord
                        (t.name.clone(), abilities)
                    } else {
                        continue;
                    }
                }
                Item::ValueDef(v) => {
                    let field_tys: Vec<Ty> = v
                        .fields
                        .iter()
                        .map(|f| Ty::from_type_expr(&f.type_annotation))
                        .collect();
                    let mut abilities = conjunct_field_abilities(&field_tys, env);
                    // Ord: §8.6 — only single-field value where that field has Ord.
                    // Empty-field value has zero fields, explicitly not Ord.
                    let ord_ok =
                        v.fields.len() == 1 && env.ty_has_ability(&field_tys[0], Ability::Ord);
                    if !ord_ok {
                        abilities.remove(&Ability::Ord);
                    }
                    (v.name.clone(), abilities)
                }
                Item::DataDef(d) => {
                    let field_tys: Vec<Ty> = d
                        .fields
                        .iter()
                        .map(|f| Ty::from_type_expr(&f.type_annotation))
                        .collect();
                    let mut abilities = conjunct_field_abilities(&field_tys, env);
                    abilities.remove(&Ability::Ord); // §8.6: data never auto-derives Ord
                    if d.fields.iter().any(|f| f.is_mut) {
                        abilities.remove(&Ability::Hash); // §8.6: mut fields disallow Hash
                    }
                    (d.name.clone(), abilities)
                }
                _ => continue,
            };
            let prev = env.type_abilities.get(&name).cloned().unwrap_or_default();
            if prev != new_abilities {
                changed = true;
                env.register_type_abilities(name, new_abilities);
            }
        }
        if !changed {
            break;
        }
    }
}

/// Return `true` when return-type checking should be skipped for this pair.
/// Used by both implicit-final-expression (check_fn) and explicit `return` (Stmt::Return)
/// paths to keep the skip rules in sync.
///
/// TODO: Named/Generic are skipped because the type checker doesn't yet resolve
/// constructor calls (e.g. `Some(x)` → Option<T>) — once that lands, these can
/// be tightened and the check will cover real-world return-type mismatches.
fn return_check_skip(declared: &Ty, actual: &Ty) -> bool {
    actual.is_error()
        || declared.is_error()
        || matches!(
            actual,
            Ty::Never | Ty::Unit | Ty::Named(_) | Ty::Generic(_, _)
        )
        || matches!(declared, Ty::Named(_) | Ty::Generic(_, _))
}

/// Extract the outermost type name from a TypeExpr, if simple.
fn type_expr_name(ty: &TypeExpr) -> Option<String> {
    match &ty.kind {
        TypeExprKind::Named(name) => Some(name.clone()),
        TypeExprKind::Generic(name, _) => Some(name.clone()),
        TypeExprKind::Fn(..) => None,
    }
}

fn check_item(item: &Item, env: &mut TypeEnv, report: &mut Report) {
    match item {
        Item::FnDef(f) => check_fn(f, env, None, report),
        Item::Stmt(s) => {
            check_stmt(s, env, report);
        }
        Item::ImplDef(i) => check_impl(i, env, report),
        // TypeDef/TraitDef bodies are registered in collect_top_level_types;
        // no further per-item checks yet.
        _ => {}
    }
}

/// §8.7: verify that an `impl Trait for Type` implements every required method.
fn check_impl(i: &ImplDef, env: &mut TypeEnv, report: &mut Report) {
    let Some(required) = env.trait_methods(&i.trait_name).cloned() else {
        // Trait not registered — either unknown trait (resolver should have caught
        // this) or an external trait (Into, Eq, Hash, Ord, Debug are auto/external).
        // Skip silently; we only enforce explicit impls for registered traits.
        return;
    };
    let implemented: HashSet<String> = i.methods.iter().map(|m| m.name.clone()).collect();
    let missing: Vec<String> = required
        .iter()
        .filter(|m| !implemented.contains(m.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let target = type_expr_name(&i.target_type).unwrap_or_else(|| "?".into());
        let quoted: Vec<String> = missing.iter().map(|m| format!("`{m}`")).collect();
        report.add(
            Diagnostic::error(format!(
                "impl of `{}` for `{target}` is missing method {}",
                i.trait_name,
                quoted.join(", ")
            ))
            .with_code("E0500")
            .with_label(Label::new(i.span, "missing required trait methods"))
            .with_note(format!(
                "trait `{}` requires: {}",
                i.trait_name,
                required
                    .iter()
                    .map(|m| format!("`{m}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        );
    }

    // Type-check each method body. Bind `self` to the impl's target type so
    // method bodies get meaningful E0308/E0306 diagnostics on `self.field`.
    let self_ty = Ty::from_type_expr(&i.target_type);
    for m in &i.methods {
        check_fn(m, env, Some(&self_ty), report);
    }
}

fn check_fn(f: &FnDef, env: &mut TypeEnv, self_ty: Option<&Ty>, report: &mut Report) {
    env.push();
    for param in &f.params {
        let ty = Ty::from_type_expr(&param.type_annotation);
        env.record_type(param.span, ty.clone());
        env.define(param.name.clone(), ty);
    }
    if f.self_param.is_some() {
        // Free fns pass None (no enclosing impl); impl methods pass the target type
        // so `self.field` type-checks against the impl'd type rather than silently
        // cascading through Error.
        let ty = self_ty.cloned().unwrap_or(Ty::Error);
        env.define("self".to_string(), ty);
    }

    let declared_ret = f
        .return_type
        .as_ref()
        .map(Ty::from_type_expr)
        .unwrap_or(Ty::Unit);

    // Push return type context so Stmt::Return and `?` can access it.
    env.push_return_type(declared_ret.clone());

    // Walk body and track the last expression statement's type
    let mut last_expr_ty: Option<Ty> = None;
    let mut last_expr_span = None;
    for (i, stmt) in f.body.iter().enumerate() {
        check_stmt(stmt, env, report);
        // Cache the last expression statement's type (avoids double inference)
        if i + 1 == f.body.len()
            && let Stmt::Expr(expr_stmt) = stmt
        {
            last_expr_ty = Some(infer_expr(&expr_stmt.expr, env, report));
            last_expr_span = Some(expr_stmt.expr.span);
        }
    }

    // Return type verification: check that the last expression's type matches
    // the declared return type (if any). Explicit `return` stmts are checked in
    // check_stmt via the return_type_stack.

    if declared_ret != Ty::Unit
        && let (Some(actual_ty), Some(span)) = (last_expr_ty, last_expr_span)
        && !return_check_skip(&declared_ret, &actual_ty)
        && actual_ty != declared_ret
    {
        report.add(
            Diagnostic::error(format!(
                "return type mismatch: expected {}, found {}",
                declared_ret.display_name(),
                actual_ty.display_name()
            ))
            .with_code("E0309")
            .with_label(Label::new(span, "this expression has the wrong type")),
        );
    }

    env.pop_return_type();
    env.pop();
}

fn check_stmt(stmt: &Stmt, env: &mut TypeEnv, report: &mut Report) {
    match stmt {
        Stmt::Let(s) => {
            let value_ty = infer_expr(&s.value, env, report);
            // When a type annotation is present, use the declared type as the
            // binding type rather than the inferred value type.  This lets the
            // type checker resolve generic collections correctly: e.g.
            // `let xs: List<String> = []` — the empty-list literal infers as
            // `List<Var(0)>` (unknown element), but the annotation pins the
            // binding to `List<String>`, allowing later `list.push` element-
            // type checks to see the concrete element type.
            let binding_ty = if let Some(annotation) = &s.type_annotation {
                let expected = Ty::from_type_expr(annotation);
                check_type_match(&expected, &value_ty, s.span, report);
                expected
            } else {
                value_ty
            };
            env.record_type(s.span, binding_ty.clone());
            env.define_let(s.name.clone(), binding_ty);
        }
        Stmt::Mut(s) => {
            let value_ty = infer_expr(&s.value, env, report);
            // Same annotation-takes-precedence logic as Stmt::Let above.
            let binding_ty = if let Some(annotation) = &s.type_annotation {
                let expected = Ty::from_type_expr(annotation);
                check_type_match(&expected, &value_ty, s.span, report);
                expected
            } else {
                value_ty
            };
            env.record_type(s.span, binding_ty.clone());
            env.define(s.name.clone(), binding_ty);
        }
        Stmt::Return(s) => {
            // For bare `return`, actual is definitively Unit — no deferral needed.
            let (actual_ty, is_bare) = match &s.value {
                Some(v) => (infer_expr(v, env, report), false),
                None => (Ty::Unit, true),
            };
            // Compare against the enclosing fn's declared return type.
            if let Some(declared_ret) = env.current_return_type().cloned() {
                // Bare `return` should error when declared_ret != Unit, even though
                // the general skip rules ignore Unit actuals (they exist to tolerate
                // unresolved if/else). Skip rules still apply when declared is Named/Generic
                // (type checker hasn't resolved the constructor).
                let should_check = if is_bare {
                    !matches!(declared_ret, Ty::Named(_) | Ty::Generic(_, _))
                        && !declared_ret.is_error()
                } else {
                    !return_check_skip(&declared_ret, &actual_ty)
                };
                if should_check && actual_ty != declared_ret {
                    let span = s.value.as_ref().map(|v| v.span).unwrap_or(s.span);
                    report.add(
                        Diagnostic::error(format!(
                            "return type mismatch: expected {}, found {}",
                            declared_ret.display_name(),
                            actual_ty.display_name()
                        ))
                        .with_code("E0309")
                        .with_label(Label::new(span, "this return value has the wrong type")),
                    );
                }
            }
        }
        Stmt::Defer(s) => {
            infer_expr(&s.expr, env, report);
        }
        Stmt::Break(s) => {
            if !env.in_loop() {
                report.add(
                    Diagnostic::error("`break` used outside of a loop")
                        .with_code("E0214")
                        .with_label(Label::new(s.span, "`break` is only valid inside while/for")),
                );
            }
        }
        Stmt::Continue(s) => {
            if !env.in_loop() {
                report.add(
                    Diagnostic::error("`continue` used outside of a loop")
                        .with_code("E0215")
                        .with_label(Label::new(
                            s.span,
                            "`continue` is only valid inside while/for",
                        )),
                );
            }
        }
        Stmt::Expr(s) => {
            infer_expr(&s.expr, env, report);
        }
    }
}

/// Infer the type of an expression.
pub fn infer_expr(expr: &Expr, env: &mut TypeEnv, report: &mut Report) -> Ty {
    match &expr.kind {
        // Literals
        ExprKind::IntLit(_) => Ty::Int,
        ExprKind::FloatLit(_) => Ty::Float,
        ExprKind::StringLit(_) => Ty::String,
        ExprKind::StringInterp(_) => Ty::String,
        ExprKind::BoolLit(_) => Ty::Bool,
        ExprKind::UnitLit => Ty::Unit,
        ExprKind::ListLit(items) => {
            if items.is_empty() {
                Ty::Generic("List".into(), vec![Ty::Var(0)])
            } else {
                let elem_ty = infer_expr(&items[0], env, report);
                Ty::Generic("List".into(), vec![elem_ty])
            }
        }
        ExprKind::MapLit(entries) => {
            // v0.1 supports `Map<String, Int>` only — the runtime backs it
            // with a linked-list-of-(key, value) (see runtime/src/stdlib_map.rs).
            // Other K / V combinations are tracked in §22 as deferred.
            if entries.is_empty() {
                // Empty literal needs a type annotation to disambiguate.
                // Without a binding hint, fall back to Map<String, Int>.
                return Ty::Generic("Map".into(), vec![Ty::String, Ty::Int]);
            }
            let key_ty = infer_expr(&entries[0].0, env, report);
            let val_ty = infer_expr(&entries[0].1, env, report);
            for (k, v) in entries.iter().skip(1) {
                infer_expr(k, env, report);
                infer_expr(v, env, report);
            }
            if !matches!(key_ty, Ty::String | Ty::Error) || !matches!(val_ty, Ty::Int | Ty::Error) {
                report.add(
                    Diagnostic::error(format!(
                        "map literals are restricted to `Map<String, Int>` in v0.1, \
                         got `Map<{}, {}>`",
                        key_ty.display_name(),
                        val_ty.display_name()
                    ))
                    .with_label(Label::new(expr.span, "unsupported key/value type")),
                );
                return Ty::Error;
            }
            Ty::Generic("Map".into(), vec![Ty::String, Ty::Int])
        }

        // Identifier lookup
        ExprKind::Ident(name) => {
            let ty = env.lookup(name).cloned().unwrap_or(Ty::Error);
            env.record_type(expr.span, ty.clone());
            ty
        }

        // Field access — deferred (needs type info about the target)
        ExprKind::FieldAccess(obj, _) => {
            infer_expr(obj, env, report);
            Ty::Error // field resolution requires knowing the object's type definition
        }

        // Binary operations (§10.1)
        ExprKind::BinaryOp(left, op, right) => {
            let left_ty = infer_expr(left, env, report);
            let right_ty = infer_expr(right, env, report);
            infer_binop(*op, &left_ty, &right_ty, expr.span, env, report)
        }

        // Unary operations
        ExprKind::UnaryOp(op, operand) => {
            let ty = infer_expr(operand, env, report);
            match op {
                UnaryOp::Neg => {
                    if !matches!(ty, Ty::Int | Ty::Float | Ty::Error) {
                        report.add(
                            Diagnostic::error(format!(
                                "unary `-` requires Int or Float, found {}",
                                ty.display_name()
                            ))
                            .with_code("E0300")
                            .with_label(Label::new(expr.span, "cannot negate this type")),
                        );
                    }
                    ty
                }
                UnaryOp::Not => {
                    if !matches!(ty, Ty::Bool | Ty::Error) {
                        report.add(
                            Diagnostic::error(format!(
                                "`not` requires Bool, found {}",
                                ty.display_name()
                            ))
                            .with_code("E0300")
                            .with_label(Label::new(expr.span, "expected Bool")),
                        );
                    }
                    Ty::Bool
                }
            }
        }

        // Assignment
        ExprKind::Assign(lhs, rhs) => {
            // Reject assignment to `let`-bound (immutable) variables (E0206).
            if let ExprKind::Ident(name) = &lhs.kind
                && env.is_let_bound(name)
            {
                report.add(
                    Diagnostic::error(format!(
                        "cannot assign to `{name}` because it is not declared `mut`"
                    ))
                    .with_code("E0206")
                    .with_label(Label::new(expr.span, "assignment to immutable variable")),
                );
            }
            // Reject rebinding an outer `mut` from inside a closure (E0402, spec §9.4).
            if let ExprKind::Ident(name) = &lhs.kind
                && env.is_lambda_outer_mut(name)
            {
                report.add(
                    Diagnostic::error(format!(
                        "cannot assign to `{name}` captured by closure"
                    ))
                    .with_code("E0402")
                    .with_label(Label::new(
                        expr.span,
                        "closures capture `mut` bindings by reference but rebinding is forbidden (spec §9.4)",
                    )),
                );
            }
            infer_expr(lhs, env, report);
            infer_expr(rhs, env, report);
            Ty::Unit
        }

        // Function call
        ExprKind::Call(callee, args) => {
            // Method-call shape: `obj.method(args)` parses as Call(FieldAccess(obj, m), args).
            // Intercept to verify trait impls (§8.7) before general call type-checking.
            if let ExprKind::FieldAccess(obj, method) = &callee.kind {
                let obj_ty = infer_expr(obj, env, report);
                check_trait_method_call(&obj_ty, method, expr.span, env, report);

                // Special-case: module-qualified List mutations.
                // `list.push(xs: List<T>, x)` and `list.push_str` require the
                // element argument to match the list's element type.  The
                // generic method-call interceptor below intentionally skips
                // argument type-checking (to avoid cascading errors for
                // unresolved method returns), but that also silences `List<T>`
                // × wrong-type-element mismatches that would otherwise reach
                // LLVM and produce an opaque E0500.  We handle this case here
                // before falling through to the Ty::Error return.
                if let ExprKind::Ident(module_name) = &obj.kind
                    && module_name == "list"
                    && matches!(method.as_str(), "push" | "push_str")
                    && args.len() == 2
                {
                    let list_ty = infer_expr(&args[0].value, env, report);
                    let elem_ty = infer_expr(&args[1].value, env, report);
                    if let Ty::Generic(name, params) = &list_ty
                        && name == "List"
                        && let Some(expected) = params.first()
                    {
                        check_type_match(expected, &elem_ty, args[1].span, report);
                    }
                    return list_ty;
                }

                // Still infer arg types so argument errors surface.
                for arg in args {
                    infer_expr(&arg.value, env, report);
                }
                // Method return type is not yet resolved by the type checker
                // (impl method signatures live in MIR). Returning Ty::Error here
                // intentionally suppresses downstream E0308 — users see the
                // E0501 (Stringable impl) diagnostic without cascading false
                // positives. When method resolution lands, this should return
                // the impl method's declared return type.
                return Ty::Error;
            }

            let callee_ty = infer_expr(callee, env, report);
            match callee_ty {
                Ty::Fn(param_tys, ret_ty) => {
                    if args.len() != param_tys.len() {
                        report.add(
                            Diagnostic::error(format!(
                                "expected {} argument{}, found {}",
                                param_tys.len(),
                                if param_tys.len() == 1 { "" } else { "s" },
                                args.len()
                            ))
                            .with_code("E0301")
                            .with_label(Label::new(expr.span, "wrong number of arguments")),
                        );
                        // Still infer arg types to find errors in arguments
                        for arg in args {
                            infer_expr(&arg.value, env, report);
                        }
                    } else {
                        // Check each argument type against parameter type
                        for (arg, param_ty) in args.iter().zip(param_tys.iter()) {
                            let arg_ty = infer_expr(&arg.value, env, report);
                            check_type_match(param_ty, &arg_ty, arg.span, report);
                        }
                    }
                    *ret_ty
                }
                Ty::Error => {
                    for arg in args {
                        infer_expr(&arg.value, env, report);
                    }
                    Ty::Error
                }
                _ => {
                    // Could be a constructor call (e.g., Point(x: 1.0, y: 2.0))
                    // For now, accept and return Error
                    for arg in args {
                        infer_expr(&arg.value, env, report);
                    }
                    Ty::Error
                }
            }
        }

        ExprKind::TurbofishCall(callee, _, args) => {
            infer_expr(callee, env, report);
            for arg in args {
                infer_expr(&arg.value, env, report);
            }
            Ty::Error // turbofish resolution deferred
        }

        // Index
        ExprKind::Index(obj, idx) => {
            infer_expr(obj, env, report);
            infer_expr(idx, env, report);
            Ty::Error // element type resolution deferred
        }

        // Propagation (?)
        ExprKind::Propagate(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            // Determine the inner "ok" payload type and the operand's family (Option/Result).
            let (payload_ty, inner_kind) = match inner_ty {
                Ty::Generic(ref name, ref args) if name == "Option" && args.len() == 1 => {
                    (args[0].clone(), Some("Option"))
                }
                Ty::Generic(ref name, ref args) if name == "Result" && args.len() == 2 => {
                    (args[0].clone(), Some("Result"))
                }
                Ty::Error => (Ty::Error, None),
                _ => {
                    report.add(
                        Diagnostic::error(format!(
                            "`?` requires Option or Result, found {}",
                            inner_ty.display_name()
                        ))
                        .with_code("E0302")
                        .with_label(Label::new(expr.span, "cannot use `?` on this type")),
                    );
                    return Ty::Error;
                }
            };

            // §12.2: the enclosing fn must return the same family (Option/Result).
            // Option<T>? requires fn -> Option<U>; Result<T,E>? requires fn -> Result<U,F>.
            if let (Some(kind), Some(ret)) = (inner_kind, env.current_return_type()) {
                let ok = match ret {
                    Ty::Generic(name, args) => match kind {
                        "Option" => name == "Option" && args.len() == 1,
                        "Result" => name == "Result" && args.len() == 2,
                        _ => false,
                    },
                    Ty::Error => true, // cascade
                    _ => false,
                };
                if !ok {
                    report.add(
                        Diagnostic::error(format!(
                            "`?` on {kind} requires the enclosing function to return {kind}; found {}",
                            ret.display_name()
                        ))
                        .with_code("E0310")
                        .with_label(Label::new(
                            expr.span,
                            format!("cannot use `?` here — enclosing fn returns {}", ret.display_name()),
                        ))
                        .with_note(format!(
                            "change the function's return type to {kind}<...>, or handle the {kind} explicitly."
                        )),
                    );
                }

                // §12.2: for Result, verify the error-type Into<F> conversion
                // is available (or identity). E0311 fires when the inner E
                // differs from the enclosing F and no `impl Into<F> for E`
                // has been declared.
                if kind == "Result"
                    && let Some(inner_e) = inner_ty.result_err_type()
                    && let Some(ret_e) = ret.result_err_type()
                    // Skip cascade: if either error slot is already Ty::Error
                    // the upstream type check has already reported the root
                    // cause. Avoid chaining a misleading E0311 on top.
                    && !inner_e.is_error()
                    && !ret_e.is_error()
                {
                    let from = inner_e.monomorphized_name();
                    let to = ret_e.monomorphized_name();
                    if !env.has_into_conversion(&from, &to) {
                        report.add(
                            Diagnostic::error(format!(
                                "`?` cannot convert error type {} into {}: no `impl Into<{}> for {}` found",
                                inner_e.display_name(),
                                ret_e.display_name(),
                                ret_e.display_name(),
                                inner_e.display_name(),
                            ))
                            .with_code("E0311")
                            .with_label(Label::new(
                                expr.span,
                                "error type conversion required here",
                            ))
                            .with_note(format!(
                                "declare `impl Into<{}> for {}` with a `fn into(self) -> {}` method, \
                                 or restructure to return the inner error directly.",
                                ret_e.display_name(),
                                inner_e.display_name(),
                                ret_e.display_name(),
                            )),
                        );
                    }
                }
            }
            // Top-level `?` is reported as E0211 by the resolver (ADR-0006 Rule 3);
            // we don't duplicate the diagnostic here.

            payload_ty
        }

        // Await (§14.3): Task<T>.await unwraps to T; bare value .await is identity
        // (M9 runtime uses a real thread-pool; see runtime/src/task.rs)
        ExprKind::Await(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            match inner_ty {
                Ty::Generic(ref name, ref args) if name == "Task" && args.len() == 1 => {
                    args[0].clone()
                }
                other => other,
            }
        }

        // Control flow
        ExprKind::If(if_expr) => check_if(if_expr, env, report),
        ExprKind::Match(m) => {
            let subject_ty = infer_expr(&m.subject, env, report);
            check_match_pattern_compatibility(&subject_ty, m, report);
            check_match_exhaustiveness(&subject_ty, m, env, report);
            check_nested_exhaustiveness(&subject_ty, m, env, report);
            check_redundant_arms(m, report);
            // Unify arm types: divergent arms (Never — `return` / `?` / panic
            // / nested match-of-Never) are absorbed by other arms. Without
            // this, a single `when None return Err(...) end` arm would force
            // the whole match to type as Unit and any subsequent use of the
            // bound value would fail with E0305 (Unit vs Int comparison).
            let mut arm_ty: Option<Ty> = None;
            for arm in &m.arms {
                env.push();
                bind_pattern_types(&arm.pattern, env);
                for stmt in &arm.body {
                    check_stmt(stmt, env, report);
                }
                let this_ty = arm
                    .body
                    .last()
                    .map(|last| stmt_type(last, env, report))
                    .unwrap_or(Ty::Unit);
                env.pop();
                arm_ty = Some(match arm_ty.take() {
                    None => this_ty,
                    Some(prev) => match (&prev, &this_ty) {
                        (Ty::Never, _) => this_ty,
                        (_, Ty::Never) => prev,
                        (Ty::Error, _) => this_ty,
                        (_, Ty::Error) => prev,
                        _ => {
                            if !types_compatible(&prev, &this_ty) {
                                report.add(
                                    Diagnostic::error(format!(
                                        "match arms have incompatible types: `{}` vs `{}`",
                                        prev.display_name(),
                                        this_ty.display_name()
                                    ))
                                    .with_code("E0305")
                                    .with_label(Label::new(m.span, "mismatched arm types")),
                                );
                            }
                            prev
                        }
                    },
                });
            }
            arm_ty.unwrap_or(Ty::Unit)
        }
        ExprKind::For(f) => {
            infer_expr(&f.iter, env, report);
            env.push();
            env.enter_loop();
            env.define(f.binding.clone(), Ty::Error); // element type unknown without generics
            for stmt in &f.body {
                check_stmt(stmt, env, report);
            }
            env.exit_loop();
            env.pop();
            Ty::Unit
        }
        ExprKind::While(w) => {
            let cond_ty = infer_expr(&w.condition, env, report);
            if !matches!(cond_ty, Ty::Bool | Ty::Error) {
                report.add(
                    Diagnostic::error(format!(
                        "while condition must be Bool, found {}",
                        cond_ty.display_name()
                    ))
                    .with_code("E0304")
                    .with_label(Label::new(expr.span, "expected Bool")),
                );
            }
            env.push();
            env.enter_loop();
            for stmt in &w.body {
                check_stmt(stmt, env, report);
            }
            env.exit_loop();
            env.pop();
            Ty::Unit
        }

        ExprKind::Lambda(lam) => {
            let param_tys: Vec<Ty> = lam
                .params
                .iter()
                .map(|p| Ty::from_type_expr(&p.type_annotation))
                .collect();
            let ret_ty = lam
                .return_type
                .as_ref()
                .map(Ty::from_type_expr)
                .unwrap_or(Ty::Unit);
            let saved_loop_depth = env.loop_depth;
            env.loop_depth = 0;
            env.push_return_type(ret_ty.clone());
            // Snapshot outer mut bindings before entering the lambda scope (E0402).
            env.enter_lambda_scope();
            env.push();
            for (param, ty) in lam.params.iter().zip(&param_tys) {
                env.define(param.name.clone(), ty.clone());
            }
            for stmt in &lam.body {
                check_stmt(stmt, env, report);
            }
            env.pop();
            env.exit_lambda_scope();
            env.pop_return_type();
            env.loop_depth = saved_loop_depth;
            Ty::Fn(param_tys, Box::new(ret_ty))
        }

        ExprKind::Spawn(inner) => {
            let inner_ty = infer_expr(inner, env, report);
            Ty::Generic("Task".into(), vec![inner_ty])
        }
    }
}

fn check_if(if_expr: &IfExpr, env: &mut TypeEnv, report: &mut Report) -> Ty {
    let cond_ty = infer_expr(&if_expr.condition, env, report);
    if !matches!(cond_ty, Ty::Bool | Ty::Error) {
        report.add(
            Diagnostic::error(format!(
                "if condition must be Bool, found {}",
                cond_ty.display_name()
            ))
            .with_code("E0304")
            .with_label(Label::new(if_expr.span, "expected Bool")),
        );
    }

    env.push();
    for stmt in &if_expr.then_body {
        check_stmt(stmt, env, report);
    }
    let then_ty = if_expr
        .then_body
        .last()
        .map(|s| stmt_type(s, env, report))
        .unwrap_or(Ty::Unit);
    env.pop();

    let else_ty = match &if_expr.else_body {
        Some(ElseBranch::Else(body)) => {
            env.push();
            for stmt in body {
                check_stmt(stmt, env, report);
            }
            let ty = body
                .last()
                .map(|s| stmt_type(s, env, report))
                .unwrap_or(Ty::Unit);
            env.pop();
            ty
        }
        Some(ElseBranch::ElseIf(inner)) => check_if(inner, env, report),
        None => Ty::Unit,
    };

    // Unify arm types: if both arms agree (or one is Never/Error), use
    // that type; otherwise fall back to Ty::Unit for a statement-shaped
    // if. Without this, `let b = if c then 1 else 2 end` was reported as
    // Unit and subsequent `b + 1` tripped E0305.
    if types_compatible(&then_ty, &else_ty) {
        if then_ty.is_never() || then_ty.is_error() {
            else_ty
        } else {
            then_ty
        }
    } else {
        Ty::Unit
    }
}

/// Infer binary operator result type.
fn infer_binop(
    op: BinOp,
    left: &Ty,
    right: &Ty,
    span: Span,
    env: &TypeEnv,
    report: &mut Report,
) -> Ty {
    if left.is_error() || right.is_error() {
        return Ty::Error;
    }

    match op {
        // Arithmetic: Int/Float operands -> same type
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            if left == right && matches!(left, Ty::Int | Ty::Float) {
                left.clone()
            } else {
                report.add(
                    Diagnostic::error(format!(
                        "arithmetic operator requires matching Int or Float operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                Ty::Error
            }
        }
        // Remainder: Int-only. Float remainder is out of scope for v0.1
        // (no std library math surface; callers can compute via fmod later).
        BinOp::Rem => {
            if matches!(left, Ty::Int) && matches!(right, Ty::Int) {
                Ty::Int
            } else {
                report.add(
                    Diagnostic::error(format!(
                        "`%` requires Int operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                Ty::Error
            }
        }
        // Equality: same type with Eq ability -> Bool
        BinOp::Eq | BinOp::NotEq => {
            // §7.2: Float does NOT have Eq (ADR-0002)
            if matches!(left, Ty::Float) || matches!(right, Ty::Float) {
                report.add(
                    Diagnostic::error("Float does not have Eq; use float module for comparison")
                        .with_code("E0306")
                        .with_label(Label::new(span, "Float has no Eq (ADR-0002)"))
                        .with_note("use float.eq() or float.approx_eq() instead"),
                );
                return Ty::Error;
            }
            // Operands must be the same type
            if left != right {
                report.add(
                    Diagnostic::error(format!(
                        "cannot compare {} with {} using ==",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                return Ty::Error;
            }
            // §8: user-defined types need Eq ability (auto-derived from fields).
            if matches!(left, Ty::Named(_) | Ty::Generic(_, _))
                && !env.ty_has_ability(left, Ability::Eq)
            {
                report.add(
                    Diagnostic::error(format!(
                        "type `{}` does not have Eq; `==` is not available",
                        left.display_name()
                    ))
                    .with_code("E0306")
                    .with_label(Label::new(span, "missing Eq ability"))
                    .with_note(
                        "Eq is auto-derived when every field has Eq. Check for Float fields or mut fields that block auto-derivation.",
                    ),
                );
                return Ty::Error;
            }
            Ty::Bool
        }
        // Reference equality: only valid for data types (§8.6)
        // For now, require same type on both sides
        BinOp::RefEq => {
            if left != right {
                report.add(
                    Diagnostic::error(format!(
                        "cannot compare {} with {} using ===",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                return Ty::Error;
            }
            Ty::Bool
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            if left != right {
                report.add(
                    Diagnostic::error(format!(
                        "comparison requires matching operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0305")
                    .with_label(Label::new(span, "type mismatch")),
                );
                return Ty::Error;
            }
            // Built-in orderings: Int / Float.
            if matches!(left, Ty::Int | Ty::Float | Ty::Error) {
                return Ty::Bool;
            }
            // Named/Generic: require Ord ability (§8 auto-derivation).
            if matches!(left, Ty::Named(_) | Ty::Generic(_, _)) {
                if env.ty_has_ability(left, Ability::Ord) {
                    return Ty::Bool;
                }
                report.add(
                    Diagnostic::error(format!(
                        "type `{}` does not have Ord; comparison operators are not available",
                        left.display_name()
                    ))
                    .with_code("E0307")
                    .with_label(Label::new(span, "missing Ord ability"))
                    .with_note(
                        "Ord is auto-derived only for single-field value types whose field has Ord. data types and ADTs never auto-derive Ord.",
                    ),
                );
                return Ty::Error;
            }
            report.add(
                Diagnostic::error(format!(
                    "comparison not supported for {}",
                    left.display_name()
                ))
                .with_code("E0305")
                .with_label(Label::new(span, "unsupported operand type")),
            );
            Ty::Error
        }
        // Logical: Bool operands -> Bool (§10.1)
        BinOp::And | BinOp::Or => {
            if !matches!(left, Ty::Bool) || !matches!(right, Ty::Bool) {
                report.add(
                    Diagnostic::error(format!(
                        "logical operator requires Bool operands, found {} and {}",
                        left.display_name(),
                        right.display_name()
                    ))
                    .with_code("E0307")
                    .with_label(Label::new(span, "expected Bool")),
                );
            }
            Ty::Bool
        }
    }
}

/// §8.7: check that a trait-backed method call is valid — currently only
/// `.to_string()` requires explicit `impl Stringable for T`.
/// Other method calls are resolved at MIR lowering; we only flag the
/// well-known Stringable requirement here.
fn check_trait_method_call(
    obj_ty: &Ty,
    method: &str,
    span: Span,
    env: &TypeEnv,
    report: &mut Report,
) {
    if method != "to_string" {
        return; // only Stringable is enforced in v0.1
    }
    // Skip if we don't have a concrete named type to check (Error / primitives / generics).
    let type_name = match obj_ty {
        Ty::Named(name) => name.clone(),
        Ty::Generic(name, _) => name.clone(),
        _ => return,
    };
    // Primitives / prelude types have implicit Stringable via core formatting —
    // not enforced here. Only user-defined types require an explicit impl.
    // Heuristic: if the type is registered as an ADT/value/data and lacks the impl, warn.
    if !env.has_trait_impl("Stringable", &type_name) && env.is_user_defined_type(&type_name) {
        report.add(
            Diagnostic::error(format!(
                "type `{type_name}` does not implement Stringable; `to_string()` requires an explicit `impl Stringable for {type_name}`"
            ))
            .with_code("E0501")
            .with_label(Label::new(span, "no Stringable impl for this type"))
            .with_note(
                "add `impl Stringable for ...` or call the method on a type that implements it.",
            ),
        );
    }
}

fn check_type_match(expected: &Ty, actual: &Ty, span: Span, report: &mut Report) {
    if !types_compatible(expected, actual) {
        report.add(
            Diagnostic::error(format!(
                "type mismatch: expected {}, found {}",
                expected.display_name(),
                actual.display_name()
            ))
            .with_code("E0308")
            .with_label(Label::new(
                span,
                format!("expected {}", expected.display_name()),
            )),
        );
    }
}

/// A pattern is a catch-all when it matches any value:
/// - `_` (wildcard)
/// - a bare ident binding (e.g. `when x`) — binds `x` to the subject
fn is_catchall_pattern(kind: &PatternKind) -> bool {
    matches!(kind, PatternKind::Wildcard | PatternKind::Ident(_))
}

fn bind_pattern_types(pat: &Pattern, env: &mut TypeEnv) {
    match &pat.kind {
        PatternKind::Ident(name) => {
            env.define(name.clone(), Ty::Error); // actual type from match subject; deferred
        }
        PatternKind::Constructor(_, fields) => {
            for field in fields {
                bind_pattern_types(&field.pattern, env);
            }
        }
        _ => {}
    }
}

/// §10.3: `match` must be exhaustive.
/// Reports E0400 when an enumerable subject type (ADT, Option, Result, Bool)
/// has uncovered variants and no wildcard/ident catch-all arm.
///
/// Limitations (future work):
/// - Nested Constructor exhaustiveness (e.g. Err(NotFound) only) not checked
/// - Unknown Named types and generics other than Option/Result are skipped
/// - Literal exhaustiveness (Int/String) not checked
///
/// Check that match arm Constructor patterns are compatible with the subject type.
/// Catches the common case where `?` strips a Result to `T` but the arms still
/// use `Some(s)` / `None` or `Ok` / `Err` patterns against the plain value.
fn check_match_pattern_compatibility(subject_ty: &Ty, match_expr: &MatchExpr, report: &mut Report) {
    // Primitive types cannot be matched with Constructor patterns.
    let subject_is_primitive = matches!(
        subject_ty,
        Ty::Int | Ty::Float | Ty::String | Ty::Unit | Ty::Bool
    );
    // Generic ADT names expected by the subject (e.g. "Option", "Result").
    let subject_generic_name = match subject_ty {
        Ty::Generic(name, _) => Some(name.as_str()),
        _ => None,
    };

    for arm in &match_expr.arms {
        if let PatternKind::Constructor(ctor_name, _) = &arm.pattern.kind {
            if subject_is_primitive {
                report.add(
                    Diagnostic::error(format!(
                        "pattern type mismatch: subject has type `{}` but pattern `{}` is a constructor",
                        subject_ty.display_name(),
                        ctor_name,
                    ))
                    .with_code("E0312")
                    .with_label(Label::new(arm.pattern.span, "this constructor pattern does not match the subject type"))
                    .with_note("the subject is a primitive type and cannot be matched with constructor patterns"),
                );
                return; // one error is enough
            }
            // Option subject matched with Result constructors (or vice versa).
            if let Some(subj_name) = subject_generic_name {
                let ctor_is_option = matches!(ctor_name.as_str(), "Some" | "None");
                let ctor_is_result = matches!(ctor_name.as_str(), "Ok" | "Err");
                let mismatch = (subj_name == "Option" && ctor_is_result)
                    || (subj_name == "Result" && ctor_is_option);
                if mismatch {
                    report.add(
                        Diagnostic::error(format!(
                            "pattern type mismatch: subject has type `{}` but pattern `{}` belongs to `{}`",
                            subject_ty.display_name(),
                            ctor_name,
                            if ctor_is_option { "Option" } else { "Result" },
                        ))
                        .with_code("E0312")
                        .with_label(Label::new(arm.pattern.span, "pattern does not match subject type")),
                    );
                    return;
                }
            }
        }
    }
}

fn check_match_exhaustiveness(
    subject_ty: &Ty,
    match_expr: &MatchExpr,
    env: &TypeEnv,
    report: &mut Report,
) {
    // A wildcard or ident-binding arm is a catch-all — exhaustiveness is satisfied.
    // Rationale: in a match context, a bare lowercase ident is always a fresh
    // binding (not a constructor reference — those parse as `Constructor(name, _)`).
    // This mirrors the semantics of Rust and OCaml: `when x` binds `x` to the
    // subject and matches any value, identical to `when _` except that the value
    // is nameable inside the arm body.
    let has_catchall = match_expr
        .arms
        .iter()
        .any(|arm| is_catchall_pattern(&arm.pattern.kind));
    if has_catchall {
        return;
    }

    // Determine the expected variant set for the subject type.
    let (type_display, expected): (String, Vec<String>) = match subject_ty {
        Ty::Bool => ("Bool".into(), vec!["true".into(), "false".into()]),
        Ty::Named(name) => match env.adt_variants(name) {
            Some(vs) => (name.clone(), vs.clone()),
            None => return, // not an enumerable type → skip
        },
        Ty::Generic(name, _) if name == "Option" || name == "Result" => {
            match env.adt_variants(name) {
                Some(vs) => (name.clone(), vs.clone()),
                None => return,
            }
        }
        _ => return, // non-enumerable or unresolved type → skip
    };

    if expected.is_empty() {
        return;
    }

    // Collect variant names matched by Constructor/BoolLit patterns.
    let mut covered: HashSet<String> = HashSet::new();
    for arm in &match_expr.arms {
        match &arm.pattern.kind {
            PatternKind::Constructor(name, _) => {
                covered.insert(name.clone());
            }
            PatternKind::BoolLit(b) => {
                covered.insert(if *b { "true".into() } else { "false".into() });
            }
            _ => {}
        }
    }

    let missing: Vec<String> = expected
        .iter()
        .filter(|v| !covered.contains(v.as_str()))
        .cloned()
        .collect();

    if !missing.is_empty() {
        let quoted: Vec<String> = missing.iter().map(|v| format!("`{v}`")).collect();
        report.add(
            Diagnostic::error(format!(
                "non-exhaustive match on {type_display}: missing pattern {}",
                quoted.join(", ")
            ))
            .with_code("E0400")
            .with_label(Label::new(match_expr.span, "non-exhaustive patterns"))
            .with_note(format!(
                "not covered: {}. Add arms for these patterns or use `_` for a catch-all.",
                quoted.join(", ")
            )),
        );
    }
}

/// §10.3: check nested Constructor exhaustiveness for Option<ADT> / Result<ADT, _> / Result<_, ADT>.
/// Example: `match r when Ok(x) => ... when Err(NotFound) => ... end` with
/// `r: Result<T, MyErr>` and `MyErr = | NotFound | Forbidden` reports E0401
/// because `Err(Forbidden)` is uncovered.
///
/// Limitations (acknowledged in the emitted note):
/// - depth-1 only — only the field directly inside the outer Constructor is checked.
///   Deeper nesting (e.g. `Some(Ok(Red))`) is not analyzed.
/// - Only Option/Result. User ADTs with ADT fields are not supported yet.
/// - `Ty::Generic` inner types (e.g. `Option<Option<T>>`) are skipped — only direct
///   `Ty::Named` ADTs are analyzed. See `adt_name_of`.
fn check_nested_exhaustiveness(
    subject_ty: &Ty,
    match_expr: &MatchExpr,
    env: &TypeEnv,
    report: &mut Report,
) {
    // Skip if the outer match already has a catch-all — nothing to check further.
    let outer_catchall = match_expr
        .arms
        .iter()
        .any(|arm| is_catchall_pattern(&arm.pattern.kind));
    if outer_catchall {
        return;
    }

    // Per-outer-variant inner ADT name (fixed set — Option/Result only).
    // Small N (2) so a simple array beats a HashMap here.
    let inner_types: Vec<(&'static str, Option<String>)> = match subject_ty {
        Ty::Generic(name, args) if name == "Option" && args.len() == 1 => {
            vec![("Some", adt_name_of(&args[0], env)), ("None", None)]
        }
        Ty::Generic(name, args) if name == "Result" && args.len() == 2 => {
            vec![
                ("Ok", adt_name_of(&args[0], env)),
                ("Err", adt_name_of(&args[1], env)),
            ]
        }
        _ => return,
    };

    // For each outer variant, accumulate inner patterns used.
    // State: (covered inner variant names, nested catchall seen, first-arm span for labels).
    #[derive(Default)]
    struct InnerState {
        covered: HashSet<String>,
        has_catchall: bool,
        /// Span of the first nested arm encountered for this outer variant.
        /// Used as the label location for E0401 — narrower than the match span.
        first_arm_span: Option<Span>,
    }
    let mut inner_info: Vec<(&'static str, InnerState)> = inner_types
        .iter()
        .map(|(v, _)| (*v, InnerState::default()))
        .collect();

    for arm in &match_expr.arms {
        if let PatternKind::Constructor(outer_name, fields) = &arm.pattern.kind {
            // Map user-written variant name to our canonical key.
            if !matches!(outer_name.as_str(), "Some" | "None" | "Ok" | "Err") {
                continue;
            }
            let Some(entry) = inner_info
                .iter_mut()
                .find(|(v, _)| *v == outer_name.as_str())
                .map(|(_, s)| s)
            else {
                continue;
            };
            if entry.first_arm_span.is_none() {
                entry.first_arm_span = Some(arm.pattern.span);
            }
            // Look at the first field's pattern (Option/Result payloads are single-field).
            if let Some(f) = fields.first() {
                match &f.pattern.kind {
                    _ if is_catchall_pattern(&f.pattern.kind) => {
                        entry.has_catchall = true;
                    }
                    PatternKind::Constructor(inner_name, _) => {
                        entry.covered.insert(inner_name.clone());
                    }
                    _ => {}
                }
            } else {
                // Unit variant (None). No inner type to check.
                entry.has_catchall = true;
            }
        }
    }

    // For each variant whose inner is an ADT, check inner exhaustiveness.
    for (outer_v, inner_ty_name) in &inner_types {
        let Some(inner_name) = inner_ty_name else {
            continue;
        };
        let Some(expected_inner) = env.adt_variants(inner_name) else {
            continue;
        };
        let Some(state) = inner_info
            .iter()
            .find(|(v, _)| v == outer_v)
            .map(|(_, s)| s)
        else {
            continue;
        };
        if state.has_catchall {
            continue;
        }
        let missing: Vec<String> = expected_inner
            .iter()
            .filter(|v| !state.covered.contains(v.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let quoted: Vec<String> = missing.iter().map(|v| format!("`{v}`")).collect();
            let label_span = state.first_arm_span.unwrap_or(match_expr.span);
            report.add(
                Diagnostic::error(format!(
                    "non-exhaustive nested match in `{outer_v}(...)`: missing inner {}",
                    quoted.join(", ")
                ))
                .with_code("E0401")
                .with_label(Label::new(label_span, "nested patterns are not exhaustive"))
                .with_note(format!(
                    "inner type `{inner_name}` not covered: {}. Add nested arms or use `{outer_v}(_)`.",
                    quoted.join(", ")
                ))
                .with_note(
                    "nested exhaustiveness is checked at depth 1 only — deeper nesting is not analyzed.".to_string(),
                ),
            );
        }
    }
}

/// Return the ADT type name if `ty` resolves to a known user-defined ADT.
/// Only `Ty::Named` is matched; generic types (e.g. `Option<Option<_>>`) are
/// deliberately skipped to honor the depth-1 limitation.
fn adt_name_of(ty: &Ty, env: &TypeEnv) -> Option<String> {
    match ty {
        Ty::Named(name) if env.adt_variants(name).is_some() => Some(name.clone()),
        _ => None,
    }
}

/// §10.3: detect unreachable match arms.
/// Reports W0401 when:
/// - An arm follows a catch-all (wildcard or ident binding) — all subsequent arms are dead.
/// - A Constructor pattern repeats an earlier one with the *same head* AND both arms
///   have no distinguishing sub-patterns (zero fields, or all fields are catch-alls).
///   Example: `when Red / when Red` warns; `when Err(NotFound) / when Err(Forbidden)`
///   does NOT warn because nested patterns differ.
/// - Duplicate BoolLit / IntLit / StringLit literals always warn.
fn check_redundant_arms(match_expr: &MatchExpr, report: &mut Report) {
    let mut seen_heads: HashSet<String> = HashSet::new();
    let mut catchall_seen = false;
    let mut catchall_span: Option<Span> = None;

    for arm in &match_expr.arms {
        if catchall_seen {
            let note_span = catchall_span.unwrap_or(arm.pattern.span);
            report.add(
                Diagnostic::warning("unreachable match arm: preceded by a catch-all pattern")
                    .with_code("W0401")
                    .with_label(Label::new(arm.pattern.span, "this arm is unreachable"))
                    .with_note(format!(
                        "a wildcard or ident pattern at span {:?} already matches everything",
                        (note_span.start, note_span.end)
                    )),
            );
            continue;
        }

        match &arm.pattern.kind {
            PatternKind::Wildcard | PatternKind::Ident(_) => {
                catchall_seen = true;
                catchall_span = Some(arm.pattern.span);
            }
            PatternKind::Constructor(name, fields) => {
                // Only treat a repeated Constructor head as redundant when its sub-patterns
                // cannot distinguish it from the earlier arm (zero fields, or every field is
                // a catch-all). Otherwise, nested patterns may differentiate the arms —
                // e.g. `Err(NotFound)` vs `Err(Forbidden)` are distinct.
                let all_fields_catchall =
                    fields.iter().all(|f| is_catchall_pattern(&f.pattern.kind));
                let head_is_redundant_candidate = fields.is_empty() || all_fields_catchall;
                if head_is_redundant_candidate && !seen_heads.insert(name.clone()) {
                    report.add(
                        Diagnostic::warning(format!(
                            "unreachable match arm: variant `{name}` already matched"
                        ))
                        .with_code("W0401")
                        .with_label(Label::new(
                            arm.pattern.span,
                            "this arm duplicates a previous Constructor pattern",
                        ))
                        .with_note(
                            "the earlier arm already matches this variant with no distinguishing sub-pattern.",
                        ),
                    );
                }
                // Arms with distinguishing sub-patterns are not tracked — they could
                // legitimately differ from a later arm (pattern equality is not analyzed).
            }
            PatternKind::BoolLit(b) => {
                let name = if *b { "true" } else { "false" };
                if !seen_heads.insert(name.into()) {
                    report.add(
                        Diagnostic::warning(format!(
                            "unreachable match arm: `{name}` already matched"
                        ))
                        .with_code("W0401")
                        .with_label(Label::new(arm.pattern.span, "duplicate boolean literal")),
                    );
                }
            }
            PatternKind::IntLit(n) => {
                let key = format!("int:{n}");
                if !seen_heads.insert(key) {
                    report.add(
                        Diagnostic::warning(format!(
                            "unreachable match arm: integer `{n}` already matched"
                        ))
                        .with_code("W0401")
                        .with_label(Label::new(arm.pattern.span, "duplicate integer literal")),
                    );
                }
            }
            PatternKind::StringLit(s) => {
                let key = format!("str:{s}");
                if !seen_heads.insert(key) {
                    report.add(
                        Diagnostic::warning(format!(
                            "unreachable match arm: string `{s:?}` already matched"
                        ))
                        .with_code("W0401")
                        .with_label(Label::new(arm.pattern.span, "duplicate string literal")),
                    );
                }
            }
            _ => {}
        }
    }
}

fn stmt_type(stmt: &Stmt, env: &mut TypeEnv, report: &mut Report) -> Ty {
    match stmt {
        Stmt::Expr(s) => infer_expr(&s.expr, env, report),
        // `return`, `break`, and `continue` are divergent — control never falls through.
        Stmt::Return(_) | Stmt::Break(_) | Stmt::Continue(_) => Ty::Never,
        _ => Ty::Unit,
    }
}
