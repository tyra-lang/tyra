# Tyra Language Specification

- **Version**: 0.1 (under development)
- **Status**: Draft
- **Last updated**: 2026-04-15

## 1. 目的

Tyra は次の性質を同時に満たすことを目指す。

- Ruby 由来の読みやすい構文を持つ
- TypeScript のように実用的な静的型を持つ
- Go のように build / test / fmt / deploy が単純である
- Rust ほど厳格な所有権規則を持たない
- Python より曖昧さが少ない
- AI と人間の共同編集で解釈がぶれにくい

一言で言えば、Tyra は **読みやすく、型安全で、配布しやすく、曖昧さの少ない実用言語** である。

この仕様は言語の意味論を定義する。実装はネイティブコンパイルを主目標とし、リファレンス実装は LLVM を用いる。

---

## 2. 設計原則

### 2.1 明示性

- 構文は一意に解釈できることを優先する
- 呼び出しの省略、暗黙変換、実行時メタプログラミングは採用しない
- `null` は言語仕様に存在しない
- truthy / falsy は採用しない

### 2.2 読みやすさ

- ブロックは `end` で閉じる
- 過剰な記号構文を避ける
- 引数ラベルと型を通じて API の意味を明示する

### 2.3 実用的な型

- 静的型付けを採用する
- 強い局所型推論を持つ
- `Option` と `Result` を標準概念にする
- public API では引数型と戻り値型を必須とする

### 2.4 運用の単純さ

- 公式ツールチェーンを一つに統一する
- formatter を標準化し、コードスタイルの自由度を下げる
- リファレンス実装は単一ネイティブバイナリ生成を第一目標とする

### 2.5 AI フレンドリー

- 同じ入力は同じ AST になりやすい構文を持つ
- 命名規則、import、型表現、エラー処理のパターンを統一する
- DSL 的自由度より補完可能性と可読性を優先する

---

## 3. 非目標

Tyra v0.1 は次を狙わない。

- OS やカーネル向けの極低レベル制御
- Rust のような ownership / borrow checker
- Python のような REPL 中心設計
- フロントエンド専用言語
- 高度なマクロシステム
- 継承ベースの OOP
- runtime reflection

---

## 4. 想定ユースケース

Tyra の第一ターゲットは以下。

- Web バックエンド / API サーバ
- CLI ツール
- 社内業務アプリ
- 中小規模サービス実装

---

## 5. 字句規則

### 5.1 識別子

- 識別子は ASCII のみを使用できる。Unicode 識別子は v0.1 では認めない
- 型名: `PascalCase`
- 関数名・変数名: `snake_case`
- 定数名: `UPPER_SNAKE_CASE`
- モジュール名: `snake_case`

### 5.2 予約語

`fn`, `data`, `value`, `type`, `trait`, `impl`, `let`, `mut`, `if`, `else`, `match`, `when`, `for`, `in`, `while`, `return`, `defer`, `async`, `await`, `spawn`, `import`, `export`, `and`, `or`, `not`, `true`, `false`, `end`

### 5.3 コメント

```tyra
# line comment
```

複数行コメントは v0.1 では採用しない。

### 5.4 文の終端

- 改行で文を区切る
- 必要な場合のみ `,` を用いる
- `;` は採用しない
- `(` `)`, `[` `]`, `{` `}` の内部では改行は文の区切りとならない
- 末尾カンマは許可する

---

## 6. ブロック構文

Tyra はキーワードと `end` によるブロックを持つ。

```tyra
if ready
  run()
else
  wait()
end
```

理由:

- 構造が人間に分かりやすい
- インデントだけに意味を持たせない
- AI にとって境界が明確

### 6.1 トップレベル実行文

エントリポイントファイルでは、宣言以外の文をトップレベルに記述できる。これらは暗黙の `fn main() -> Unit` の本体として扱われる (設計根拠は ADR-0006 を参照)。

式を文位置に置いたものを**式文**と呼ぶ。式文の値は破棄される。トップレベルで許可される実行文は、式文、`let`/`mut` 束縛、`if`、`match`、`for`、`while`、`defer` である。

```tyra
# エントリポイントファイル: fn main は不要
print("hello, tyra")
```

上記は以下と等価である:

```tyra
fn main() -> Unit
  print("hello, tyra")
end
```

宣言 (`fn`, `type`, `value`, `data`, `trait`, `impl`, `import`) はトップレベル実行文ではない。`export` は宣言に付く修飾子であり、実行文ではない。宣言は暗黙 main の外側に残り、実行文のみが暗黙 main の本体に入る。宣言と実行文の混在は許可される。ただし `fn main` は例外であり、トップレベル実行文と共存できない (後述の規則を参照)。

前方参照可能なのはトップレベル宣言名 (関数名、型名、trait 名など) に限る。トップレベル実行文に置かれた `let`/`mut` 束縛は暗黙 main のローカル変数であり、前方参照の対象ではない。

```tyra
# 宣言と実行文の混在: fib は print より後に定義されているが参照できる
print("fib(10) = #{fib(10)}")

fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end
```

規則:

