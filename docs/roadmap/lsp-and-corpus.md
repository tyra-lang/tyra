# Roadmap: LSP and Static Corpus

## Status

| Track | Phase | Status |
|-------|-------|--------|
| Static Corpus | Initial files (01–10) | ✅ Done |
| LSP | Scaffold / `initialize` stub | ✅ Done |
| LSP | Diagnostics (syntax errors on save) | ✅ Done |
| LSP | Hover (type of identifier) | ✅ Done (2026-05-04) |
| LSP | VS Code extension scaffold | ✅ Done (2026-05-04) |
| LSP | Go to definition | ✅ Done (2026-05-04) |
| LSP | Completion | ✅ Done (2026-05-04) |
| Static Corpus | Break / control-flow programs | ✅ Done (2026-05-05) |
| Static Corpus | Negative corpus (`bad/` directory) | ✅ Done (2026-05-05) |
| Static Corpus | CI integration (`check.sh` in workflow) | ✅ Done (2026-05-05) |

---

## Static Corpus

### Goal

Maintain a growing set of **human-written, compiler-verified** Tyra programs
separate from AI-generated benchmark output.  Changes to the compiler should
never silently break these programs.

### Location

`bench/static-corpus/`

### Short-term tasks (next 1–2 cycles)

1. ✅ **CI hook** — `.github/workflows/static-corpus.yml` added (2026-05-05).
   Triggers on push and PR to main; builds `tyra-cli`, then runs `check.sh`.
2. ✅ **Extend with break/continue** — `11-break-loop.tyra` added (2026-05-05).
   Exercises `break` inside `while` and `for`; `tyra check` subcommand also added.
3. ✅ **Negative corpus** — `bad/` subdirectory added (2026-05-05), expanded to
   9 programs (E0104 / E0200 / E0206 / E0214 / E0301 / E0302 / E0305 / E0309 /
   E0400). `check.sh` extracts the expected code from the filename and verifies
   that stderr contains `error[Exxxx]` and exit is non-zero.

### Mid-term tasks

4. **Auto-generate from prompt suite** — after each AI benchmark run that
   achieves `any_pass = 100`, promote passing AI programs to the corpus
   (after human review).
5. **Coverage report** — annotate which spec sections (§) are covered by at
   least one corpus file; flag uncovered sections.

---

## Language Server Protocol (LSP)

### Goal

Provide IDE support for Tyra in VS Code (and any LSP-compatible editor) to
improve developer ergonomics and accelerate language adoption.

### Location

`tools/lsp/tyra-lsp/`

### Architecture

```
Editor (VS Code)
    ↕ LSP JSON-RPC (stdin/stdout)
tyra-lsp binary
    ├── tower-lsp  (transport / dispatch)
    ├── tyra-driver  (compilation pipeline)
    │     ├── tyra-lexer
    │     ├── tyra-parser
    │     ├── tyra-types   (type checker — produces Ty per binding)
    │     └── tyra-diagnostics
    └── document store  (open files, incremental parse)
```

`tyra-lsp` will reuse the existing `tyra-driver` pipeline and type-checker
results.  The document store maintains the in-memory version of each open file
and re-runs the pipeline on every `textDocument/didChange` notification.

### Short-term tasks (next 1–2 cycles)

1. **Diagnostics on save** — wire `tyra-driver` into
   `textDocument/didOpen` + `textDocument/didChange` handlers; publish
   `textDocument/publishDiagnostics` with compiler errors.
2. **VS Code extension scaffold** — create `tools/lsp/vscode-tyra/` with a
   minimal `package.json` that spawns `tyra-lsp` and registers `.tyra` file
   associations.
3. **Document store** — a `HashMap<Url, String>` that caches the latest source
   text so the server can re-parse on change without hitting disk.

### Mid-term tasks

4. **Hover** — ✅ Done (2026-05-04). On `textDocument/hover`, resolve the
   hovered identifier's `Ty` from the type environment and render a
   human-readable string.
5. **Go to definition** — ✅ Done (2026-05-04). Uses `DefIndex` (reference
   span → definition span) built by `tyra-resolve` to jump to the binding site.
6. **Completion** — ✅ Done (2026-05-04). File-wide user-defined names
   (fn / let / mut / param / for-binding / type / import alias) + prelude
   functions / constructors / types + language keywords. Position-independent
   (all names in the file are offered regardless of cursor scope).

### Dependencies

- `tower-lsp 0.20` (already in `tools/lsp/tyra-lsp/Cargo.toml`)
- `tokio` full (async runtime)
- `tyra-driver` (reuse compilation pipeline; to be added when diagnostics land)

### Known constraints

- **UTF-16 column accuracy**: `SourceMap::offset_at` treats LSP `Position.character`
  as a UTF-8 byte column. This is correct for ASCII identifiers. Non-ASCII content
  inside string literals is not a hover target, so the practical impact is minimal.
  A proper UTF-16 → byte conversion should be added before shipping the extension
  to users who write non-ASCII comments or identifiers.
- Incremental compilation is not planned for v0.1 scope; full re-parse on each
  edit is acceptable for files < 1 000 lines.
- Type spans for `let`/`mut` statement names are recorded at the statement-level
  span because the AST does not carry a dedicated span for the binding name token.
  Hovering anywhere within the `let x: T = …` line shows the binding type.
- `check_in_memory` returns an empty `TypeIndex` and `DefIndex` when an early
  pipeline stage (parse, resolve) fails; hover and go-to-definition show nothing
  in error files until the error is fixed.
- **Completion scope**: position-independent — every name defined anywhere in
  the file is offered as a candidate, regardless of cursor scope. A local
  binding from a sibling function may appear while editing another function.
  VS Code prefix-matching filters most noise; position-aware scoping is
  deferred to a future iteration.
- **Completion type-awareness**: method completions for `xs.<Tab>` or
  `math.<Tab>` (module member access) are not yet supported in v0.1.
- **Completion intrinsics**: `__`-prefixed intrinsic names (e.g. `__fs_read_raw`)
  are intentionally excluded from completion; they are implementation details.
- **Go-to-definition scope**: only `ExprKind::Ident` references are tracked.
  Field-access paths (`module.member`) and prelude/builtin names (no definition
  span) are not supported in v0.1.
- **Definition span granularity**: definition spans cover the entire statement
  (`let x: T = …` line) rather than just the name token, because the AST does
  not carry a dedicated span for the binding identifier. A future AST extension
  can improve jump precision.
