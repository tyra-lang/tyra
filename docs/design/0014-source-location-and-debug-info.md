# ADR 0014: Source-location threading and debug info (DWARF)

- **Status**: Proposed
- **Date**: 2026-05-25
- **Spec sections affected**: なし（実装内部）; ツール: デバッガ / coverage

## Context

MIR lowering で AST の `Span` が完全に破棄される。
`tyra-mir/src/ir.rs` の `Instruction` / `Function` / `Program` に位置情報は存在せず、
`tyra-codegen-llvm/src/codegen.rs` も行情報を持たない。

この欠落により以下が不可能:

- **パニック診断**: `panic("msg")` 時に発生行を出力できない（現状は `puts(msg)+abort` のみ）
- **coverage**: どの行が実行されたかを IR 計装で追跡できない
- **DAP デバッガ**: DWARF 行テーブル / `DILocalVariable` を生成できない

本 ADR は三者の共通基盤となる Span 再配線と、DWARF テキスト IR 生成方針を決定する。

## Decision

### 1. MIR への位置情報導入

`tyra-mir/src/ir.rs` に `SourceLoc` 型を追加:

```rust
pub struct SourceLoc {
    pub file_id: u32,
    pub line: u32,
    pub col: u32,
}
```

`Instruction` を `SourceLoc` で包んだ `Stmt` 型を導入し、`Function::body` の型を
`Vec<Instruction>` から `Vec<Stmt>` に変更する:

```rust
pub struct Stmt {
    pub loc: SourceLoc,
    pub instr: Instruction,
}
```

`Function` に source file 情報、`Program` にファイル一覧を追加する。

### 2. lowering での Span 伝播

`compiler/crates/tyra-mir/src/lower/*.rs`（adt / call / expr / match_lower / method / mod / propagate / types）が
現在捨てている AST / monomorphize 由来の `span` を `SourceLoc` として MIR へ伝播させる。

`tyra-mir/src/monomorphize.rs` は `Stmt` / `TypeExpr` に span を保持しているため、
この情報を lowering パスに引き継ぐことで取得できる。

### 3. 変数メタ情報の保持（locals 表示の前提）

行情報だけでは locals を DWARF で表現できない。
ローカル変数 / パラメータについて「名前・型・スコープ・alloca スロット（格納場所）」を
MIR → codegen で保持できるよう拡張する:

```rust
pub struct LocalMeta {
    pub name: String,
    pub ty: Ty,
    pub scope: ScopeId,
    pub alloca_name: String,  // codegen が発行した alloca の LLVM 名
}
```

この情報は Phase 4a-ii の `DILocalVariable` + `llvm.dbg.declare` 生成に使う。

### 4. codegen での行情報使用

`tyra-codegen-llvm/src/codegen.rs` / `instr_emit.rs` は命令発行時に
`Stmt::loc` を追跡し、以下の 2 用途に使用する:

- **DWARF**: `!DILocation(line: N, col: M, scope: !func_scope)` を `!dbg` に付与
- **coverage**: `(file_id, line)` カウンタのインクリメント挿入（§5 参照）

### 5. coverage 計装方式（Tyra 独自・簡易行レポート）

LLVM covmap（`__llvm_covmap` / `__llvm_prf_*`）には**寄せない**。
以下の Tyra 独自方式に確定する:

**カウンタ単位と配置ルール（BB 単位構造的ルール）**:
- カウンタは `(file_id, line)` ペアにつき 1 個（グローバル配列の固定インデックス）
- その `(file, line)` を**最初に導入する各 basic block の入口**で、同一カウンタに increment を挿入する
- 同一ソース行が複数の相互排他 BB に lowering されても、実行された BB がカウンタに記録される
- 同一 BB 内で同じ `(file, line)` が再出現しても increment は BB あたり 1 回

**カウンタ配列とサイドカー**:
- `(file_id, line)` → カウンタインデックスの対応表（covmap）をコンパイル時に確定し、
  サイドカー `<bin>.tyra-covmap` として出力する
- カウンタ配列は mmap したサブプロセス固有ファイル（`$TYRA_COV_DIR/<test-id>.covraw`）に置く
- increment は mmap 上で直接行う（atexit フラッシュに依存しない）

