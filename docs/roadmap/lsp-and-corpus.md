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
| Static Corpus | Spec coverage report (`coverage.sh`) | ✅ Done (2026-05-05) |
| LSP | UTF-16 `Position.character` encoding | ✅ Done (2026-05-05) |
| LSP | Member-access completion (`module.<Tab>`, builtin methods) | ✅ Done (2026-05-05) |
| LSP | Find references (`textDocument/references`) | ✅ Done (2026-05-05) |
| LSP | Rename (`textDocument/rename`) | ✅ Done (2026-05-05) |
| LSP | Document symbols (`textDocument/documentSymbol`) | ✅ Done (2026-05-05) |
| LSP | Signature help (`textDocument/signatureHelp`) | ✅ Done (2026-05-05) |
| LSP | Semantic tokens (`textDocument/semanticTokens/full`) | ✅ Done (2026-05-05) |
| LSP | Code action / quick fix (`textDocument/codeAction`) | ✅ Done (2026-05-06) |
| LSP | Inlay hints (`textDocument/inlayHint`) | ✅ Done (2026-05-06) |
| LSP | Folding range (`textDocument/foldingRange`) | ✅ Done (2026-05-06) |
| LSP | Document highlight (`textDocument/documentHighlight`) | ✅ Done (2026-05-06) |
| LSP | Selection range (`textDocument/selectionRange`) | ✅ Done (2026-05-06) |
| LSP | Call hierarchy (`textDocument/prepareCallHierarchy` + `callHierarchy/{incoming,outgoing}Calls`) | ✅ Done (2026-05-06) |
| LSP | Linked editing range (`textDocument/linkedEditingRanges`) | ✅ Done (2026-05-06) |
| LSP | Type definition (`textDocument/typeDefinition`) | ✅ Done (2026-05-06) |

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
5. ✅ **Coverage report** — `coverage.sh` added (2026-05-05). Cross-references
   `SPEC_REF: §...` annotations against `docs/spec/ja/language-spec.md`
   headings; lists covered / uncovered / unknown sections. Current: 30/81 (37%).

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

- **UTF-16 column accuracy**: ✅ Fixed (2026-05-05). `SourceMap::offset_at_utf16`
  and `SourceMap::line_col_utf16` now count UTF-16 code units as required by
  LSP 3.17. All hover, go-to-definition, and diagnostic range positions are
  accurate for files containing non-ASCII identifiers, comments, or emoji.
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
- **Completion type-awareness**: when the cursor is positioned after
  `<ident>.`, completion switches to member-access mode.  Module members
  (`math.<Tab>`, `string.<Tab>`) are enumerated from mangled `module__member`
  symbols produced by `resolve_imports`.  Method completions for primitive types
  (`String`, `List<T>`) use a hardcoded table.  User-defined `impl` blocks and
  full `Ty`→method resolution are deferred.
- **Completion member-access best-effort**: when the file contains a dangling
  `.` (E0103), `TypeIndex` is empty so type-directed method completion degrades
  to module-symbol lookup only.
- **Completion intrinsics**: `__`-prefixed intrinsic names (e.g. `__fs_read_raw`)
  are intentionally excluded from completion; they are implementation details.
- **Rename scope**: same as references — only `ExprKind::Ident` use-spans are renamed.
  Field-access paths, pattern bindings, and type-name references are not renamed.
  No scope-collision check: renaming to a name already bound in the same scope produces
  invalid code (post-rename compiler errors surface this).  `prepareRename` is not
  implemented.  Cross-file rename is not supported.
- **Outline scope**: imports and locals inside function bodies are omitted from
  `textDocument/documentSymbol`.  Both `range` and `selectionRange` use the
  item-level span (the parser does not emit per-identifier name spans).
  `workspace/symbol` is not supported (single-file driver).
- **Inlay hints scope**: v1 は `let` / `mut` 文の型ヒント (`: T`) のみ。
  関数引数ラベル、戻り値ヒント、closure パラメタ型、for-loop binding、
  pattern destructuring の各 binding は未対応。型注釈付きの束縛
  (`let x: Int = 1`) はスキップ。`Ty::Var(_)` / `Ty::Error` は出さない。
  挿入位置は AST に identifier 専用 span が無いため
  `span.start + "let ".len() + name.len()` で計算 (ASCII 識別子前提)。
  `inlayHint/resolve` と workspace 設定 (`editor.inlayHints.*`) は未対応。
- **Type definition scope**: ユーザ定義 `value` / `data` / `type` のみ対応。
  プリミティブ (`Int`, `String` 等) と prelude generics
  (`Option<T>`, `Result<T,E>`, `List<T>`, `Map<K,V>`, `Set<T>`) は
  def span を持たないため `None` を返す。`Ty::Generic(name, args)` の
  場合 `args` には再帰せず、外側 `name` で解決する。trait 名解決および
  クロスファイル type definition は未対応。