- `fn main` はエントリポイントファイルにのみ定義できる。`import` されるモジュールファイルに `fn main` が存在した場合はコンパイルエラーとする
- `fn main` に `export` を付けることはできない。`main` はエントリポイント専用の関数名であり、外部公開の対象ではない
- `fn main` とトップレベル実行文は同一ファイルに共存できない (コンパイルエラー)
- トップレベル実行文は暗黙の `fn main() -> Unit` として型検査されるため、`?`、`.await`、`return` は使用できない
- `import` されるモジュールファイルにはトップレベル実行文を記述できない (§13.1)
- トップレベル `let`/`mut` はエントリポイントファイルでは暗黙 main のローカル変数であり、モジュールスコープ束縛ではない。実行により評価される束縛はエントリポイントの暗黙 main 内にのみ存在する
- モジュールファイルでは `let`/`mut` を含むトップレベル実行文は一切禁止する
- `defer` はトップレベルで使用可能だが、暗黙 main のスコープ脱出時に LIFO 順で実行される (通常はプログラム終了直前に相当する)
- エントリポイントファイルはツールチェーンが指定する。アプリケーションパッケージではエントリポイントをちょうど1つ要求する。ライブラリパッケージではエントリポイントは不要である

---

## 7. 値と変数

### 7.1 変数束縛

```tyra
let name = "tyra"
mut count = 0
```

- `let` は束縛の再代入を禁止する
- `mut` は束縛の再代入を許可する
- デフォルトは immutable binding である

### 7.2 基本型

- `Int`
- `Float`
- `Bool`
- `String`
- `Rune`
- `Bytes`
- `Unit`
- `Never`

`Int` は 64-bit 符号付き整数、`Float` は IEEE 754 double precision とする。
整数リテラルは文脈型がなければ `Int`、浮動小数リテラルは文脈型がなければ `Float` に推論される。
`Rune` は Unicode scalar value を表す 32-bit 値とする。grapheme cluster は `String` の責務とする。

`Float` は `Eq` ability を持たない。IEEE 754 の `NaN != NaN` と構造的等価の矛盾を避けるため。Float の比較には標準ライブラリ `float` モジュールの関数を用いる (設計根拠は ADR-0002 を参照)。

`Unit` はリテラル `()` で表す。

```tyra
let result: Result<Unit, Error> = Ok(())
```

`Never` は値を持たない型であり、関数が戻らないことを示す。`Never` はすべての型のサブタイプである。

```tyra
fn panic(_ message: String) -> Never
  ...
end

let x: Int = if condition
  42
else
  panic("unreachable")  # Never は Int に合致する
end
```

### 7.3 文字列

通常の文字列は補間をサポートする。

```tyra
let msg = "hello, #{user.name}"
```

エスケープシーケンス:

- `\n` — 改行
- `\t` — タブ
- `\r` — キャリッジリターン
- `\\` — バックスラッシュ
- `\"` — ダブルクォート
- `\0` — null バイト
- `\u{XXXX}` — Unicode コードポイント (1〜6桁の16進数)

#### raw string

raw string は `r"..."` で記述する。エスケープシーケンスと文字列補間は処理されない。

```tyra
let pattern = r"\d{3}-\d{4}"
let path = r"C:\Users\mika\docs"
let query = r"SELECT * FROM users WHERE name = '#{not_interpolated}'"
```

規則:

- `r"..."` 内ではバックスラッシュと `#{}` がリテラル文字として扱われる
- `"` 自体を含めることはできない (エスケープ手段がないため)
- raw string の型は `String` である (通常の文字列と同じ型)

multi-line string は v0.1 では採用しない。

---

## 8. 型システム

### 8.1 基本方針

- 静的型付け
- 強い局所型推論
- public function の引数型と戻り値型は必須
- ローカル変数は推論可

```tyra
let x = 10
let y: Int = 20
```

### 8.2 Nullable を持たない

`null` は存在しない。欠損は `Option<T>` で表す。

```tyra
let user: Option<User> = repo.find_user(id)
```

`T?` 構文は v0.1 では採用しない。型を明示的にするため `Option<T>` に統一する。

### 8.3 Result

回復可能な失敗は `Result<T, E>` で表す。

```tyra
fn parse_int(text: String) -> Result<Int, ParseError>
  ...
end
```

### 8.4 Generics

型適用は宣言位置と型注釈位置で山括弧を用いる。

```tyra
fn first<T>(items: List<T>) -> Option<T>
  ...
end
```

Tyra には **trait** と **ability** の 2 種類の制約がある。

- trait: 差し替え可能な振る舞いを表す
- ability: 型が持つ基本能力を表す compiler-known な制約であり、`impl` では実装できない

v0.1 の ability は `Eq`, `Hash`, `Ord`, `Debug` の 4 つである。

```tyra
fn contains<T: Eq>(_ items: List<T>, _ target: T) -> Bool
  ...
end
```

複数の制約が必要な場合は `+` で結合する。

```tyra
fn deduplicate<T: Eq + Hash>(_ items: List<T>) -> List<T>
  ...
end
```

