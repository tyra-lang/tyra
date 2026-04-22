// Lexical scope management for name resolution.
// spec reference: §7.1 (bindings), §9 (functions), §13 (modules)

use std::collections::HashMap;

use tyra_diagnostics::Span;

/// What a name resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum Symbol {
    /// Local variable binding (let or mut)
    Local { mutable: bool, span: Span },
    /// Function parameter
    Param { span: Span },
    /// Function definition
    Function { span: Span },
    /// Type definition (value, data, type alias, ADT)
    TypeDef { span: Span },
    /// Trait definition
    TraitDef { span: Span },
    /// Imported name
    Import { path: Vec<String>, span: Span },
    /// Prelude builtin (print, println, panic, Some, None, Ok, Err, etc.)
    Prelude { name: String },
}

/// A single lexical scope containing name bindings.
#[derive(Debug)]
struct Scope {
    bindings: HashMap<String, Symbol>,
}

impl Scope {
    fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }
}

/// Stack of lexical scopes for name resolution.
#[derive(Debug)]
pub struct ScopeStack {
    scopes: Vec<Scope>,
}

impl ScopeStack {
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::new()],
        }
    }

    /// Create with prelude names already defined.
    pub fn with_prelude() -> Self {
        let mut stack = Self::new();
        for name in PRELUDE_FUNCTIONS {
            stack.define(
                name.to_string(),
                Symbol::Prelude {
                    name: name.to_string(),
                },
            );
        }
        for name in PRELUDE_CONSTRUCTORS {
            stack.define(
                name.to_string(),
                Symbol::Prelude {
                    name: name.to_string(),
                },
            );
        }
        for name in PRELUDE_TYPES {
            stack.define(
                name.to_string(),
                Symbol::Prelude {
                    name: name.to_string(),
                },
            );
        }
        stack
    }

    /// Push a new scope (entering a function body, block, etc.)
    pub fn push(&mut self) {
        self.scopes.push(Scope::new());
    }

    /// Pop the current scope (leaving a function body, block, etc.)
    pub fn pop(&mut self) {
        debug_assert!(self.scopes.len() > 1, "cannot pop the global scope");
        self.scopes.pop();
    }

    /// Define a name in the current scope.
    /// Returns the previous definition if the name was already defined in this scope.
    pub fn define(&mut self, name: String, symbol: Symbol) -> Option<Symbol> {
        self.scopes
            .last_mut()
            .unwrap()
            .bindings
            .insert(name, symbol)
    }

    /// Look up a name, searching from innermost to outermost scope.
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for scope in self.scopes.iter().rev() {
            if let Some(sym) = scope.bindings.get(name) {
                return Some(sym);
            }
        }
        None
    }

    /// Check if a name is defined in the current (innermost) scope only.
    pub fn defined_in_current(&self, name: &str) -> bool {
        self.scopes.last().unwrap().bindings.contains_key(name)
    }

    /// Current nesting depth (0 = global scope).
    pub fn depth(&self) -> usize {
        self.scopes.len() - 1
    }
}

impl Default for ScopeStack {
    fn default() -> Self {
        Self::new()
    }
}

// -- Prelude definitions (§17.1) --

/// Prelude functions: auto-imported, no `import` needed.
const PRELUDE_FUNCTIONS: &[&str] = &[
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "parse",
    // M10 phase 1: intrinsic stdlib backing. Not intended for user code,
    // but exposed at the prelude so `stdlib/fs.tyra` can call them without
    // needing an `import` (which would create a circular dependency).
    "__fs_read_raw",
    "__fs_errno",
    "__fs_errmsg",
    "__fs_write_raw",
    "__fs_exists",
    // M11 phase 1: http client backing. See runtime/src/stdlib_http.rs.
    "__http_get",
    "__http_status",
    "__http_body",
    "__http_errno",
    "__http_errmsg",
    // M11 phase 2: http server backing. See runtime/src/stdlib_http_server.rs.
    "__http_server_new",
    "__http_server_route",
    "__http_server_listen",
    // M10 phase 2: json stdlib backing. See runtime/src/stdlib_json.rs.
    "__json_parse",
    "__json_err_msg",
    "__json_err_line",
    "__json_err_col",
    "__json_kind",
    "__json_is_string",
    "__json_is_int",
    "__json_is_bool",
    "__json_str",
    "__json_int",
    "__json_bool",
    "__json_get",
    "__json_at",
    // stdin backing. See runtime/src/stdlib_io.rs.
    "__io_read_line",
    "__io_read_to_end",
    "__io_eof",
    // §17.3.4: string stdlib backing. See runtime/src/stdlib_string.rs.
    "__string_len",
    "__string_is_empty",
    "__string_trim",
    "__string_to_upper",
    "__string_to_lower",
    "__string_contains",
    "__string_starts_with",
    "__string_ends_with",
    "__string_parse_int",
    "__string_parse_errno",
];

