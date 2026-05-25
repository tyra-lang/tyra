# ADR 0012: Panic semantics and panic-expectation signaling

- **Status**: Proposed
- **Date**: 2026-05-25
- **Spec sections affected**: §（パニック/異常終了の記述）; テストランナ動作

## Context

Tyra の意図的 `panic` / 配列 OOB / OOM / libc segfault は全て
`abort()` → `SIGABRT` → `exit_code: None` に収束し、テストランナ側から区別できない。
timeout のみ `timed_out: true` で既に別判定されているが、他は均質化されている。

`tyra-cli/src/main.rs` の `run_test_file_core` は `RunOutcome`
（`tyra-driver/src/lib.rs:1713`）の exit code / signal を検査するが、
現状 `SIGABRT` が来た場合の原因分類手段がない。

パニック期待テスト（"このテストは意図的に panic する"とマークされたテスト）を
正しく判定するには、**意図的 panic の exit を他の異常終了と区別する識別シグナルが必要**。
`emit_panic_call` の変更単独では不十分。OOB 等は `panic()` を経由せず
`abort()` を直打ちする経路が codegen / ランタイムに残るため、
それらを踏んだテストを誤って "pass" と判定するリスクがある。

**制約: `core.sys.exit` の公開 API**:
`spec §17.3` に `core.sys.exit(_ code: Int) -> Never` が公開 API として存在する（`language-spec.md:1156`）。
テストコードが `sys.exit(101)` を直接呼べるため、exit code 単独での識別は偽 pass のリスクがある。

**制約: Boehm GC の OOM 動作**:
`tyra-codegen-llvm/src/instr_emit.rs:435` のコメントに明記されている通り、
`GC_malloc` は NULL を返さず、OOM 時は `GC_oom_func`（デフォルトは `abort()`）を呼ぶ。
`abort()` の exit code は OS が決定する（SIGABRT → exit_code: None）。
これは codegen の null-check 分岐では捕捉できない。
OOM を分類するには `GC_set_oom_func` でカスタムハンドラを設置する必要があるが、
v0.6.0 のスコープ外とする。

本 ADR は runner-native なパニック期待機構のための識別シグナルを定める。
**callable な stdlib `assert.panics` API は提供しない**（延期でもなく非提供）。

## Decision

### 1. 全 abort 経路の棚卸しと分類

実装前に、ランタイム / codegen 中の全 `abort()` / `SIGABRT` 発生点を列挙し分類する。
少なくとも以下のカテゴリが存在する:

| カテゴリ | 例 | 識別方法 |
|---|---|---|
| 言語レベル `panic()` | `emit_panic_call`, `builtins.rs` | exit(101) **＋** stderr センチネル |
| ランタイムチェック由来 | `list` OOB 等 | exit(102) |
| OOM / alloc 失敗 | `GC_oom_func` (Boehm GC) | abort() → SIGABRT → exit_code: None（分類保証なし） |
| libc / segfault | OS シグナル (SIGSEGV 等) | exit_code = None (捕捉対象外) |

### 2. 識別シグナルの実装

**意図的 panic の 2 段階識別**:

`core.sys.exit(_ code: Int)` が公開 API として存在するため、exit code 101 単独では
テストコード中の `sys.exit(101)` と区別できない。そのため意図的 panic は
**exit code 101 ＋ stderr センチネル**の組み合わせで識別する。

`emit_panic_call`（`compiler/crates/tyra-codegen-llvm/src/builtins.rs`）を以下に変更:
```
fputs("__TYRA_PANIC__\n", stderr)  // センチネル書き込み
exit(101)                          // 識別用 exit code
```

ランタイムチェック由来（OOB 等）の abort サイト → `exit(102)`（センチネルなし）。

**OOM の扱い（v0.6.0 スコープ外）**:
Boehm GC の `GC_oom_func`（デフォルト: `abort()`）は SIGABRT → exit_code: None を生成する。
これは `__TYRA_PANIC__` センチネルを書かず exit code 101 でもないため、
panic 期待の pass 条件を満たさない（偽 pass しない）。
v0.6.0 では OOM の exit code 分類は保証しない。
将来は `GC_set_oom_func` でカスタムハンドラを設置することで分類できる（延期）。

Phase 1 の Span 再配線（ADR 0014）完了後、センチネルに発生行情報を付加できる
（例: `__TYRA_PANIC__:file.tyra:42`）。