規則:

- 各型パラメータは 0 個、1 個、または 2 個の制約を持てる
- 制約の形式は `<T: Constraint>` または `<T: A + B>` とする
- `Constraint` には trait または ability を書ける
- 3 個以上の制約、`where` 節、associated type、higher-kinded type は採用しない
- 型適用: `List<Int>`
- インデックス: `items[0]`
- リストリテラル: `[1, 2, 3]`
- 式位置での明示的型適用は turbofish を用い、`parse::<Int>(text)` と書く
- `foo<A, B>(x)` のような式中山括弧は曖昧性回避のため認めない

### 8.5 型エイリアスと Union / ADT

`type` は型エイリアスと ADT の両方に用いる。

型エイリアス:

```tyra
type UserId = Int
type Handler = fn(Request) -> Response
```

Union / ADT:

```tyra
type Payment =
  | Card(last4: String)
  | Bank(bank_name: String)
  | Cash
```

- ADT は data セマンティクス (参照型、GC 管理) を持つ (設計根拠は ADR-0001 を参照)
- 再帰的自己参照を持てる
- exhaustive `match` を要求する
- tag 付き union を標準にする
- named field を持つ constructor pattern は named destructuring を基本とする
- `when Card(last4)` は `when Card(last4: last4)` の省略記法とする
- 全フィールドが `Eq` を満たすバリアントのみで構成される ADT は `Eq` ability を自動で持つ
- `Ord` ability は自動では付与されない (`data` と同じ規則)

#### コンストラクタ呼び出し

ADT バリアントのコンストラクタは `型名.バリアント名` の qualified 形式で呼び出す。

```tyra
let c = Color.Red
let p = Payment.Card(last4: "1234")
let e = AppError.NotFound
```

例外: `Option` と `Result` のバリアント (`Some`, `None`, `Ok`, `Err`) は prelude に含まれるため unqualified で使用できる。

```tyra
let user: Option<User> = Some(find_user())
let result: Result<Int, Error> = Ok(42)
let empty: Option<Int> = None
```

`match` のパターンでは、match 対象の型からバリアントが一意に特定できるため unqualified で記述する。

```tyra
let p = Payment.Card(last4: "1234")   # 構築: qualified

match p
when Card(last4: last4)               # パターン: unqualified
  "card: #{last4}"
when Cash
  "cash"
end
```

### 8.6 value と data

Tyra は `value` と `data` を区別する。

#### value

- 値型である
- 代入・引数渡し・戻り値で意味論上コピーされる
- 実装はコピー省略最適化をしてよい
- フィールドは常に immutable である
- 再帰的自己参照を直接持てない
- 再帰構造を表したい場合は `data` を用いる
- 全フィールドが `Eq` を満たす場合、`Eq` ability を自動で持つ
- 全フィールドが `Hash` を満たす場合、`Hash` ability を自動で持つ
- 単一フィールドの `value` で、そのフィールドが `Ord` を満たす場合に限り、`Ord` ability を自動で持つ
- 全フィールドが `Debug` を満たす場合、`Debug` ability を自動で持つ
- `Hash` ability を持つ型は必ず `Eq` ability も持つ
- `==` は `Eq` ability を持つ場合に使える
- `===` は存在しない
- 組み込み `copy(...)` が自動提供される

```tyra
value Point
  x: Float
  y: Float
end
```

```tyra
value Money
  cents: Int
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)
```

#### value の copy

`value` 型には組み込み `copy(...)` が自動提供される。

- `copy` はすべてのフィールドを任意の named argument として受け取る
- 省略されたフィールドは元の値が引き継がれる
- すべての引数は named argument でなければならない (位置引数不可)
- 引数名は対象 `value` 型のフィールド名と一致しなければならない
- 同じフィールドを複数指定することはできない
- `copy()` (引数ゼロ) は元の値と等価な新しいインスタンスを返す
- 戻り値の型はレシーバと同じ `value` 型である

```tyra
value Point
  x: Float
  y: Float
end

let p1 = Point(x: 1.0, y: 2.0)
let p2 = p1.copy(x: 3.0)         # Point(x: 3.0, y: 2.0)
let p3 = p1.copy(x: 0.0, y: 0.0) # Point(x: 0.0, y: 0.0)
let p4 = p1.copy()               # Point(x: 1.0, y: 2.0)
```

`data` 型には `copy` は自動提供されない。`data` の更新は `mut` フィールドへの直接代入による。

#### data

- 参照型である
- GC 管理である
- 再帰構造や共有を前提にできる
- フィールドは immutable がデフォルトであり、可変フィールドは `mut` を明示する
- `===` は参照同一性を比較する
- 全フィールドが `Eq` を満たす場合、`Eq` ability を自動で持つ
- `Hash` ability は、全フィールドが immutable かつ全フィールドが `Hash` を満たす場合にのみ自動で持つ
- `mut` フィールドを持つ `data` は `Hash` ability を持てない
- `Ord` ability は自動では付与されない
- 全フィールドが `Debug` を満たす場合、`Debug` ability を自動で持つ
- `Hash` ability を持つ型は必ず `Eq` ability も持つ
- `==` は `Eq` ability を持つ場合に使える
- 順序が必要な場合は `sort_by`, `min_by`, `max_by` などのキー関数付き API を使う

