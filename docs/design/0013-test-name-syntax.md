# ADR 0013: `test "name"` language syntax

- **Status**: Proposed
- **Date**: 2026-05-25
- **Spec sections affected**: 新規（テスト構文）; §（lexer/parser 文法）

## Context

現状、Tyra のテスト発見は `tyra-cli/src/main.rs` の `find_test_fns`（L1275）が
`test_*` という命名規約を使った関数を走査する方式のみ。
言語レベルにテスト概念は存在しない。

ADR-0008 が `test "name"` 言語構文を "将来 ADR として延期" と明記しており、
本 ADR がその正式決定である。

パニック期待（ADR 0012）のマーク手段として言語構文側に `panics` 修飾子を載せるため、
命名規約方式（`test_panics_*`）と対称な UX を提供する必要がある。

## Decision

### 1. 文法

```
test "<name>" [panics] <body> end
```

- `"<name>"`: 任意の文字列リテラル（テスト名）
- `[panics]`: 省略可能な修飾子キーワード（v0.6.0 では `panics` のみ）
- `<body>`: 通常の式列（`let` / `mut` / 関数呼び出し等）
- `end`: ブロック終端

将来の修飾子追加に備え、name の直後に 0 個以上の修飾子トークン列を許す形で実装する。
v0.6.0 時点の修飾子は `panics` のみ。

### 2. contextual keyword 実装方式

`test` と `panics` は **真の contextual keyword** として実装する:

- **lexer は変更しない**: `test` も `panics` も `Ident` として字句化を維持する
- **`parse_item` でのみ特別扱い**: `tyra-parser/src/lib.rs` の `parse_item`（L45 付近）で
  「item 位置に `Ident("test")` が来て、次のトークンが文字列リテラル」の組み合わせを peek し、
  条件が満たされた場合のみ `TestDef` 構文として解釈する
- name 解析後、`Ident("panics")` を peek したら修飾子として消費する
  （それ以外の `Ident` は body の開始とみなす）

この方式により:
- `test` という名前の変数 / 関数 / モジュール → 壊れない
- `panics` という名前の変数 / 関数 → 壊れない
- `test_anything` という命名規約の既存テスト関数 → 壊れない

### 3. AST と戻り値セマンティクス

`tyra-ast/src/types.rs` の `Item` enum に追加:

```rust
TestDef {
    name: String,
    expects_panic: bool,
    body: Vec<Stmt>,
}
```

**戻り値セマンティクス（既存ランナとの整合）**:

`tyra-cli/src/main.rs` の `synthesize_runner`（L1329）は各テスト関数を
`match {name}()` で呼び出し、`when Ok(_)` / `when Err(__runner_msg)` で分岐する。
つまり全テスト関数は `Result<Unit, String>` を返す必要がある。

`test "name" ... end` は MIR lowering 時に **`Result<Unit, String>` を返す隠し関数**へ変換する:
- `end` は暗黙の `Ok(())` を返す（body が最後まで到達した場合）
- body 中で `?` 演算子を使うと `Err(msg)` を早期 return できる（既存の `assert` との整合）
- `panics` 修飾子が付いたテストでは、body が **`panic()` 呼び出しや OOB 等の異常終了**を起こすことが期待される。
  runner が exit(101) + stderr センチネル `__TYRA_PANIC__` の組み合わせで判定する（ADR 0012）。
  **`assert.eq(...)` 等の assert 失敗は `panic` ではない**: `stdlib/assert.tyra` の各関数は
  `Result<Unit, String>` を返し、`?` で `Err(msg)` を早期 return するモデルである（`assert.tyra:3`）。
  assert 失敗は `Err` を返して runner が exit(1) で "not ok" とするため、`panics` 期待の pass 条件
  （exit 101 + センチネル）を満たさない。`panics` 修飾子は `panic()` や OOB 等の**プロセス異常終了**専用。

