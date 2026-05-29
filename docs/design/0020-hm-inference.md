# ADR 0020: Hindley-Milner 型推論 (rank-1) の導入

- **Status**: Accepted
- **Date**: 2026-05-29
- **Spec sections affected**: §12 (Type system), §12.4 (Type inference) (実装内部)

## Context

### 現行 (v0.7) の `Ty::Var` の問題

現行の型チェッカー (`compiler/crates/tyra-types/src/checker.rs`) では、
`Ty::Var` は「型推論変数」ではなく **「何にでも一致する寛容な型」** として実装されている:

```rust
fn types_compatible(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Var, _) | (_, Ty::Var) => true,  // 何にでも一致する
        ...
    }
}
```

この実装は v0.7 では意図的な設計であった (ADR 0017 §5 参照)。
しかし、この「寛容な型」設計が v0.8 では重大な問題を起こしている。

**問題 1: `Ty::Error` が codegen まで到達して E0500 LLVM crash が発生する**

以下のコードパスで `Ty::Error` が生成され、codegen まで漏れ出す:

- `checker.rs:1313` — MapLit のキー/値型が `Ty::Var` のまま残り、codegen で型消去に失敗
- `checker.rs:1944` — MethodCall のレシーバ型が `Ty::Var` のとき、メソッド解決が不定になる
- `checker.rs:1969` — impl method の返り値型フォールバックで `Ty::Error` を返す

**問題 2: AI-gen benchmark の失敗率**

AI 生成コードベンチマーク Run 17 では:

- 2/100 が E0500 (LLVM crash) で失敗
- 失敗の根本原因はいずれも `Ty::Var` が解決されないまま codegen に到達したケース
- ADR 0017 の診断改善で E0308 は減少したが、`Ty::Var` 起因の E0500 は残存している

**問題 3: let-polymorphism の欠如**

```tyra
fn id(x) { x }          // x の型が Ty::Var のまま
let a = id(42)          // Int に使える
let b = id("hello")     // String に使えるはず → Ty::Var が Int に単一化されていると失敗
```

現行では `Ty::Var` が最初に使われた型に無制限に一致するため、
同じ関数が異なる型で使われると予測不能な動作になる。

### なぜ今 HM 推論を導入するか

ADR 0017 では「v0.8+ で Ty::Var 問題を解決する」と明記していた。
v0.8 のターゲット (AI-gen 100/100) を達成するには、
E0500 の根本原因である `Ty::Var` 問題の解決が必須である。

## Decision

**Hindley-Milner 型推論 (rank-1 のみ)** を導入する。

### 1. 推論変数の真の実装

`compiler/crates/tyra-types/src/ty.rs` に以下を追加:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TyVarId(pub u32);

pub enum Ty {
    // 既存
    Int, Float, Bool, String, Unit,
    List(Box<Ty>), Option(Box<Ty>), Result(Box<Ty>, Box<Ty>),
    Named(String), Generic(String),
    Fn(Vec<Ty>, Box<Ty>),
    Error,
    // 変更: Var を真の推論変数に
    Var(TyVarId),  // 以前: Var (引数なし、何にでも一致)
}
```

`TyVarId` は単調増加のカウンタで生成する。各型チェック文脈で `fresh_var()` を呼ぶことで
新しい推論変数を生成できる。

### 2. 代入写像 (Substitution)

```rust
pub struct Substitution(HashMap<TyVarId, Ty>);

impl Substitution {
    pub fn empty() -> Self { ... }
    pub fn bind(id: TyVarId, ty: Ty) -> Self { ... }
    pub fn apply(&self, ty: &Ty) -> Ty { ... }   // 代入を型に適用
    pub fn compose(self, other: Self) -> Self { ... }
}
```

型推論の過程で `Substitution` を積み上げ、最終的に全ての `Ty::Var(id)` を
具体型に置き換える。

### 3. 単一化アルゴリズム (occurs check 必須)

```rust
pub fn unify(a: &Ty, b: &Ty, subst: &mut Substitution) -> Result<(), UnifyError> {
    let a = subst.apply(a);
    let b = subst.apply(b);
    match (&a, &b) {
        (Ty::Var(id), ty) | (ty, Ty::Var(id)) => {
            if occurs(id, ty) {
                return Err(UnifyError::OccursCheck(*id, ty.clone()));
            }
            subst.extend(*id, ty.clone());
            Ok(())
        }
        (Ty::List(a_inner), Ty::List(b_inner)) => unify(a_inner, b_inner, subst),
        (Ty::Option(a_inner), Ty::Option(b_inner)) => unify(a_inner, b_inner, subst),
        (Ty::Fn(a_params, a_ret), Ty::Fn(b_params, b_ret)) => {
            if a_params.len() != b_params.len() {
                return Err(UnifyError::Arity(...));
            }
            for (ap, bp) in a_params.iter().zip(b_params.iter()) {
                unify(ap, bp, subst)?;
            }
            unify(a_ret, b_ret, subst)
        }
        (a, b) if a == b => Ok(()),
        _ => Err(UnifyError::Mismatch(a.clone(), b.clone())),
    }
}