```tyra
data User
  id: Int
  mut name: String
end
# User は mut フィールドを持つため Hash を持たない
# Set<User> や Map<User, V> はコンパイルエラー

data Config
  host: String
  port: Int
end
# Config は全フィールド immutable かつ Hash を満たすため Hash を自動で持つ
# Set<Config> は利用可能
```

#### フィールド更新規則

- `value` のフィールド更新はできない
- `data` のフィールド更新は、対象フィールドが `mut` であり、かつ受け手の束縛が `mut` である場合にのみ許可される
- 関数引数の束縛はデフォルトで immutable である
- この規則は Java, Kotlin, Swift の参照型より厳格であり、Tyra は可変状態を束縛レベルでも明示する

```tyra
mut user = User(id: 1, name: "mika")
user.name = "mika sato"
```

### 8.7 Trait

継承の代わりに `trait` を使う。

```tyra
trait Stringable
  fn to_string(self) -> String
end
```

実装:

```tyra
impl Stringable for User
  fn to_string(self) -> String
    "#{self.name}"
  end
end
```

規則:

- `trait` は `value` と `data` の両方に実装できる
- v0.1 の trait dispatch は静的 dispatch のみとする
- trait object は v0.1 では存在しない
- `self` は `value` では値渡し、`data` では参照渡しとする
- 関数名 overload は禁止するが、trait dispatch は overload と見なさない
- 異種要素を 1 つのコレクションで扱う必要がある場合は、trait object ではなく ADT を用いる

### 8.8 Nominal typing

v0.1 は nominal typing を採用する。理由は次の通り。

- エラーメッセージを単純に保つ
- AI による型推定の揺れを減らす
- コンパイラ実装を明確にする

---

## 9. 関数

### 9.1 定義

```tyra
fn add(_ x: Int, _ y: Int) -> Int
  x + y
end
```

`fn main` はプログラムのエントリポイントである。`main` はエントリポイントファイルにのみ定義でき、`export` は付けられない (§6.1)。エントリポイントファイルにトップレベル実行文がある場合は `fn main` の記述を省略できる。省略時は暗黙の `fn main() -> Unit` に正規化される。`Result` 返却や `async` が必要な場合は明示的に `fn main` を定義する。

```tyra
# 明示 main: Result 返却が可能
fn main() -> Result<Unit, AppError>
  let config = read_config("app.conf")?
  start_server(config)?
end

# 明示 async main
async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.listen(port: 8080).await?
end
```

### 9.2 呼び出し

```tyra
let n = add(1, 2)
```

- 括弧は必須
- 引数なし呼び出しも `name()` を必須にする
- `foo bar` のような Ruby 的省略は認めない

### 9.3 引数ラベル

Tyra は Swift に近い規則を採る。

- public function の引数はデフォルトでラベル必須である
- `_` を付けた引数は位置引数とする
- 外部ラベルと内部名が同じ場合は 1 回だけ書く
- 外部ラベルと内部名が異なる場合のみ両方を書く
- 位置引数の後にラベル付き引数を並べられる
- ラベル付き引数の後に位置引数は置けない
- 同じ関数呼び出しで省略可能なラベルは存在しない

```tyra
fn create_user(name: String, admin: Bool) -> User
  ...
end

fn set_position(to target: Point) -> Unit
  ...
end

fn add(_ x: Int, _ y: Int) -> Int
  x + y
end

create_user(name: "mika", admin: true)
set_position(to: point)
add(1, 2)
```

関数パラメータは常に immutable binding である。可変が必要な場合は関数本体内で `mut` 束縛する。

```tyra
fn process(_ x: Int) -> Int
  mut count = x
  count = count + 1
  count
end
```

### 9.4 関数型と匿名関数

`fn` は関数定義、関数型、匿名関数を統一して表す。

関数型は `fn(...) -> T` で表す。

```tyra
fn map<T, U>(_ items: List<T>, _ f: fn(T) -> U) -> List<U>
  ...
end
```

匿名関数は `fn` 式で表す。

```tyra
let double = fn(_ x: Int) -> Int
  x * 2
end
```

クロージャのキャプチャ規則:

- キャプチャはデフォルトで読み取り専用である
- `mut` 束縛の再代入をクロージャ内部から行うことはできない
- `value` のキャプチャは意味論上コピーされる
- `data` のキャプチャは参照として扱われる

### 9.5 return

最後の式は暗黙 return とする。

```tyra
fn abs(_ x: Int) -> Int
  if x >= 0
    x
  else
    -x
  end
end
```

必要なら `return` を使える。

---

## 10. 制御構文

### 10.1 式と文

Tyra は式指向を採る。

- `if` と `match` は式である
- `if` は文位置では文として扱われ、その場合の値は `Unit` である (詳細は §10.2)
- `while` と `for` は文であり、値は `Unit` である
- block の最後の式が block の値になる