- **Linked editing range scope**: references と同じ範囲 (`ExprKind::Ident` のみ)。
  def 側は `find_binding_name_span` で識別子トークンに narrow できる定義のみ対応
  (narrow に失敗した場合は `None` を返し linked editing を提供しない —
  LSP 仕様が要求する「全 range が同一長」を保証するため)。
  フィールドアクセス・パターン束縛・型名・prelude/builtin は未対応。
  `word_pattern` は省略しクライアントデフォルトに委ねる。
- **Call hierarchy scope**: 対象は `Item::FnDef` (top-level) と `trait` / `impl` 内 method のみ。
  top-level 文 (init script) からの呼び出しは caller 不明として `incomingCalls` から除外。
  メソッド呼び出し (`receiver.method()`) は callee が `FieldAccess` であり `def_index` に
  無いため未対応。prelude 関数 (`println` 等) への outgoing は def span を持たないため未対応。
  クロスファイル (workspace driver 不在) は未対応。
- **Selection range scope**: AST 階層 (`Item` → `Stmt` / `Expr` → 子) のみ。
  トークンレベル (識別子の文字単位、`(` `)` `,` 区切り) は未対応。
  `ElseBranch` は span を持たないため chain には現れない (内側 IfExpr/Stmts は出る)。
  Position 不正または AST に対応ノードが無い場合は全体 None を返す。
- **Document highlight scope**: references と同じ範囲 (`ExprKind::Ident` のみ)。
  kind は `TEXT` で統一 (read/write の区別は未対応)。クロスファイル・フィールド
  アクセス・型名・パターン束縛は未対応。prelude / builtin 名は定義 span を持たない
  ため未対応。
- **Folding range scope**: v1 は AST 由来のブロック構造のみ
  (`fn` / `data` / `type` / `trait` (+methods) / `impl` (+methods) /
  `value` / `if`-`else if`-`else` / `while` / `for` / `match` (含 arms) /
  `lambda`)。連続する `import` は 1 つの `Imports` 範囲にまとめる。
  単一行のアイテムはスキップ。`end_line` は `end` キーワード行の 1 行前に
  設定し、折りたたみ時に閉じトークンが見えるようにする。
  **コメント折り畳み (`FoldingRangeKind::Comment`)** は未対応 — lexer が
  コメントを破棄しているため。`foldingRange/resolve` および
  `collapsed_text` 動的指定は未対応。
- **Code action scope**: v1 は E0200 (undefined name) の typo 訂正のみ。
  候補は `state.symbols` + prelude 名から Levenshtein 距離 ≤ 2 で抽出し、
  上位 3 件を提案。E0309 戻り型ラッパ・未使用 import 削除・E0214 不要セミコロン等は未対応。
  `tyra-diagnostics::Diagnostic` に構造化サジェストフィールドが無いため、
  エラーメッセージを文字列パースして識別子名を抽出している。
  `CodeActionResolveProvider`・source actions・refactor.* 系は未対応。
- **Semantic tokens scope**: lexer + AST のハイブリッド方式。発行するトークン種別は
  KEYWORD / FUNCTION / TYPE / ENUM_MEMBER / PARAMETER / VARIABLE / STRING / NUMBER /
  COMMENT。識別子参照はトップレベル `fn` 定義・`let`/`mut` 束縛・`Param` を
  def_index 経由で分類し、prelude シンボルは名前マッチで DEFAULT_LIBRARY 修飾子を付与。
  フィールドアクセス (`x.field`) の field 名・引数ラベル・import path セグメント・
  パターン中の識別子は AST に専用 span がないため未分類。複数行にまたがるトークン
  (改行を含む raw string 等) は LSP 仕様で禁止のためスキップ。
  `semanticTokens/range` と incremental delta (`/full/delta`) は未対応。
- **Signature help scope**: ユーザ定義トップレベル `fn` と prelude のごく一部
  (`print` / `println` / `eprint` / `eprintln` / `panic`) のみ対応。
  メソッド呼び出し (`receiver.method(...)`)、トレイトメソッド、`impl` 内 self メソッド、
  ジェネリックパラメタの具体化はサポートしない。active call 検出はテキストベース走査
  (タイピング中に AST が壊れていても動作する) で、`#` 行コメント・`(* *)` ブロック
  コメント・`"..."` 文字列リテラル (`\` エスケープ込み) をスキップする。型表示は
  `TypeExpr` の構文形 (`type_expr_name`) で、`Ty` ベースのエイリアス展開や型推論
  結果は反映しない。
- **References scope**: only `ExprKind::Ident` references are tracked (same scope as
  go-to-definition).  Field-access / pattern bindings / type names are not surfaced.
  Cross-file references are not supported in v0.1.
- **Go-to-definition scope**: only `ExprKind::Ident` references are tracked.
  Field-access paths (`module.member`) and prelude/builtin names (no definition
  span) are not supported in v0.1.
- **Definition span granularity**: definition spans cover the entire statement
  (`let x: T = …` line) rather than just the name token, because the AST does
  not carry a dedicated span for the binding identifier. A future AST extension
  can improve jump precision.
