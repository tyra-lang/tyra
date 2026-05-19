use tyra_diagnostics::{SourceId, Span};
use tyra_driver::DefIndex;

use crate::DocState;

/// Find the definition span that the cursor at `offset` refers to.
///
/// Two cases:
/// 1. Cursor is inside a *use-span* in `def_index` → return the associated def-span.
/// 2. Cursor is inside a *def-span* (the definition site itself) → return that def-span.
/// 3. Neither → return `None`.
///
/// When multiple spans contain the offset, the smallest one wins (most specific).
pub(crate) fn find_def_span_at_cursor(state: &DocState, offset: u32) -> Option<Span> {
    // Case 1: cursor inside a use-span.
    let use_match = state
        .def_index
        .iter()
        .filter(|(s, _)| s.source == state.source_id && s.start <= offset && offset < s.end)
        .min_by_key(|(s, _)| s.end - s.start);
    if let Some((_, def_span)) = use_match {
        return Some(*def_span);
    }

    // Case 2: cursor inside a def-span (definition site).
    state
        .def_index
        .values()
        .filter(|d| d.source == state.source_id && d.start <= offset && offset < d.end)
        .min_by_key(|d| d.end - d.start)
        .copied()
}

/// Return all use-spans in `def_index` whose definition is `def_span`,
/// restricted to spans in `source_id`.
///
/// Filtering by source is a defensive guard against future cross-file import
/// resolution: without it, use-spans from other files would be returned with
/// the wrong URI by the handler.
///
/// O(n) over def_index entries; acceptable for single-file v0.1 scope.
pub(crate) fn find_uses_for_def(
    def_index: &DefIndex,
    def_span: Span,
    source_id: SourceId,
) -> Vec<Span> {
    def_index
        .iter()
        .filter(|(use_span, d)| use_span.source == source_id && **d == def_span)
        .map(|(use_span, _)| *use_span)
        .collect()
}