/// Prelude ADT constructors: unqualified access to Option/Result variants.
const PRELUDE_CONSTRUCTORS: &[&str] = &["Some", "None", "Ok", "Err"];

/// Prelude types and abilities.
const PRELUDE_TYPES: &[&str] = &[
    // Primitive types (§7.2)
    "Int",
    "Float",
    "Bool",
    "String",
    "Rune",
    "Bytes",
    "Unit",
    "Never",
    // Standard types
    "Option",
    "Result",
    "List",
    "Map",
    "Set",
    "Task",
    // Standard traits (§17.1)
    "Into",
    "Stringable",
    // Abilities (§8.4)
    "Eq",
    "Hash",
    "Ord",
    "Debug",
];

#[cfg(test)]
mod tests {
    use super::*;
    use tyra_diagnostics::SourceMap;

    fn test_span() -> Span {
        let mut sources = SourceMap::new();
        let id = sources.add("test.tyra".into(), "".into());
        Span::new(id, 0, 0)
    }

    #[test]
    fn basic_lookup() {
        let mut scopes = ScopeStack::new();
        let span = test_span();
        scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        assert!(scopes.lookup("x").is_some());
        assert!(scopes.lookup("y").is_none());
    }

    #[test]
    fn nested_scopes() {
        let mut scopes = ScopeStack::new();
        let span = test_span();
        scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        scopes.push();
        scopes.define(
            "y".into(),
            Symbol::Local {
                mutable: true,
                span,
            },
        );
        // Both visible from inner scope
        assert!(scopes.lookup("x").is_some());
        assert!(scopes.lookup("y").is_some());
        scopes.pop();
        // y no longer visible
        assert!(scopes.lookup("x").is_some());
        assert!(scopes.lookup("y").is_none());
    }

    #[test]
    fn shadowing() {
        let mut scopes = ScopeStack::new();
        let span = test_span();
        scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        scopes.push();
        scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: true,
                span,
            },
        );
        // Inner x shadows outer x
        if let Some(Symbol::Local { mutable, .. }) = scopes.lookup("x") {
            assert!(*mutable);
        } else {
            panic!("expected Local");
        }
        scopes.pop();
        // Back to outer x
        if let Some(Symbol::Local { mutable, .. }) = scopes.lookup("x") {
            assert!(!*mutable);
        } else {
            panic!("expected Local");
        }
    }

    #[test]
    fn prelude_available() {
        let scopes = ScopeStack::with_prelude();
        assert!(scopes.lookup("print").is_some());
        assert!(scopes.lookup("println").is_some());
        assert!(scopes.lookup("panic").is_some());
        assert!(scopes.lookup("Some").is_some());
        assert!(scopes.lookup("None").is_some());
        assert!(scopes.lookup("Ok").is_some());
        assert!(scopes.lookup("Err").is_some());
        assert!(scopes.lookup("Int").is_some());
        assert!(scopes.lookup("Option").is_some());
        assert!(scopes.lookup("Result").is_some());
    }

    #[test]
    fn defined_in_current() {
        let mut scopes = ScopeStack::new();
        let span = test_span();
        scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        assert!(scopes.defined_in_current("x"));
        scopes.push();
        assert!(!scopes.defined_in_current("x"));
        scopes.define(
            "y".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        assert!(scopes.defined_in_current("y"));
    }

    #[test]
    fn duplicate_in_same_scope() {
        let mut scopes = ScopeStack::new();
        let span = test_span();
        let first = scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: false,
                span,
            },
        );
        assert!(first.is_none());
        let second = scopes.define(
            "x".into(),
            Symbol::Local {
                mutable: true,
                span,
            },
        );
        assert!(second.is_some()); // returns previous definition
    }

    #[test]
    fn scope_depth() {
        let mut scopes = ScopeStack::new();
        assert_eq!(scopes.depth(), 0);
        scopes.push();
        assert_eq!(scopes.depth(), 1);
        scopes.push();
        assert_eq!(scopes.depth(), 2);
        scopes.pop();
        assert_eq!(scopes.depth(), 1);
    }
}