### 3. テストランナ側の判定ロジック

`tyra-driver/src/lib.rs` の `RunOutcome` / `tyra-cli/src/main.rs` の
`run_test_file_core` を以下のとおり拡張:

```
// パニック期待フラグが立っているテストの判定
if test.expects_panic {
    let has_sentinel = outcome.stderr.contains("__TYRA_PANIC__");
    match (outcome.exit_code, has_sentinel) {
        (Some(101), true) => PASS,   // 意図的 panic → センチネル + exit 101 で確定
        (Some(0), _)      => FAIL,   // 正常終了 → panic すべきだったのにしなかった
        (Some(101), false) => FAIL,  // sys.exit(101) の直接呼び出し → センチネルなしで偽 pass させない
        (Some(102), _)    => FAIL,   // OOB → 誤 pass させない
        (None, _)         => FAIL,   // OS signal (OOM abort / segfault 等)
        _                 => FAIL,
    }
} else {
    // 通常テスト: exit 0 が pass、それ以外は fail
}
```

### 4. パニック期待のマーク手段

2 経路を併存させる（意味論は同一）:

- **(A) 命名規約**: `test_panics_*` 関数（既存の `test_*` 命名規約の拡張）
- **(B) 言語構文**: `test "<name>" panics ... end`（ADR 0013 で定める `panics` 修飾子）

どちらも「パニック期待フラグ」をテストメタに付与し、上記 3 の判定ロジックに渡す。

### 5. stdlib 非提供

プロセス生成も終了コード検査もできない stdlib レイヤでは
パニック期待を callable な関数として実装できない。
`assert.panics(...)` 相当の API は **v0.6.0 では提供しない**（将来も計画なし）。
パニック期待はテストランナネイティブの機構のみで実現する。

## Alternatives considered

### A. exit code のみ（不採用）

`core.sys.exit(_ code: Int) -> Never` が公開 API として存在するため、
テストコードが `sys.exit(101)` を呼ぶと偽 pass する。不採用。

### B. signal handler でカスタムシグナル

`SIGUSR1` 等を使って意図的 panic を通知。プロセスモデルが複雑化する上、
Alpine musl 等での移植性懸念があり却下。

### C. longjmp ベースの捕捉

panic を `longjmp` で巻き戻し return value にマッピング。
デストラクタ / GC ルートの安全性が保証できず、スコープが大きすぎるため却下。

### D. callable な stdlib `assert.panics` 関数

プロセス生成・終了コード検査が stdlib からは不可能（ADR-0003 で stdlib は純粋計算 + intrinsic のみ）。
実装不可能なため却下。

### E. 意図的 panic のみシグナル化し他 abort は据え置き

OOB 等が exit code 未分類のまま残ると、OOB を起こすテストが
`panics` 期待で誤 pass する。根本問題を解決しないため却下。

## Consequences

**Positive**

- パニック期待テストが意図的 panic 時のみ pass と判定される（センチネル + exit code の 2 段階）
- `sys.exit(101)` の直接呼び出しによる偽 pass を防止できる
- OOB / segfault を誤 pass させない（exit_code: None または exit(102) → fail）
- OOM は `GC_oom_func` の abort → SIGABRT → None → fail（偽 pass しない）
- TAP / JUnit 出力の失敗分類が精緻化される

**Negative / accepted tradeoffs**

- 全 abort サイトの棚卸しが前提作業として加わる（実装前に実施必須）
- `abort()` から `exit(<code>)` への変更により core dump 生成がなくなる
  （デバッグは `-g` ビルド + lldb で代替可能）

**Implementation order**

1. abort サイト全棚卸し（codegen + runtime の全リスト）
2. `emit_panic_call` → `fputs("__TYRA_PANIC__\n", stderr)` + `exit(101)`
3. ランタイムチェック由来 abort → `exit(102)`（センチネルなし）
4. OOM: `GC_oom_func` は据え置き（abort() → SIGABRT → None、分類保証なし。将来 `GC_set_oom_func` で対応）
5. `run_test_file_core` の判定ロジック拡張（stderr キャプチャ + センチネル検出）
6. Phase 1 完了後: センチネルに発生行情報を付加（例: `__TYRA_PANIC__:file.tyra:42`）
