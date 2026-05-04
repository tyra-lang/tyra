// tyra-resolve: Name resolution for the Tyra language.
// spec reference: §6.1 (top-level), §7.1 (bindings), §9 (functions), §13 (modules), §17 (prelude)

mod resolver;
mod scope;

pub use resolver::resolve;
pub use scope::{ScopeStack, Symbol};
pub use scope::{PRELUDE_CONSTRUCTORS, PRELUDE_FUNCTIONS, PRELUDE_TYPES};

use std::collections::HashMap;
use tyra_diagnostics::Span;

/// Maps a reference expression's span to the definition span of the resolved binding.
/// Built by the resolver and consumed by the LSP to implement go-to-definition.
/// `Prelude` symbols have no definition span and are not included.
pub type DefIndex = HashMap<Span, Span>;

/// LSP completion item kind for a user-defined name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Variable,
    TypeDef,
    Module,
}

/// Flat list of (name, kind) pairs for every user-defined binding in a file.
/// Collected by the resolver and consumed by the LSP completion handler.
/// Prelude names are NOT included here; the LSP adds them from PRELUDE_*.
///
/// TODO: convert `check_in_memory` return type to a named struct in a future cleanup.
pub type SymbolList = Vec<(String, CompletionKind)>;
