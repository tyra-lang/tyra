// tyra-diagnostics: Error reporting infrastructure for the Tyra compiler.
//
// All user-facing errors go through this crate. Never eprintln! directly.
// Error format: error[E0042]: message at file:line:col
//
// spec reference: AGENTS.md "Diagnostics" section

mod diagnostic;
mod report;
mod source;
mod span;

pub use diagnostic::{Diagnostic, Label, Level};
pub use report::Report;
pub use source::{SourceId, SourceMap};
pub use span::Span;
