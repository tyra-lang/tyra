# ADR 0027: Character-level string API (Unicode scalar values) and list sorting

- **Status**: Accepted
- **Date**: 2026-06-12
- **Spec sections affected**: stdlib string/list sections (§11 area), §22 (partial un-deferral of "string 拡張 API" / "list 拡張 API")

## Context

The string module exposes only a byte-level character interface today:
`byte_at(_ s, _ i) -> Option<Int>` and `from_byte(_ b) -> String`
(`stdlib/string.ty:76,104`). The ai-gen sweep showed models hand-rolling
byte-level tokenizers and character transforms (rot13, toggle-case,
capitalize-words) on top of these. That works for ASCII but silently corrupts
multi-byte UTF-8 text — `byte_at` indexes bytes, and models routinely treat
the result as "a character".

Two latent naming problems compound this:

- `to_upper` / `to_lower` exist but are **ASCII-only**
  (`runtime/src/stdlib_string.rs:74-97`; non-ASCII passes through unchanged).
  The names promise Unicode case mapping they do not deliver. Maintainer
  direction (2026-06-12): names must encode their constraints.
- "Character" is ambiguous: UTF-8 byte, Unicode scalar value (USV), or
  grapheme cluster. Each gives different APIs and cost profiles. Tyra's
  single-interpretation principle requires picking one and saying so.

The list module follows an `Int`-default + `_str`-suffix convention
(`map`/`map_str`, `fold`/`fold_str` — `stdlib/list.ty`). Sorting is absent;
§22 defers `sort_by`.

## Decision

### 1. "Character" means Unicode scalar value (USV)

All new character-level APIs operate on USVs. Grapheme clusters are explicitly
**out of scope**: the spec documents that combining sequences and emoji ZWJ
sequences are split into their constituent USVs. Byte-level APIs
(`byte_at` / `from_byte`) remain unchanged for binary/ASCII work; the spec
cross-references the two levels.

### 2. New string functions (all USV-based)

```tyra
export fn chars(_ s: String) -> List<String>          # one element per USV
export fn char_at(_ s: String, _ index: Int) -> Option<String>   # USV index; O(n) — documented
export fn char_code(_ s: String) -> Option<Int>       # code point iff s is exactly one USV, else None
export fn from_char_code(_ code: Int) -> Option<String>  # None for surrogates / >0x10FFFF
```

- `char_at` is **O(n)** in the number of USVs (UTF-8 has no random access);
  the spec states this so users iterating positionally are steered to
  `chars()` instead.
- `char_code` returns `None` unless the string is exactly one USV — no
  "first character" guessing.
- `from_char_code` validates: surrogate range and out-of-range code points
  yield `None`, never a replacement character. No silent repair.

### 3. Rename ASCII case functions

`to_upper` → `to_ascii_upper`, `to_lower` → `to_ascii_lower`. Semantics
unchanged (ASCII letters mapped, everything else passes through — now stated
in the spec). The old names are **removed**, not aliased — one way to do
things. Breaking change, acceptable pre-1.0 (precedent: ADR-0025). Full
Unicode case mapping is out of scope (locale tailoring, Turkish-i problem);
if ever added it will be a separate, explicitly named API.

### 4. List sorting

Following the existing `Int`-default + `_str` convention:

```tyra
export fn sort(_ xs: List<Int>) -> List<Int>                  # ascending
export fn sort_str(_ xs: List<String>) -> List<String>        # ascending, byte order
```

- `sort_str` orders by UTF-8 byte sequence (same order `SortedMap` uses for
  String keys) — documented, no locale collation.
- `Float` lists are not sortable through this API (Float has no `Eq`,
  ADR-0002; total-order semantics for NaN would contradict it).
- Stable sort. Returns a new list (persistent collections convention).
- **The name `sort_by` is deliberately reserved** for the future generic
  `sort_by<T, K: Ord>` (an ability constraint on the key type). Shipping an
  `Int`-only `sort_by` now would force either a breaking re-typing or an
  awkward second name when stdlib generics land — both worse than waiting.
  v0.11 ships `sort` / `sort_str` only; key-based sorting stays in §22.

## Consequences

- Models (and humans) get a correct, predictable character API; byte-level
  hand-rolling becomes unnecessary for text work.
- **Breaking**: code using `to_upper`/`to_lower` must rename. Mechanical fix;
  release notes carry a migration one-liner.
- The `_str` convention spreads further (`sort`/`sort_str`); the eventual
  stdlib-generics cleanup grows slightly, accepted consciously.
- `chars()` materialises a `List<String>` (one small string per USV) — fine
  for the target workloads (CLI/text processing), documented as O(n) memory.
- New runtime intrinsics: `__string_chars`, `__string_char_at`,
  `__string_char_code`, `__string_from_char_code`, `__list_sort`,
  `__list_sort_str`.

## Implementation note (2026-06-13, as landed)

- Runtime: `tyra_string_chars` rides the same `ListStringRet` out-parameter
  protocol as split; `char_at`/`from_char_code` use a thread-local
  `char_errno` (mirroring `parse_int`); `char_code` uses the -1 sentinel
  (mirroring `byte_at`). Sorting lives in new `runtime/src/stdlib_list.rs`
  (`tyra_list_sort_int` — GC-atomic output array; `tyra_list_sort_str` —
  scanned array, byte-order comparison), passing lists by ref in AND out.
- Codegen: `__string_chars` reuses the split out-param emitter;
  `__list_sort[_str]` got a dedicated in+out by-ref emitter; scalar char
  intrinsics ride the SIMPLE table. Registered in resolver / checker / MIR
  intrinsic tables like their predecessors.
- Checker: `sort`/`sort_str` were added to the list structural table as
  strict shapes, so `list.sort(List<String>)` is E0308 (corpus
  `bad/E0308-sort-elem-mismatch.ty`) rather than the silent container
  fallback.
- `to_upper`/`to_lower` exports removed as decided; in-repo users
  (corpus 18, getting-started 04) migrated. The runtime symbols
  (`tyra_string_to_upper/lower`) are unchanged.
- End-to-end corpus case: `35-string-chars-list-sort.ty` (USV counting on
  multi-byte input, surrogate rejection, ASCII-only casing, both sorts).

## Alternatives considered

| Option | Rejected because |
|---|---|
| Grapheme clusters as "character" | Requires Unicode segmentation tables; version-dependent results contradict predictability |
| UTF-8 bytes as "character" (status quo) | Corrupts non-ASCII text; models already misuse it |
| A `Char` type | New scalar type ripples through the type system, abilities, codegen; single-USV `String` covers the use cases |
| Keep `to_upper` name, document ASCII limit | Name still promises what it doesn't do; LLMs will keep mis-predicting (maintainer direction 2026-06-12) |
| Alias `to_upper` → `to_ascii_upper` | Two ways to do the same thing; contradicts convention fixity |
| `char_at` returning `Option<Int>` (code point) | Mixing "index into string" and "to number" in one call; composition of `char_at` + `char_code` is clearer |
