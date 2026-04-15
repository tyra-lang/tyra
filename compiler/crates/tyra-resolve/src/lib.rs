// tyra-resolve: Name resolution for the Tyra language.
// spec reference: §6.1 (top-level), §7.1 (bindings), §9 (functions), §13 (modules), §17 (prelude)

mod resolver;
mod scope;

pub use resolver::resolve;
pub use scope::{ScopeStack, Symbol};
