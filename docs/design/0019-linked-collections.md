# ADR 0019: 挿入順保証コレクション — `LinkedMap<K,V>` と `LinkedSet<T>`

- **Status**: Accepted
- **Date**: 2026-05-29
- **Spec sections affected**: §17.3.8 (LinkedMap), §17.3.9 (LinkedSet) (新設)

## Context

ADR 0016 で `Map<K,V>` および `Set<T>` を HAMT (Hash Array Mapped Trie) ベースの永続データ構造に置き換えた。
HAMT の iteration order は **ハッシュ値の DFS 順** であり、挿入順ではない。

この仕様は ADR 0016 §3 に明記されているが、実際の AI 生成コード (bench/ai-gen) では以下のパターンが頻出する：

```tyra
let config = {}
config = config.insert("host", "localhost")
config = config.insert("port", "8080")
config = config.insert("timeout", "30")

for k, v in config {
  println(k + "=" + v)   // AI は挿入順を暗黙仮定して記述しがち
}
```

Run 17 のベンチマーク分析では、このような iteration order 依存コードが **出力結果検証の失敗源** として観測可能な件数に達した。
AI は `Map<K,V>` の iteration が挿入順を保証すると暗黙仮定して書くため、
HAMT の hash 順と実際の出力が一致せず、テストが落ちる。

**要件の整理**:

1. 挿入順を保証するコレクション型が必要
2. HAMT の persistent semantics (structural sharing, path-copy) を侵食してはならない
3. `Map<K,V>` の ABI (`tyra_map_*` シンボル群) を変更してはならない
4. リテラル構文 `{}` の意味を変えてはならない (`{}` は `Map` のまま)

**`Map<K,V>` への順序保証追加は採択しない**。
理由: HAMT に挿入順インデックスを埋め込むと structural sharing が entries 側でも失われ、
メモリ効率が悪化する。また ABI 変更は既存バイナリとの非互換を生む。
別型として独立させることで HAMT の不変条件を保つ。

## Decision

**HAMT を侵食しない別型** として `LinkedMap<K,V>` / `LinkedSet<T>` を追加する。

### 1. 内部表現

```
LinkedMap<K,V>:
  entries: PersistentVector<(K,V)>   // 挿入順を保持するベクタ
  index:   Hamt<K, usize>            // key → entries index のルックアップ
```

- `entries` には `(K,V)` ペアを挿入順に追記する
- `index` は key を HAMT でハッシュし、対応する `entries` のインデックスを保持する
- `get(k)`: `index` で O(log n) ルックアップ → `entries[i]` で O(1) アクセス
- `iteration`: `entries` を先頭から走査 → 挿入順保証

`LinkedSet<T>` は `LinkedMap<T, ()>` の薄いラッパーとして実装する。

### 2. 操作の計算量契約

| 操作 | 計算量 | 備考 |
|------|--------|------|
| `insert(k, v)` | O(log n) | `entries` 末尾追記 + `index` path-copy |
| `get(k)` | O(log n) | `index` ルックアップのみ |
| `contains_key(k)` | O(log n) | `index` ルックアップのみ |
| `remove(k)` | O(n) | `entries` compact が必要 (後述) |
| `len()` | O(1) | `entries.len()` |
| `for k, v in lm` | O(n) | `entries` を順に走査 |

**`remove` が O(n) である理由**:

`remove(k)` は以下のステップが必要:

1. `index` から key のインデックス `i` を取得: O(log n)
2. `entries` からインデックス `i` の要素を除去し compact: O(n)
   - ベクタからの削除は後続要素のシフトが発生する
   - compact 後は `index` の全エントリを再マッピングする必要がある: O(n log n)

頻繁な remove が必要なユースケースには `Map<K,V>` を使用すること。
`LinkedMap<K,V>` は「挿入してから順番通りに読む」パターンに特化した型である。

### 3. ABI: 別シンボル群

`stdlib_linked_map.rs` / `stdlib_linked_set.rs` で新規に実装し、
シンボル名は既存の `tyra_map_*` / `tyra_set_*` とは完全に分離する:

```
tyra_linked_map_new       -> LinkedMap
tyra_linked_map_insert    -> LinkedMap x K x V -> LinkedMap
tyra_linked_map_remove    -> LinkedMap x K -> LinkedMap
tyra_linked_map_get       -> LinkedMap x K -> Option<V>
tyra_linked_map_contains  -> LinkedMap x K -> Bool
tyra_linked_map_len       -> LinkedMap -> Int
tyra_linked_map_iter_next -> (Iterator state)

tyra_linked_set_new
tyra_linked_set_insert
tyra_linked_set_remove
tyra_linked_set_contains
tyra_linked_set_len
tyra_linked_set_iter_next
```

既存の `tyra_map_*` / `tyra_set_*` シンボル群は一切変更しない。

### 4. API surface (Tyra 言語面)

```tyra
// LinkedMap<K,V>
LinkedMap.new() -> LinkedMap<K,V>
lm.insert(k: K, v: V) -> LinkedMap<K,V>   // 新規キーは末尾に追加; 既存キーは値を更新 (順序維持)
lm.remove(k: K) -> LinkedMap<K,V>         // O(n)
lm.get(k: K) -> Option<V>
lm.contains_key(k: K) -> Bool
lm.len() -> Int

for k, v in lm { ... }    // 挿入順を保証

// LinkedSet<T>
LinkedSet.new() -> LinkedSet<T>
ls.insert(v: T) -> LinkedSet<T>
ls.remove(v: T) -> LinkedSet<T>
ls.contains(v: T) -> Bool
ls.len() -> Int

for v in ls { ... }        // 挿入順を保証
```