#### 論理演算子

論理演算子は `and`, `or`, `not` のキーワードを用いる。

```tyra
if age >= 18 and country == "JP"
  grant_access()
end

if not user.is_banned
  allow_login()
end

if score < 60 or has_warnings
  request_review()
end
```

- `and` — 論理 AND (短絡評価)
- `or` — 論理 OR (短絡評価)
- `not` — 論理 NOT (前置)
- 両辺は `Bool` でなければならない
- 優先順位: `not` > `and` > `or`

`or` は論理 OR 演算子としてのみ使用される。

### 10.2 if

```tyra
if ok
  handle_ok()
else
  handle_error()
end
```

- 条件式は `Bool` のみ
- truthy / falsy は採用しない
- `if` は式位置では式として扱われ、文位置では文として扱われる
- 式位置では `else` を必須とし、両 branch の値型は一致しなければならない
- 文位置 (副作用のみが目的) では `else` を省略できる。値は `Unit` とする

例:

```tyra
# 式位置: else 必須
let label = if x > 0
  "positive"
else
  "non-positive"
end

# 文位置: else 省略可
if user.is_admin
  log.info("admin login")
end
```

「式位置」とは、`if` の値が束縛、関数引数、戻り値、その他の式として使われる位置をいう。それ以外は「文位置」となる。

#### else if

`else` の直後に `if` を続けることができる。この場合、`end` は全体で1つだけ書く。

```tyra
if x > 0
  "positive"
else if x < 0
  "negative"
else
  "zero"
end
```

これは `else` ブロック内に `if` をネストしたものとは異なる。ネストした場合は `end` が2つ必要になる。

### 10.3 match

```tyra
match result
when Ok(value)
  render(value)
when Err(err)
  log_error(err)
end
```

- exhaustive であること
- ADT / enum / literal に使える
- 網羅不能な型に対してはワイルドカード `_` を使う
- パターンは入れ子にできる
- `if` と同様に、式位置と文位置で扱いが異なる
- 式位置では各 `when` 節の値型は一致しなければならない
- 文位置では各 `when` 節の値は捨てられ、全体の値は `Unit` となる
- v0.1 ではガード節を持たない。条件分岐が必要な場合は `if / else` を使う

### 10.4 while

```tyra
while running
  tick()
end
```

戻り値は `Unit` である。

### 10.5 for

```tyra
for item in items
  print(item)
end
```

- C 風 `for (;;)` は採用しない
- 戻り値は `Unit` である
- `break` と `continue` は v0.1 では採用しない

---

## 11. コレクション

標準コレクション:

- `List<T>`
- `Map<K, V>`
- `Set<T>`

リテラル:

```tyra
let nums = [1, 2, 3]
let user_by_id = {1: "mika", 2: "jun"}
```

- map literal のキーは任意の式を許可する
- `Map<K, V>` のキー型 `K` は `Hash` を満たさなければならない
- index 構文は `items[index]` とする
- `items[index]` は境界外アクセス時に panic する
- 安全なアクセスには `items.get(index)` を使い、`Option<T>` を返す

```tyra
let x = items[0]           # 境界外なら panic
let y = items.get(0)       # Option<T> を返す
let z = items.get(0)?      # Option 早期 return (関数戻り値が Option の場合)
```

---

## 12. エラー処理

### 12.1 原則

- 予測可能な失敗は `Result`
- 予測不能なバグは panic
- 例外機構は v0.1 では採用しない
- `Option` は欠損表現、`Result` はエラー表現として区別する。`?` は両方に使える

#### panic

`panic` はプログラムを異常終了させる関数である。マクロではない。

```tyra
fn panic(_ message: String) -> Never
```

```tyra
fn divide(_ a: Int, _ b: Int) -> Int
  if b == 0
    panic("division by zero")
  end
  a / b
end
```

- `panic` は `core` モジュールに含まれ、prelude から常に利用可能
- 戻り値型は `Never` であり、任意の型が期待される位置で使える
- `panic` は回復不能な状態を示す。回復可能な失敗には `Result` を用いる

### 12.2 伝播演算子

`?` は `Result` と `Option` の両方に使える。

#### Result に対する ?

```tyra
fn load_user(_ id: Int) -> Result<User, AppError>
  let row = db.find(id)?
  decode_user(row)?
end
```

規則:

- `expr?` は `expr` が `Result<T, E>` である場合に使える
- 現在の関数戻り値型は `Result<U, F>` でなければならない
- `E` は `Into<F>` を実装していなければならない
- `Ok(value)` なら `value` に評価される
- `Err(e)` なら `Err(e.into())` で早期 return する

#### Option に対する ?

```tyra
fn user_name(_ id: Int) -> Option<String>
  let user = repo.find(id)?
  Some(user.name)
end
```

規則:

- `expr?` は `expr` が `Option<T>` である場合にも使える
- 現在の関数戻り値型は `Option<U>` でなければならない
- `Some(value)` なら `value` に評価される
- `None` なら `None` で早期 return する

#### Into

`Into<T>` は `core` prelude に含まれる標準 trait とする。

