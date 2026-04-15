// tyra-ast: AST node definitions for the Tyra language.
// spec reference: §6-§14 (all syntax constructs)
//
// The AST is consumed by both the parser (construction) and the type checker (analysis).
// Every node carries a Span for error reporting.

pub use tyra_diagnostics::Span;

mod types;

pub use types::*;
