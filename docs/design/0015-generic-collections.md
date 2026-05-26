# ADR 0015: Generic collections — `Map<K,V>` and `Set<T>`

- **Status**: Accepted
- **Date**: 2026-05-26
- **Spec sections affected**: §17.3.6 (map), 新規 §17.3.x (set), §8.4（ability constraints）

## Context

現状の `Map` は `Map<String,Int>` にハードコードされている（`MapLit` AST ノード、
`tyra-mir/src/lower/expr.rs` L958-971 で `__map_new_string_int` 等に直接 lowering、
ランタイムは連結リスト）。

`Set` は `tyra-resolve/src/scope.rs` L254 で型名として予約されているのみで、
パーサ / ランタイム / intrinsic の実装はゼロ。

**能力制約 (`Eq`/`Hash`) の現状**:
`Eq` / `Hash` / `Ord` / `Debug` は既に compiler-known ability として実装済みかつ auto-derived
（spec §8.4 ja L325「v0.1 の ability は Eq, Hash, Ord, Debug の 4 つ」、
`tyra-types/src/checker.rs` L767 prelude で「auto-derived」明記、
`conjunct_field_abilities` L779-785 の `candidates` に 4 つとも存在）。

**重要な区別**:
既存の `Eq` / `Hash` は **checker 側の型述語**（コンパイル時の充足判定）にすぎず、
任意の box 値を引数に取りランタイムで呼出可能な eq / hash 関数を生成する基盤は存在しない。
本 ADR の核は「その**ランタイム eq / hash 関数生成器を codegen に新規追加**」することであり、
ability の定義・導出規則の新設ではない（型述語は再利用、ランタイム関数生成は新規 first-class 工数）。

## Decision

### 1. Map<K,V> の完全一般化

任意の `K: Eq + Hash` / 任意の `V` をサポートする完全一般化を採用する
（surface のみ generic でランタイム限定の暫定段階は取らない）。

既存の `Map<String,Int>` ハードコードは廃止し、以下を変更する:
- `tyra-mir/src/lower/expr.rs` の MapLit lowering を (K,V) パラメトリックに変更
- `tyra-mir/src/monomorphize.rs` / `lower/types.rs` / `adt.rs` で型処理を汎用化
- `tyra-resolve/src/resolver.rs`、`tyra-types/src/checker.rs` での MapLit 型付け

### 2. 空リテラル `{}` の型付け（双方向推論）

既存の「`{}` を `Map<String,Int>` に寄せる暗黙規則」を**廃止**する。

代わりに使用位置の期待型から K / V を**双方向推論**する:
- `let m: Map<K,V> = {}` → 期待型から K,V を推論 → OK
- 関数引数位置 / 戻り型から期待型が流入するケース → OK
- 期待型が得られない文脈での裸の `{}` → 型エラー（明示注釈または `map.new()` を促す診断）
- 非空リテラル `{k: v, ...}` は要素から従来通り推論（変更なし）

`tyra-types/src/checker.rs` の `MapLit` 型付けに期待型伝播（双方向）を実装する。

### 3. Set<T> の完全一般化（グリーンフィールド）

任意の `T: Eq + Hash` をサポートする first-class ジェネリックとして追加する。
型システム・パーサ・ランタイム・intrinsic を全部新規に構築する。

構築方法: `set` モジュール + intrinsic（リテラル構文は回避してパーサ変更を最小化）:
- `set.new() -> Set<T>`（構築; 空集合。`T` を推論できる文脈では型注釈省略可）
- `s.insert(x: T) -> Set<T>`（メソッド形式; 非破壊的。重複は冪等）
- `s.contains(x: T) -> Bool`（メソッド形式）
- `s.len() -> Int`（メソッド形式）

`stdlib/set.tyra`（新規）+ 5 層配線（resolve / types / mir / codegen / type_scan）。

### 4. ランタイム実現方式（box 化単一汎用ハッシュ表 + fn ポインタ）

任意の (K,V) / T を扱うため、以下の方式に確定する:

- **ランタイム（Rust、1 回コンパイル）**: 単一の汎用ハッシュ表実装。
  キー / 値を box 化したポインタで保持
- **compiler（各具体型ごと）**: `Eq`（等価比較）/ `Hash`（ハッシュ）関数を生成し、
  その関数ポインタをハッシュ表構築時に渡す（vtable 的 dispatch）

per-(K,V) / per-T のランタイム単相化生成（text IR で冗長・コード膨張）は**却下**。