```tyra
trait Into<T>
  fn into(self) -> T
end
```

規則:

- `Into<T> for T` はコンパイラが自動提供する
- v0.1 の `?` は `Into` を特別扱いしてよい
- `From` は v0.1 では採用しない

### 12.3 defer

```tyra
fn handle() -> Result<Unit, AppError>
  defer print("handler exited")
  let text = fs.read_to_string("app.conf")?
  ...
end
```

規則:

- `defer` は現在のスコープ脱出時に LIFO 順で実行される
- GC はメモリ回収のみを担う
- リソース解放は `defer` または明示的 close による
- v0.1 は finalizer を持たない

---

## 13. モジュール

### 13.1 ファイルとモジュール

- 1 ファイル = 1 モジュール
- ファイル名はモジュール名と一致
- モジュールファイル (`import` される側) には宣言 (`fn`, `type`, `value`, `data`, `trait`, `impl`) のみ記述できる
- モジュールファイルにトップレベル実行文や `let`/`mut` 束縛を記述することはできない
- v0.1 ではモジュールレベルの初期化セマンティクスを定義しない

### 13.2 import

```tyra
import http.server
import app.user_repo as user_repo
```

規則:

- `import a.b.c` は末尾名 `c` を現在スコープに導入する
- `as` による別名を許可する
- 完全修飾名 `a.b.c.name` の参照も許可する
- wildcard import は禁止
- 相対 import は v0.1 では不採用

### 13.3 export

```tyra
export fn serve(port: Int) -> Result<Unit, ServerError>
  ...
end
```

デフォルトは private。

v0.1 の visibility は `export` と private の二段階のみとする。`internal` 相当の中間可視性は持たない。

---

## 14. 並行処理

### 14.1 方針

- async / await を標準機能とする
- 共有可変状態より message passing を推奨する
- actor は v0.1 では標準抽象ではなくライブラリ提供とする
- リファレンス実装は M:N work-stealing scheduler を用いる

### 14.2 async function

```tyra
async fn fetch_user(_ id: Int) -> Result<User, HttpError>
  ...
end
```

型規則:

- `async fn f(...) -> T` の呼び出し結果型は `Task<T>` である
- async 関数は sync 関数を自由に呼べる
- sync 関数は `Task<T>` を生成できるが、`.await` は async 関数内でのみ使える
- `main` は `fn main() -> Result<Unit, E>` または `async fn main() -> Result<Unit, E>` のどちらでもよい

例:

- `async fn fetch_user(_ id: Int) -> Result<User, HttpError>` の呼び出し結果型は `Task<Result<User, HttpError>>` である
- `fetch_user(id).await?` は次の順で評価される

  1. `fetch_user(id)` -> `Task<Result<User, HttpError>>`
  2. `.await` -> `Result<User, HttpError>`
  3. `?` -> `User`

### 14.3 await

`await` は postfix 形式とする。

```tyra
let user = fetch_user(id).await?
```

規則:

- `.await` は postfix 演算子である
- 結合順序は `.await` が `?` より先である
- `fetch_user(id).await?` は `(fetch_user(id).await)?` と解釈される

### 14.4 spawn

v0.1 では `spawn` を提供する。

```tyra
let task = spawn fetch_user(id)
let result = task.await?
```

規則:

- `spawn` の引数は関数呼び出しのみ許可する (任意の式は不可)
- `spawn f(args)` は関数 `f` を並行実行し `Task<T>` を返す
- `f` が sync 関数の場合、その実行を別タスクで行い結果を `Task<T>` に包む
- `f` が async 関数の場合、`.await` 相当の実行を内部で行い最終結果を `Task<T>` に包む
- v0.1 では task cancellation は言語機能に含めない
- cancellation は将来のライブラリ API に委ねる

---

## 15. メモリ管理

### 15.1 基本方針

Tyra のリファレンス実装は tracing GC を採用する。

- generational
- low-latency を重視
- runtime pause を抑える

### 15.2 所有権は採用しない

- borrow checker はない
- mutable の明示と value/data の区別で事故を減らす

### 15.3 値型最適化

- `value` はスタック配置可能
- escape analysis により不要なヒープ確保を減らす
- `List<value T>` の内部表現は実装定義とする
- レイアウト最適化は意味論に影響してはならない

---

## 16. AI フレンドリー規則

Tyra は人間だけでなく AI が扱いやすいことを設計要件に含む。

### 16.1 採用する規則

- 呼び出しは常に括弧付き
- ブロック終端は `end`
- truthy / falsy を禁止
- `null` を禁止
- public API の型を必須
- import 形式を固定
- formatter でレイアウトを固定
- 関数名 overload を禁止

### 16.2 禁止するもの

- runtime eval
- 動的メソッド定義
- 暗黙レシーバの多用
- 複数の等価構文

---

## 17. 標準ライブラリ

標準ライブラリは2段階に分かれる (設計根拠は ADR-0003 を参照)。

### 17.1 Tier 1: 言語仕様に含まれるもの

コンパイラや型検査器が依存するため、言語仕様の一部として定義する。