fn occurs(id: &TyVarId, ty: &Ty) -> bool { ... }
```

occurs check により無限型 (例: `a = List<a>`) を拒否する。

### 4. level-based let-generalization (Rémy 1992 / OCaml 流)

let-polymorphism を実装するため、**level (階層番号)** を型変数に付与する:

```rust
pub struct TyVarId(pub u32, pub Level);  // (id, level)
pub type Level = u32;
```

- グローバルレベル = 0; 各 `let` 束縛に入るたびにレベルを 1 増やす
- `let f = expr in body` を型チェックするとき:
  1. `expr` の型推論をレベル L+1 で行う
  2. `expr` の型 `t` を得る
  3. **generalize**: `t` 中でレベル > L の型変数を全て `forall` で量化する
  4. `body` では `f` の型を使うたびに量化変数を fresh な型変数に instantiate する

これにより `id : forall a. a -> a` のような多相型を正しく扱える。

**rank-1 制限**: `forall` は型の最外側にのみ置ける。
`(forall a. a -> a) -> Int` のような rank-2 型は v0.8 では非対応。

### 5. 既存 `Ty::Var` コードパスの全置換

以下のコードパスを全て `unify` 経由に置き換える:

- `types_compatible(Ty::Var, _) -> true` を削除
- `checker.rs:1313` (MapLit): マップのキー型/値型を fresh `Ty::Var` で初期化し、各要素との `unify` で解決
- `checker.rs:1944` (MethodCall): レシーバ型を unify で解決してからメソッド候補を選択
- `checker.rs:1969` (impl method return fallback): `Ty::Error` を返さず、返り値型を `Ty::Var` で unify する

### 6. `Ty::Error` 全廃: `E9001 InternalTypeLeakedToCodegen`

codegen フェーズで `Ty::Error` が到達した場合、従来は LLVM crash (E0500) になっていた。
v0.8 では codegen の入口でガードを設ける:

```rust
// compiler/crates/tyra-codegen-llvm/src/codegen.rs の先頭で
fn codegen_expr(expr: &TypedExpr, ...) -> Result<LLVMValue, CodegenError> {
    if expr.ty == Ty::Error {
        return Err(CodegenError::InternalTypeLeaked {
            code: "E9001",
            message: "内部型エラーが codegen まで到達しました。これはコンパイラのバグです。",
            span: expr.span,
        });
    }
    ...
}
```

`E9001 InternalTypeLeakedToCodegen` は Rust の `panic!` ではなく `Result::Err` として伝播し、
ユーザーに ICE (内部コンパイラエラー) として表示する。
LLVM のセグメンテーション違反や未定義動作は発生しない。

### 7. rank-N 非対応 (v0.8 スコープ外)

v0.8 では rank-1 polymorphism のみ実装する。
以下は v0.9+ に持ち越す:

- Higher-kinded types (`Functor<F>` など)
- `where` 節による追加の ability constraint
- rank-2 以上の型

これらは spec §22 の非目標のまま変更しない。

## Alternatives considered

### A. 現行 `Ty::Var` 寛容設計のまま E0500 だけをパッチ

`Ty::Error` が codegen に到達した場合のガードのみ追加し、HM 推論は導入しない。

**却下**: E0500 はパッチできるが、根本原因 (型が未解決のまま残る) は残る。
AI-gen の他の失敗パターン (多相関数の誤用、MapLit の型推論失敗) が引き続き発生する。
ADR 0017 で「v0.8+ で根本解決」と明記したことと整合しない。

### B. 双方向型検査 (bidirectional type checking)

HM の代わりに bidirectional type checking (Pfenning & Pierce 2004) を採用する。

**却下**: bidirectional type checking は型注釈なしの let-polymorphism に弱い。
Tyra の設計 (型注釈はオプション) では HM の方が自然にフィットする。

### C. Hindley-Milner rank-1 を採択 (本案)

**採択。** OCaml / SML と同系統の推論。level-based generalization は
occurs check と組み合わせることで効率的に実装できる。

## Consequences

**Positive**

- **E0500 撲滅**: `Ty::Error` が codegen に到達しなくなる。E9001 として制御された failure になる
- **AI-gen 100/100 目標**: Run 17 の 2/100 失敗が解消し、目標到達の見込みが立つ
- **let-polymorphism**: `id`, `map`, `filter` などの多相関数が正しく動作する
- **型エラーの精度向上**: `Ty::Var` が何にでも一致しなくなるため、型エラーがより早期に検出される

**Negative / accepted tradeoffs**

- **後方互換性**: v0.7 で silently 通っていた一部プログラムが v0.8 で型エラーになる可能性がある
  - 例: 型注釈なしの空リスト `[]` を異なる型のコレクションに渡すコード
  - CHANGELOG Known Limitations に影響を受けたパターンの件数を記載する
- **コンパイラの複雑性増加**: Substitution / Level 管理が加わる
- **rank-N 非対応**: 高度な型システム機能は引き続き非対応

**実装順序**

1. `ty.rs` に `TyVarId(u32, Level)` と `Ty::Var(TyVarId)` を追加
2. `Substitution` 型と `apply` / `compose` を実装
3. `unify` 関数を occurs check 付きで実装 (単体テスト先行)
4. `checker.rs` の `types_compatible` を `unify` 呼び出しに段階的に置き換え
5. level-based generalization を `let` 束縛のチェックに組み込む
6. `checker.rs:1313`, `:1944`, `:1969` の `Ty::Error` / `Ty::Var` フォールバックを修正
7. codegen 入口に `E9001` ガードを追加
8. 既存 corpus 全件で回帰テストを実施し、新たな型エラーの件数を集計して CHANGELOG に記載
9. AI-gen Run 18 で 100/100 を目標として計測