**異常終了耐性**:
- 通常終了・`panic`（exit 101）・OOB（exit 102）では `exit()` 呼び出しにより mmap に積んだカウンタが保持される
- OOM（ADR 0012）: Boehm `GC_oom_func` が `abort()` → SIGABRT でプロセスを終了させる。
  SIGABRT は SIGKILL と異なりカーネルがページをフラッシュするため mmap データは通常保持されるが、
  **v0.6.0 では保証しない**（OOM の exit code 分類自体が保証外のため）。best-effort とする。
- `timeout(SIGKILL)` は best-effort（カーネルが dirty ページを flush するタイミングに依存）

**親側マージ**:
- `run_test_file_core` が全サブプロセス完了後に `$TYRA_COV_DIR/*.covraw` を読み、
  同一レイアウトゆえ要素ごと総和して 1 つの集計を得る
- `<bin>.tyra-covmap` と突合し行 / 関数カバレッジを算出

**カバレッジ定義**:
- 行カバレッジ = 合算 counter > 0 の `(file, line)` / covmap 中の総 `(file, line)`
- 関数カバレッジ = 入口 counter > 0 の関数 / 総関数
- **branch coverage は v0.6.0 スコープ外**（レポート / docs に明記）

### 6. DWARF 生成（テキスト IR 手書き）

v0.6.0 では**テキスト IR を維持**し、DWARF メタデータをテキスト IR に手書きで挿入する
（inkwell 移行は別リリースに延期 — スコープ規律 strategy §7.1）。

生成するメタデータノード:
- `DICompileUnit` / `DIFile`（ファイル単位）
- `DISubprogram`（関数単位）
- `DILocation`（命令単位、`!dbg` で付与）
- `DILocalVariable` + `llvm.dbg.declare` / `llvm.dbg.value`（locals 表示用）
- `DIBasicType` / `DICompositeType`（型記述、Int / Bool / String / struct / ADT 最低限）

clang 呼び出し（`tyra-driver/src/lib.rs` ~L1648）に `-gdwarf-4` を追加する
（debug ビルドは既に `-O0`）。

**既知の制約**:
- `-O2` ビルドでは locals が不正確（`-O0` のみ保証）
- 複合型（クロージャ fat pointer、再帰 ADT）の内部展開は段階的に対応
- Boehm GC は非移動型のため、スタック / alloca 上の locals は安定した DWARF 位置に留まる
  （GC-aware unwinding は不要）

## Alternatives considered

### A. inkwell 移行して `DebugInfoBuilder` を利用

型安全な API で DWARF を生成できる。ただし inkwell 移行は単体で別リリース規模の工数になるため、
v0.6.0 のスコープに含めると爆発する。延期。

### B. Span を AST → codegen で side-table 管理（MIR を汚さない）

AST ノード ID と行情報のマッピングテーブルを lowering の外側で維持し、
codegen がそれを参照する方式。MIR の `Instruction` 型が変更されないため
既存の MIR パスへの影響が最小。ただし lowering / monomorphize で生成される
中間命令と元 AST ノードの対応を確実に保つのが困難（照合が脆い）。却下。

## Consequences

**Positive**

- Phase 1 が DAP / coverage / パニック行診断の**共通基盤**になる（一石三鳥）
- パニック時に発生行が診断に出るようになる（Phase 1 の即効 user-visible win）
- テキスト IR 維持によりスコープを v0.6.0 内に収める

**Negative / accepted tradeoffs**

- テキスト IR での DWARF 手書きは冗長でエラーが起きやすい
  （将来の inkwell 移行時に差し替え対象になる）
- `Function::body` の型変更（`Vec<Instruction>` → `Vec<Stmt>`）が
  MIR を参照する全パスに波及する（lowering / monomorphize / codegen 横断）

**Implementation order**

1. `tyra-mir/src/ir.rs`: `SourceLoc` / `Stmt` / `LocalMeta` 追加、`Function::body` 型変更
2. `tyra-mir/src/lower/*.rs`: 全 lowering パスで `span` → `SourceLoc` 伝播
3. `tyra-codegen-llvm/src/codegen.rs`: 命令発行時に現在行を追跡
4. Phase 1 検証: `panic("x")` を含むプログラムで発生行が診断に出ることを確認
5. Phase 3c: coverage 計装（`--coverage` フラグ制御）
6. Phase 4a: DWARF メタデータ生成（`DICompileUnit` → `DISubprogram` → `DILocation`）
7. Phase 4a-ii: `DILocalVariable` + `llvm.dbg.declare` / `llvm.dbg.value`（locals）