#### core

```tyra
# I/O
export fn print<T: Debug>(_ value: T) -> Unit
export fn println<T: Debug>(_ value: T) -> Unit
export fn eprint<T: Debug>(_ value: T) -> Unit
export fn eprintln<T: Debug>(_ value: T) -> Unit

# プログラム制御
export fn panic(_ message: String) -> Never
```

`()` は `Unit` のリテラルである。

#### core.sys

```tyra
export fn args() -> List<String>
export fn env(_ key: String) -> Option<String>
export fn exit(_ code: Int) -> Never
```

#### core.tasks

```tyra
export fn join_all<T>(_ tasks: List<Task<T>>) -> Task<List<T>>
export fn select<T>(_ tasks: List<Task<T>>) -> Task<T>
```

#### Option と Result

```tyra
type Option<T> =
  | Some(value: T)
  | None

type Result<T, E> =
  | Ok(value: T)
  | Err(error: E)
```

#### prelude

以下は全モジュールに自動導入される。import 不要。

標準 trait:

- `Into<T>`
- `Stringable`

compiler-known な標準 ability:

- `Eq`
- `Hash`
- `Ord`
- `Debug`

ADT バリアント:

- `Some`, `None` (`Option` のバリアント)
- `Ok`, `Err` (`Result` のバリアント)

関数:

- `print`, `println`, `eprint`, `eprintln`
- `panic`

演算子との対応:

- `==`, `!=` -> `Eq`
- `<`, `<=`, `>`, `>=` -> `Ord`
- `+`, `-`, `*`, `/` -> 組み込み数値演算のみ。operator overloading は行わない
- `and`, `or`, `not` -> 組み込み論理演算。`Bool` のみ

### 17.2 Tier 2: 別ドキュメントで定義するもの

言語意味論には影響しないが、実用上重要なモジュール。API 仕様は `docs/stdlib/` に別途定義する。

- `string` — 文字列操作 (split, trim, contains, replace 等)
- `collections` — `List`, `Map`, `Set` のメソッド (sort_by, min_by, max_by, map, filter 等)
- `float` — Float の比較関数 (eq, approx_eq, is_nan 等。ADR-0002 参照)
- `json` — JSON パース (§17.3 で v0.1 API 凍結)
- `http` — HTTP サーバ・クライアント
- `fs` — ファイルシステム操作 (§17.3 で v0.1 API 凍結)
- `time` — 時刻・期間
- `test` — テストフレームワーク
- `log` — ロギング

原則:

- 実務でよく使うものは標準に含める
- 依存選定の自由より再現性を優先する

### 17.3 v0.1 で凍結する Tier 2 API

M10 で `fs` と `json` の最小 API を言語仕様として凍結する。他の Tier 2
モジュール (http 等) は M11 以降に別途確定する。

#### 17.3.1 fs

呼出側は `import fs` の上で `fs.read_to_string(...)` のようにモジュール
修飾して呼ぶ。以下は `stdlib/fs.tyra` の宣言抜粋。

```tyra
# stdlib/fs.tyra
export fn read_to_string(_ path: String) -> Result<String, FsError>
export fn write_string(_ path: String, _ contents: String) -> Result<Unit, FsError>
export fn exists(_ path: String) -> Bool

export type FsError =
  | NotFound(path: String)
  | PermissionDenied(path: String)
  | IoError(message: String)
```

- `read_to_string` / `write_string` はファイル全体を読み書きする。
  大容量や streaming が必要な用途は v0.1 のスコープ外 (M11+)。
- `exists` はファイル・ディレクトリを区別しない。
- `FsError.IoError` は `NotFound` / `PermissionDenied` 以外すべてを吸収する
  catch-all バリアント。詳細な errno 列挙は v0.1 では提供しない。

#### 17.3.2 json

呼出側は `import json` の上で `json.parse(...)` / `json.Value` のように
モジュール修飾する。以下は `stdlib/json.tyra` の宣言抜粋。

```tyra
# stdlib/json.tyra
export data Value
  _handle: Int
end

export type JsonError =
  | ParseFailed(message: String, line: Int, col: Int)
  | TypeMismatch(expected: String, got: String)
  | MissingKey(key: String)

export fn parse(_ text: String) -> Result<Value, JsonError>

impl ValueOps for Value
  fn kind(self) -> String                # "null" | "bool" | "int" | "string" | "array" | "object"
  fn as_string(self) -> Option<String>
  fn as_int(self) -> Option<Int>
  fn as_bool(self) -> Option<Bool>
  fn get(self, key: String) -> Option<Value>      # object 限定
  fn at(self, _ index: Int) -> Option<Value>      # array 限定
end
```

- 数値は `Int` のみ対応。JSON 浮動小数点値は `ParseFailed` を返す
  (`Float` accessor は v0.2 以降)。
- 文字列の `\uXXXX` エスケープは BMP とサロゲートペア (RFC 8259 §7)
  に対応する。
- `TypeMismatch` / `MissingKey` は stdlib からは返さない (`as_*` / `get`
  は `None` を返す)。呼出側がユーザ Error として利用するための ADT。