### 5. erased-value ABI（コンパイラ ↔ ランタイム境界の実装真実源）

**消去表現**:
- 各キー / 値は **`i8*`（不透明ポインタ）= GC ボックスへのポインタ**で統一して格納
- ボックスは `GC_malloc` で確保し、値の標準 in-memory 表現をそのまま格納:
  - `Int` / `Bool`: 8 byte スロット
  - `String`: 既存 String 表現へのポインタ
  - `value` struct / ADT: その標準レイアウト
- スカラとポインタを 1 スロットに混在させない（統一 `i8*`）ことで
  Boehm GC が全要素を確実に走査できる

**所有 / 解放**:
- ボックス・バケット配列・テーブル本体すべて Boehm GC 所有
- **手動 free なし**（ADR 0007 整合）

**compiler 生成関数シグネチャ**:
```llvm
define i1 @tyra_eq_<ty>(i8* %a, i8* %b) { ... }
define i64 @tyra_hash_<ty>(i8* %a) { ... }
```
本体で `i8*` を当該型のボックス型へ cast し、構造に従って比較 / ハッシュ
（struct / ADT は再帰。プリミティブ Int / Bool / String から実装を始める）。

**挿入 / 取得の再構成**:
- `insert(k, v)`: k / v のボックス `i8*` を渡す
- `get(k)`: ランタイムが格納済み値ボックス `i8*`（未存在は null センチネル）を返し、
  codegen が呼出側で具体型へ cast + load して値を復元
- `contains`: bool を直接返す
- `len`: Int を直接返す

### 6. ability constraint の境界チェック（既存機構の再利用）

`K: Eq + Hash`（Map） / `T: Eq + Hash`（Set）の境界チェックには
既存の checker-side ability 述語をそのまま利用する:
- `Float` や `mut` フィールドを持つ型は `Hash` 不可（ADR-0002 整合の既存規則）
- この判定は新設せず、`conjunct_field_abilities`（L779-785）の既存ロジックを呼ぶ

## Alternatives considered

### A. `{}` を Map<String,Int> 既定のまま維持

暗黙規則が残り続ける。一般化後に `Map<Int,Bool>` の空マップが書けなくなる。却下。

### B. Set リテラル `{1, 2, 3}` 構文

`MapLit` との衝突をパーサで解消する必要がある。複雑化のため却下。
`set.new()` + `s.insert(x)` メソッド形式で代替。

### C. Map は String,Int 維持し V のみ任意

中途半端な中間状態。将来また一般化が必要になる。却下。

### D. surface generic + runtime 限定（K∈{String,Int}）の暫定段階

ユーザー判断で却下。完全一般化を選択。

### E. per-(K,V) / per-T のランタイム単相化生成

text IR で冗長・コード膨張。box 化単一表 + fn ポインタを採用。

## Consequences

**Positive**

- `Map<Int, String>` / `Map<String, Bool>` / `Map<Point, Int>`（ユーザー定義 value 型）
  等、任意の (K,V) が使える
- `Set<Int>` / `Set<String>` / `Set<Point>` 等、任意の T が使える
- ランタイムは 1 回コンパイルで固定（per-type 爆発なし）
- Boehm GC が全要素を保守的に走査できる（統一 `i8*`）

**Negative / accepted tradeoffs**

- **実装難度が高い**: `i8*` erased ABI + compiler-emitted `tyra_eq_<ty>` / `tyra_hash_<ty>` 生成
  （プリミティブ / `value` struct / ADT 再帰）は最大級の実装リスク
- ランタイムアクセスに cast + load が必要（直接アクセスより遅い）
- 最小縦切り戦略（`Map<Int,Int>` から始めて struct/ADT へ広げる）で実装する

**Implementation order**

1. `runtime/src/stdlib_map.rs` を box 化 + fn ポインタ型 API に再構築
2. compiler 生成 eq / hash 関数（プリミティブ: Int / Bool / String）
3. `Map<Int,Int>` 最小縦切りで end-to-end 検証
4. compiler 生成 eq / hash 関数（`value` struct / ADT の再帰）
5. MapLit lowering の (K,V) パラメトリック化 + 双方向 `{}` 推論
6. `runtime/src/stdlib_set.rs` 新規（Map と機構を共有）
7. Set 5 層配線
8. 検証: `Map<Int,String>` / `Map<Point,Int>` / `Set<Int>` / `Set<Point>` の E2E テスト