型チェッカは `TestDef::body` を `Result<Unit, String>` コンテキストで検査する。
body の最終式が `Unit` の場合は lowering で `Ok(())` でラップする。

将来修飾子が増えた場合は `expects_panic: bool` を `modifiers: Vec<TestModifier>` に
段階的に発展させる（v0.6.0 時点では bool で十分）。

### 4. 6 クレート横断の実装箇所

| クレート | 変更箇所 |
|---|---|
| `tyra-parser` | `parse_item` に `Ident("test") + StringLit` アーム追加 |
| `tyra-ast` | `Item::TestDef` 追加 |
| `tyra-resolve` | `TestDef` の body を通常スコープとして解決 |
| `tyra-types` | `TestDef` の body 型チェック（戻り型は `Result<Unit, String>`） |
| `tyra-mir` | `TestDef` を `Function` として lowering |
| `tyra-codegen-llvm` | `TestDef` 由来の関数を通常関数と同様に emit |
| `tyra-cli` | `find_test_fns` を `TestDef` 由来の関数にも対応させる |

### 5. 発見と命名規約の併存

`test "name"` ブロックと既存の `test_*` 関数命名規約は**併存**する。
`find_test_fns` は両方を発見し、同一の `TestMeta` 構造体に統合する。
`test "name" panics` ブロックの `expects_panic = true` は、
ADR 0012 の `test_panics_*` 命名規約と同じ判定パスに渡される。

### 6. 出力（TAP / JUnit）

`test "name"` ブロック由来のテストは、TAP / JUnit 出力において
文字列リテラルの `name` をテスト名として使用する。
命名規約由来テストは引き続き関数名をテスト名として使用する。

## Alternatives considered

### A. `test "name" do ... end`（Ruby 風）

`do` キーワードを body 開始の区切りとして使う。読みやすいが、
`do` を既存の `Ident` として使うコードへの影響を避けるには
同じ contextual keyword 実装が必要になり複雑さが増す。採用せず。

### B. アノテーション `@test` / `@panics`

`@test("name")` のようなアノテーション構文。
Tyra には現時点でアノテーション構文がなく、パーサの追加変更が大きい。却下。

### C. 予約語化

`test` を完全な予約語として lexer で `TokenKind::Test` にする。
`test` を変数 / 関数 / モジュール名として使う既存コードが壊れるため後方互換破壊。却下。

### D. `panics` を構文外（命名規約のみ）に切り出し

`test "name"` は panic 期待を持てず、panic 期待は `test_panics_*` のみで表現。
`test "name"` 構文と非対称な UX になり、名前付きテストでパニック期待を書く手段がなくなるため却下。

## Consequences

**Positive**

- 任意の文字列名でテストを書ける（スペース、日本語等を含む名前が使える）
- `panics` 修飾子により名前付きテストでもパニック期待が書ける
- lexer が変更されないため既存の `test` 識別子への後方互換が完全に保たれる
- 将来の修飾子（`timeout`, `skip` 等）追加が parse_item のみで完結する

**Negative / accepted tradeoffs**

- parse_item の分岐が増えるが、peek 2 トークン以内で決定できるため先読みのコストは小さい
- `TestDef` を 6 クレートに横断して伝播させる実装工数がある

**Implementation order**

1. `tyra-ast`: `Item::TestDef` 追加
2. `tyra-parser`: `parse_item` に contextual keyword 分岐追加（lexer 変更なし）
3. `tyra-resolve`: `TestDef` body の name 解決
4. `tyra-types`: `TestDef` body の型チェック
5. `tyra-mir`: `TestDef` → `Function` lowering
6. `tyra-codegen-llvm`: `TestDef` 関数 emit
7. `tyra-cli`: `find_test_fns` の拡張（`TestDef` 発見 + `expects_panic` 伝播）
8. 回帰テスト: `test` 識別子を使う既存コードが壊れないことを確認