**`insert` での既存キー更新時の順序**:
既存キーに対して `insert` を呼んだ場合、`entries` 内の順序は変わらず値のみ更新する
(Python の `dict` と同様の semantics)。

### 5. リテラル構文なし

`{}` は引き続き `Map<K,V>` のリテラルとして扱う。`LinkedMap` はリテラル構文を持たない:

```tyra
// Map リテラル (従来通り)
let m = {"a": 1, "b": 2}     // Map<String, Int>

// LinkedMap は明示的なコンストラクタを使う
let lm = LinkedMap.new()
lm = lm.insert("host", "localhost")
lm = lm.insert("port", "8080")

// v0.9+ 予定: LinkedMap.from([(k,v), ...]) ヘルパー
```

リテラル構文を持たないことで、コードを読んだときに
「この変数は挿入順が重要なコレクションである」という意図が明示される。

### 6. HAMT 非侵食

`stdlib_map.rs` / `stdlib_set.rs` を一切変更しない。
新規ファイル `runtime/src/stdlib_linked_map.rs` / `runtime/src/stdlib_linked_set.rs` を追加。

`Hamt<K, usize>` の実装は `stdlib_map.rs` 内の HAMT コードを
モジュール分割して再利用する (`hamt_core.rs` として切り出す)。
これにより実装の重複を避ける。

### 7. GC 安全性: double root

`LinkedMap` は Boehm GC 上で `PersistentVector` と `Hamt` の両方を保持する。
これらは互いに独立した GC root として扱う必要がある:

```
LinkedMap {
  entries: *mut PersistentVector,  // GC root 1
  index:   *mut HamtNode,          // GC root 2
}
```

`LinkedMap` 自体を `GC_malloc` で確保し、その内部ポインタ2本を
Boehm GC のスキャン対象に含める (interior pointer scanning は Boehm がデフォルトで行う)。

問題: `entries` ベクタと `index` HAMT が **互いに参照し合わない** ため、
どちらか一方しか生存している root がない状態で GC が走ると、
もう一方が誤回収される可能性がある。

対策: `LinkedMap` 構造体を単一の `GC_malloc` ブロックとして確保し、
`entries` / `index` ポインタを **同一ブロック内に格納** する。
Boehm GC はそのブロックを辿り、両ポインタを mark する。
これにより `LinkedMap` が生存している限り、両方の内部構造が保護される。

## Alternatives considered

### A. `Map<K,V>` に挿入順オプションを追加

HAMT ノードに挿入カウンタを持ち、iteration 時にソートする。

**却下**: HAMT の structural sharing 効果が entries 側で失われる。
O(n log n) のソートが毎回 iteration で必要になる。ABI 変更。

### B. OrderedMap を標準ライブラリのみで提供 (コンパイラ非対応)

`stdlib/ordered_map.tyra` として Tyra コードで実装し、FFI なしで運用。

**却下**: 再帰的なデータ型 (linked list 等) の実装には Tyra の型システム拡張が必要。
v0.8 の型システム範囲では実装が難しい。

### C. LinkedMap を採択 (本案)

別 ABI、別型、HAMT 非侵食。挿入順保証は `entries: PersistentVector` で担保。

**採択。**

## Consequences

**Positive**

- iteration order の保証により AI 生成コードの失敗パターンが減少する
- `Map<K,V>` の HAMT semantics (structural sharing, path-copy) を完全に保持
- 既存の `tyra_map_*` / `tyra_set_*` ABI への変更なし → バイナリ互換
- 「挿入順が重要」という意図がコード上で明示される (リテラルなし)

**Negative / accepted tradeoffs**

- `remove` が O(n): entries compact のコスト。頻繁な remove には `Map` を推奨
- リテラル構文なし: `LinkedMap.new().insert(...)` の冗長さは意図的
- HAMT の structural sharing は `entries` (PersistentVector) 側では失われる
  - `index` (Hamt) 側は structural sharing を維持する
  - よって頻繁な `insert` のコストは `Map` より若干高い
- 新規ファイル2本 + `hamt_core.rs` 切り出しが必要

**実装順序**

1. `runtime/src/hamt_core.rs` を `stdlib_map.rs` から切り出す
2. `runtime/src/stdlib_linked_map.rs` を実装 (PersistentVector + Hamt<K,usize>)
3. `runtime/src/stdlib_linked_set.rs` を実装 (LinkedMap<T,()> ラッパー)
4. コンパイラ side: `LinkedMap` / `LinkedSet` の型チェック、lowering 追加
5. E2E テスト: 挿入順 iteration、remove、GC 下での double root 安全性

## 実装ノート (v0.9.0)

`remove` の実装が **tombstone 方式**に変更された (`stdlib_linked_map.rs` 参照)。
元の設計 (§2 の O(n) compact) から以下に変わった:

| 条件 | 計算量 | 備考 |
|------|--------|------|
| key が存在しない | O(1) | entries/index を共有した新しいラッパーのみ確保 |
| key が存在する | O(entries_cap + idx_cap) | entry tombstone + index tombstone を記録 |

次の `insert` 呼び出し時に tombstone を compaction し、entries_cap ≈ live に戻す。
§2 の計算量表 (`remove: O(n)`) は key 存在時の最悪ケースとして近似的に有効だが、
key 不在の場合は O(1) になった点が変更の主要な改善である。

`LinkedSet<T>` は `tyra_linked_map_remove` に委譲するため、同じ計算量特性を持つ。
spec §11.1 および §11.2 にそれぞれ計算量の詳細が記載されている。