- `json.Value` は GC 管理の opaque ハンドルとして振る舞う (§8.5)。
  v0.1 ではパース済みツリーはプロセス終了まで生存する (明示的解放は
  サポートしない)。実装詳細は `runtime/src/stdlib_json.rs` 参照。

---

## 18. ツールチェーン

Tyra は公式 CLI を一つに統一する。

```bash
tyra new app
tyra run
tyra build
tyra test
tyra fmt
tyra mod
```

### 18.1 目的

- Go 的な運用性を再現する
- 学習コストを減らす
- チーム内の選択肢を減らす

### 18.2 ビルド成果物

- デフォルトは単一ネイティブバイナリ
- リリースビルドとデバッグビルドを持つ

---

## 19. 実行モデル

### 19.1 エントリポイント

プログラムの実行は `fn main` から開始する。エントリポイントファイルは以下のいずれかの形式を取る:

1. 明示的 `fn main` — `fn main() -> Unit`、`fn main() -> Result<Unit, E>`、`async fn main() -> Result<Unit, E>` のいずれか
2. トップレベル実行文 — 暗黙の `fn main() -> Unit` に正規化される (§6.1)

`fn main` はエントリポイントファイルにのみ定義でき、`export` は付けられない。アプリケーションパッケージではエントリポイントをちょうど1つ要求する。ライブラリパッケージではエントリポイントは不要である。複数のエントリポイントが検出された場合はコンパイルエラーとする。

### 19.2 コンパイルフロー

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> backend IR -> native binary
```

リファレンス実装:

```text
source -> lexer -> parser -> typed AST -> mid-level IR -> LLVM IR -> native binary
```

トップレベル実行文を持つファイルは、フロントエンドが宣言と実行文を分類し、実行文を暗黙の `fn main() -> Unit` に正規化する。正規化後の AST は明示 main と同一であり、以降のフェーズに影響しない。

### 19.3 実装方針

- パーサは曖昧性の少ない構文を前提に単純化する
- 型検査後の IR は AI とツールのために安定した形を持つ
- 将来の WASM ターゲットは検討対象とするが v0.1 では非対象

---

## 20. 書式規則

formatter は次を強制する。

- インデントは 2 spaces
- 末尾カンマ規則を統一
- `match` と `if` のレイアウトを固定
- import 順序を固定
- 行分割は formatter が構文単位で決定する

自由なスタイル選択は認めない。

---

## 21. 例

### 21.0 最小プログラム

トップレベル実行文 (§6.1) により、最小のプログラムは1行で書ける:

```tyra
print("hello, tyra")
```

関数定義とトップレベル実行文を混在させることもできる。トップレベル宣言は前方参照可能なので、実行文より後に定義された関数も呼び出せる:

```tyra
print("fib(10) = #{fib(10)}")

fn fib(_ n: Int) -> Int
  match n
  when 0
    0
  when 1
    1
  when _
    fib(n - 1) + fib(n - 2)
  end
end
```

エラー伝播や非同期処理が必要な場合は明示的 `fn main` を使う:

```tyra
fn main() -> Result<Unit, AppError>
  let config = read_config("app.conf")?
  start_server(config)?
end
```

### 21.1 ADT と match

```tyra
type Payment =
  | Card(last4: String)
  | Bank(bank_name: String)
  | Cash

fn label(payment: Payment) -> String
  match payment
  when Card(last4: last4)
    "card: #{last4}"
  when Bank(bank_name: bank_name)
    "bank: #{bank_name}"
  when Cash
    "cash"
  end
end
```

### 21.2 Result 伝播

```tyra
fn read_port() -> Result<Int, ConfigError>
  let text = fs.read_to_string("app.conf")?
  parse_int(text)?
end
```

### 21.3 HTTP サーバの入口

```tyra
import http.server

async fn main() -> Result<Unit, AppError>
  let app = server.new()
  app.get("/health", health_handler)
  app.listen(port: 8080).await?
end
```

---

## 22. v0.1 で保留する項目

次は仕様確定を後回しにする。

- macro system
- operator overloading
- actor model の言語組み込み
- package registry の中央集権 / 分散方針
- foreign function interface の詳細
- task cancellation
- multi-line string
- 3 個以上の制約、where 節、associated type
- guard clause (`when pattern if condition`)
- tuple 型
- structured concurrency
- `break` / `continue`
- モジュールレベルの初期化セマンティクス (`let`/`mut` のモジュールスコープ)
- Tier 2 標準ライブラリ API の詳細 (http, string, collections, time, test, log, float) — `fs` と `json` は §17.3 で v0.1 凍結済

---

## 23. Tyra v0.1 の要約

Tyra v0.1 は次の言語である。

- Ruby 由来の読みやすい構文を持つ
- Python より曖昧でない
- TypeScript のように型が実用的
- Rust ほど厳しくない
- Go のように build / test / fmt / deploy が単純
- AI が補完しやすいように構文と規約が一貫している

Tyra は **可読性、型安全、配布容易性、予測可能性** を最優先にした実用言語である。
