// tyra-resolve: Name resolution for the Tyra language.
// spec reference: §6.1 (top-level), §7.1 (bindings), §9 (functions), §13 (modules), §17 (prelude)

mod resolver;
mod scope;

pub use resolver::resolve;
pub use scope::{ScopeStack, Symbol};

use std::collections::HashMap;
use tyra_diagnostics::Span;

/// Maps a reference expression's span to the definition span of the resolved binding.
/// Built by the resolver and consumed by the LSP to implement go-to-definition.
/// `Prelude` symbols have no definition span and are not included.
pub type DefIndex = HashMap<Span, Span>;
