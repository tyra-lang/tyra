# Tyra

**読みやすく、静的型付けで、Ruby 風味の、コンパイル言語。**
LLVM 経由でネイティブバイナリにコンパイルされます。null なし、`Result`/`Option`、網羅的パターンマッチ、統一されたツールチェーン。

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![CI](https://github.com/tyra-lang/tyra/actions/workflows/release-gate.yml/badge.svg)](https://github.com/tyra-lang/tyra/actions/workflows/release-gate.yml)
[![Language spec](https://img.shields.io/badge/language%20spec-v0.11-informational)](docs/spec/ja/language-spec.md)
[![Run in your browser](https://img.shields.io/badge/playground-run%20in%20browser-brightgreen)](https://tyra-lang.github.io/playground/?sample=showcase&run=1)

**[▶ ブラウザで Tyra を試す](https://tyra-lang.github.io/playground/?sample=showcase&run=1)** — インストール不要 · [サイト](https://tyra-lang.github.io) · [はじめに](docs/getting-started/README.md) · [言語仕様](docs/spec/ja/language-spec.md)

```bash
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
# または: brew install tyra-lang/tap/tyra
```

<img src="docs/assets/demo.gif" alt="インストールからコンパイル・実行までを60秒以内で" width="720">

```tyra
# Tyra による小さな価格モデル: 代数的データ型、網羅的パターンマッチ、
# 値としてのエラー (Result)、そして null がどこにもない。
type Plan =
  | Free
  | Pro(seats: Int)
  | Enterprise(seats: Int, discount: Int)

fn monthly_cost(plan: Plan) -> Result<Int, String>
  match plan
  when Free
    Ok(0)

  when Pro(seats)
    Ok(seats * 20)

  when Enterprise(seats, discount)
    if discount < 0 or discount > 100
      Err("discount #{discount}% is out of range")
    else
      Ok(((seats * 15) * (100 - discount)) / 100)
    end
  end
end

fn main() -> Unit
  let plans = [Plan.Free, Plan.Pro(seats: 5), Plan.Enterprise(seats: 50, discount: 20)]
  for plan in plans
    match monthly_cost(plan)
    when Ok(cost)
      println("$#{cost}/mo")

    when Err(msg)
      println("error: #{msg}")
    end
  end
end
```

```console
$ tyra build pricing.ty -o pricing && ./pricing
$0/mo
$100/mo
$600/mo
```

Tyra は Ruby のように読め、Go のように配布できます — 単一のツールチェーン、単一のネイティブバイナリ (musl では静的ビルド)。設計上、コードは異例なほど予測可能です: 言語仕様だけを与えられ事前学習なしで、Claude は Tyra を**初回で 88.7% の確率**で正しく書けます (3 seeds × 100 プロンプト、[手法](bench/ai-gen/METHODOLOGY.md))。上記の例は実行可能で CI 検証済みです ([examples/launch/showcase.ty](examples/launch/showcase.ty))。

---

## Tyra とは

Tyra は、人間と LLM がコードを共同編集する時代に向けて、ゼロから設計された汎用プログラミング言語です。すべての設計判断は **解釈の一貫性** を最優先します。同じ入力は、人間にとっても AI にとっても、同じ構文木、同じ型、同じ意味を持つべきです。

```tyra
import fs

import string

fn word_count(path: String) -> Result<Int, FsError>
  match fs.read_to_string(path)
  when Err(e)
    Err(e)

  when Ok(contents)
    let words = string.split_whitespace(contents)
    Ok(words.len())
  end
end

fn main() -> Unit
  match word_count("notes.txt")
  when Ok(n)
    print("#{n} words")

  when Err(_)
    print("error: could not read file")
  end
end
```

## なぜ新しい言語が必要か

既存の言語は人間だけのために最適化されています。Tyra が問うのは、**「もし人間と AI の共同作業のためにゼロから言語を設計したらどうなるか?」** です。

その答えは、こういう言語です:

- **`null` がない、truthy/falsy がない、暗黙変換がない** — 曖昧さは人間にも LLM にも敵だから
- **呼び出し時に引数ラベルを明示する** (Swift 風) — コードを読むのに関数定義を毎回見に行く必要がない
- **値型と参照型を言語レベルで区別する** — メモリ意味論が推論ではなく見た目で分かる
- **trait (差し替え可能な振る舞い) と ability (構造的性質) を分離する** — Rust の trait/derive ボイラープレートを排除する独自設計
- **`end` ブロックを使う** — どんな視覚的文脈でもブロック境界が一意
- **公式ツールチェーンが1つ**: `check`、`run`、`build`、`fmt`、`test`、`new`、`mod` が利用可能 — すべて単一 CLI、別途パッケージマネージャ不要

## 設計上の影響元

Tyra は既存言語から **選択的に** 借りています。丸ごと真似はしていません。

| 影響元 | 何を |
| --- | --- |
| Swift | 引数ラベル、値型と参照型の分離、`Optional` の思想 |
| Rust | `Result<T, E>`、`?` 演算子、exhaustive match の ADT、trait |
| Ruby | `end` ブロック、文字列補間 `#{...}` |
| Go | 統一ツールチェーン、GC、単一バイナリ配布 |
| Kotlin | data class の精神を value 型に適用 |

これらの組み合わせ、特に **trait/ability の分離** は Tyra 独自の設計です。

## Hello, World

```tyra
fn main() -> Unit
  print("hello, tyra")
end
```

## 型システムの一端

```tyra
import fs

import string

# 代数的データ型と網羅的パターンマッチ
type Payment =
  | Card(last4: String)
  | Bank(bank_name: String)
  | Cash

fn label(payment: Payment) -> String
  match payment
  when Card(last4)
    "card: #{last4}"

  when Bank(bank_name)
    "bank: #{bank_name}"

  when Cash
    "cash"
  end
end

# エラーを値として扱い、例外を使わない
fn read_port() -> Result<Int, String>
  match fs.read_to_string("app.conf")
  when Err(_)
    Err("could not read app.conf")

  when Ok(contents)
    string.parse_int(contents).ok_or("invalid port number")
  end
end

# 等価性が自動導出される値型
value Point
  x: Float
  y: Float
end

let p1 = Point(x: 1.0, y: 2.0)

let p2 = p1.copy(x: 3.0)
```

## v0.10.0 の新機能 — タプル型、SortedMap、SortedSet

**タプル型** — `let`・`match`・`for` での完全な分構束縛:

```tyra-fragment
fn min_max(xs: List<Int>) -> (Int, Int)
  # ... タプルを返す
end

let (lo, hi) = min_max(values)   # let 分構束縛
```

**`SortedMap<K,V>` と `SortedSet<T>`** — キー昇順でイテレートする永続コレクション。キー型には `Ord` が必要（Float はコンパイル時に拒否 — ADR-0002）:

```tyra
import sorted_map

fn main() -> Unit
  let m: SortedMap<String, Int> = SortedMap.new()
  let m = m.insert("banana", 2)
  let m = m.insert("apple", 1)
  let m = m.insert("cherry", 3)
  for k, v in m
    print("#{k}: #{v}") # apple, banana, cherry — 昇順が保証される
  end
end
```

**`LinkedMap.from`** — タプルリストから構築:

```tyra
import linked_map

fn main() -> Unit
  let m = LinkedMap.from([("a", 1), ("b", 2), ("c", 3)])
  print("len=#{m.len()}") # 3
end
```

## クイックスタート: テスト

`*_test.ty` ファイルを作成して `tyra test` を実行します:

```tyra
# math_test.ty
import assert

fn test_add() -> Result<Unit, String>
  assert.eq(1 + 1, 2)?
  Ok(())
end
```

```bash
tyra test                      # カレントディレクトリの全 *_test.ty を実行
tyra test src/                 # 特定ディレクトリを実行
tyra test --filter add         # 名前に "add" を含むテストのみ実行
tyra test --list               # テスト関数を一覧表示するだけで実行しない
tyra test --format junit       # JUnit XML を出力 (CI のテストサマリ用)
```

完全なガイドは [docs/getting-started/08-testing.md](docs/getting-started/08-testing.md) を参照してください。

## v0.11.0 の新機能

> **AI self-correction** — import したモジュール呼び出しを完全に型検査 (新診断 E0318/E0319。`String + string.from_byte(x)` が codegen でクラッシュしなくなりました)、`Err` を返す main は stderr 報告 + exit 1 (ADR-0029)、`tyra check/build --error-format json` がエージェントループ向け NDJSON 診断を出力 (ADR-0026)、USV 文字 API + `list.sort`/`sort_str` (ADR-0027)、`to_upper`/`to_lower` は `to_ascii_upper`/`to_ascii_lower` にリネーム (破壊的変更)。修正後のマルチシードスイープ結果: **tyra+spec 88.7% mean** (3 seeds × 100 プロンプト、v0.11.0)。本番利用前に [既知の制限](#既知の制限) をご確認ください。全履歴は [CHANGELOG.md](CHANGELOG.md) を参照。

## 開発状況

**v0.11.0 で安定** — サポート済み・テスト済み:

| コンポーネント | 備考 |
| --- | --- |
| 言語仕様 v0.11 | ✅ 完成 |
| Lexer / Parser / 型検査器 | ✅ 完成 |
| LLVM codegen + Boehm GC runtime | ✅ macOS arm64 / Linux x86_64 (glibc + musl) |
| 標準ライブラリ (例: string, list, map, set, fs, io, json, assert, time, log, sorted_map, sorted_set, linked_map, http) | ✅ 完成 |
| `tyra check / run / build` CLI (ゼロ引数プロジェクトモード、`--release`) | ✅ 完成 |
| `tyra build --static` — 静的単一バイナリ (musl) | ✅ 完成 (v0.5.0+) |
| `tyra fmt [--check] [--stdin] <file\|dir>` — フォーマッタ + 100 桁ラッピング | ✅ 完成 |
| `tyra test [--filter] [--list] [--format tap\|junit] [--timeout] [--jobs N]` | ✅ 完成 |
| `tyra test --coverage` — ライン / 関数カバレッジレポート | ✅ 完成 (v0.6.0+) |
| `tyra test` のテストごとのプロセス分離 | ✅ 完成 (v0.5.0+) |
| panic 期待 (`test_panics_*` / `test "name" panics`) | ✅ 完成 (v0.6.0+) |
| `test "name" [panics] <body> end` 言語構文 | ✅ 完成 (v0.6.0+) |
| `continue` 文 | ✅ 完成 |
| `tyra new <name> [--lib] [--vcs none]` — プロジェクトスキャフォールディング | ✅ 完成 |
| `tyra mod init/add/update/remove/show/tree/sync/clean [--locked]` | ✅ 完成 |
| `tyra bench ai-gen` — AI 生成ベンチマークランナー | ✅ 完成 |
| `tyra bench <dir>` — 汎用 wall-clock マイクロベンチマークランナー | ✅ 完成 |
| ラムダ / クロージャ (spec §9.4, ADR 0011) | ✅ 完成 |
| ジェネリック `List<T>` + `map`/`filter`/`fold` | ✅ 完成 |
| ジェネリック `Map<K,V>` — HAMT 永続、`insert`/`remove`/`get`/`contains_key`/イテレーション | ✅ 完成 (v0.7.0+) |
| ジェネリック `Set<T>` — HAMT 永続、`insert`/`remove`/`contains`/イテレーション | ✅ 完成 (v0.7.0+) |
| `for k, v in m` / `for v in s` — Map/Set イテレーション | ✅ 完成 (v0.7.0+) |
| `LinkedMap<K,V>` / `LinkedSet<T>` — 挿入順保持 永続コレクション | ✅ 完成 (v0.8.0+) |
| `LinkedMap.from([(k,v), ...])` — タプルリストから構築 | ✅ 完成 (v0.10.0+) |
| タプル型 `(A, B)` — let/match/for 分構束縛 (spec §11.5) | ✅ 完成 (v0.10.0+) |
| `SortedMap<K,V>` / `SortedSet<T>` — キーソート永続コレクション (spec §11.3, §11.4) | ✅ 完成 (v0.10.0+) |
| E0314 — 非表示型の文字列補間コンパイル時診断 | ✅ 完成 (v0.10.0+) |
| モジュール呼び出しの型検査 — E0318 (未知のモジュール関数)、E0319 (print の表示可能性ゲート) (ADR-0028) | ✅ 完成 (v0.11.0+) |
| `Err` を返す main → stderr 報告 + exit 1; `tyra run` は終了コードを伝播 (ADR-0029) | ✅ 完成 (v0.11.0+) |
| `tyra check/build --error-format json` — NDJSON 診断、全経路で stderr のみ (ADR-0026) | ✅ 完成 (v0.11.0+) |
| USV 文字 API (`string.chars`/`char_at`/`char_code`/`from_char_code`) + `list.sort`/`sort_str` (ADR-0027) | ✅ 完成 (v0.11.0+) |
| E0308 診断の改善 — ヘルプヒント、セカンダリラベル、カスケード重複排除 | ✅ 完成 (v0.7.0+) |
| E0313 — for ループの束縛数不一致診断 | ✅ 完成 (v0.7.0+) |
| ジェネリック `assert.eq` / `assert.ne` (Int, String, Bool) | ✅ 完成 |
| `string.replace` / `string.join` | ✅ 完成 (v0.5.0+) |
| `Tyra.lock` + floating `branch` 制約 + 推移的依存解決 | ✅ 完成 |
| LSP サーバ (`tyra-lsp`) + VS Code 拡張 | ✅ 開発インストール可 |
| DAP デバッガ (DWARF + lldb-dap + VS Code ブレークポイント / ローカル変数) | ✅ 完成 (v0.6.0+) |
| 静的適合コーパス (42 本 + エラー事例 25 本) | ✅ CI ゲート済み |

## プラットフォームサポート

> **唯一の正典。** このセクションはプラットフォームおよびリンクモードのサポート状況に関する唯一の正典です。他のドキュメント (AGENTS.md、リリースノート) はここを参照します。

| プラットフォーム | バイナリ種別 | ステータス |
|----------|-------------|--------|
| Linux x86_64 (glibc) | 動的 | サポート済み |
| Linux x86_64 (musl) | 静的 | サポート済み (v0.5.0+) |
| macOS arm64 | 動的 | サポート済み |
| Windows x86_64 (MSVC) | 動的 (`gc.dll` を同階層に配置) | 実験的 (v0.8.0+; トラッキング専用 CI — [既知の制限](#既知の制限) 参照) |

**musl 静的リリースアーティファクトの使用方法:**

`tyra-*-linux-musl-x86_64-static.tar.gz` リリースには、ビルド済みの静的 `examples/hello` バイナリが含まれます。お使いの環境で静的リンクが機能するか確認するには:

```bash
tar xzf tyra-*-linux-musl-x86_64-static.tar.gz
cd tyra-*/
./examples/hello        # 出力: hello, tyra
file examples/hello     # 出力に "statically linked" を含むはず
```

自分のプログラムを静的バイナリとしてコンパイルするには、musl 向けの `tyra` を使用します (Alpine Linux または同等の musl ツールチェーン上で実行):

```bash
tyra build --static myprogram.ty
```

**v0.4.0 での実験的機能** — 含まれているが本番利用不可:

| コンポーネント | 備考 |
| --- | --- |
| `http.server` 標準ライブラリ | ⚠️ 基本 GET/POST ルーティングのみ、本番利用不可 |

**バックログ** — 未実装:

| コンポーネント | 備考 |
| --- | --- |
| レジストリ (`tyra publish`)、完全なレジストリバックドリゾルバ | ⏳ 将来予定 |
| Homebrew tap (`tyra-lang/tap`) | ✅ v0.10.0+ |
| apt / その他パッケージマネージャ | ⏳ 将来予定 |
| VS Code Marketplace 公開 | ⏳ 将来予定 |

## 既知の制限

- **Windows は実験的**: x86_64-pc-windows-msvc 向けのソースレベル MSVC ABI サポートを実装済みで、`tyra build` は `gc.dll` を出力バイナリと同階層に自動コピーします。ただし LLVM 公式 Windows インストーラは `llvm-sys` が必要とする dev ファイル (lib/include) を同梱しないため、`release-gate-windows` CI では LLVM-free crates の `cargo check` しか実行していません。フルコンパイラを Windows でビルドするには LLVM 22 SDK (dev ファイル込み) のローカルインストールが必要です。Windows ARM64 およびネイティブ PDB デバッグシンボルは将来予定。
- ~~**`LinkedMap.remove` / `LinkedSet.remove` は O(n)**~~: v0.9.0 で解決済み — トゥームストーンモデル採用。
- ~~**HM 型推論は保守的**~~: v0.9.0 で解決済み — チェッカー全体への substitution スレッディング実装済み。
- **`tyra build --static`**: musl 上のみ信頼できます。glibc 静的リンクは非対応 (`getaddrinfo` が壊れます)。
- **`http.server`**: 実験的。シングルスレッド、TLS なし、ミドルウェアなし。本番で使用しないでください。
- **破壊的変更**: v1.0 までは破壊的変更が予想されます。

## ドキュメント

- **[はじめに](docs/getting-started/README.md)** — インストール、hello world、テスト、プロジェクトライフサイクル
  - [プロジェクトライフサイクル](docs/getting-started/09-project-lifecycle.md) — `tyra new`、`tyra mod`、依存関係、ビルド
  - [デバッグ](docs/getting-started/10-debugging.md) — DAP デバッガ、VS Code ブレークポイント、lldb-dap セットアップ
- **[言語仕様 (日本語)](docs/spec/ja/language-spec.md)** — 唯一の正典
- **[言語仕様 (英語)](docs/spec/en/language-spec.md)** — 翻訳。最新版から遅れることがあります
- **[設計判断記録](docs/design/)** — なぜそう決めたかの記録 (ADR)
- **[RFC](docs/rfcs/)** — 将来バージョンへの変更提案
- **[サンプル](examples/)** — 標準ライブラリ機能を示す実行可能プログラム
  - [examples/11-stdlib-time-log.ty](examples/11-stdlib-time-log.ty) — `time.now_unix`、`time.monotonic_millis`、`log.info/warn/error`

## 想定領域

Tyra は次の用途に向けて設計されています:

- Web バックエンド / API サーバ
- CLI ツール
- 社内業務アプリ
- 中小規模サービス

Tyra は次の用途には **適していません**:

- OS やカーネル
- フロントエンド (ブラウザ) 開発
- 極端なリソース制約のある組み込み系
- borrow checker が必要な領域 (Rust の代替ではない)

## 非目標 (v0.1)

言語を小さく予測可能に保つため、以下は採用しません:

- ownership や borrow checker (tracing GC を使用)
- マクロやコンパイル時メタプログラミング
- runtime reflection
- 継承ベースの OOP
- 演算子オーバーロード
- trait object や動的 dispatch
- 例外機構

完全なリストは [仕様 §3 と §22](docs/spec/ja/language-spec.md) を参照してください。

## インストール

### curl | sh (Linux x86_64 と macOS Apple Silicon)

```bash
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
```

デフォルトで `~/.local/bin/tyra` にインストールされます。`--prefix` と `--version` フラグに対応しています。詳細は [docs/getting-started/01-installation.md](docs/getting-started/01-installation.md) を参照してください。

### Homebrew (macOS)

```bash
brew install tyra-lang/tap/tyra
```

### ソースからのビルド

> Rust 1.88+、LLVM 22、および Boehm GC (`bdw-gc`) が必要です。(LLVM 21 も動作します — `--features llvm21-1` を付けてください)

事前インストール:

```bash
# macOS
brew install llvm@22 bdw-gc

# Debian / Ubuntu
sudo apt install llvm-22 clang-22 libgc-dev
```

ビルド:

```bash
git clone https://github.com/tyra-lang/tyra.git
cd tyra
cargo build --release -p tyra-cli
```

バイナリは `target/release/tyra` に生成されます。

## バージョニング

Tyra は2系統のバージョンを持ちます:

- **仕様**: `spec-v0.1.0`, `spec-v0.2.0`, ... のタグ
- **コンパイラ**: `v0.1.0`, `v0.1.1`, ... のタグ

コンパイラは常にどの仕様バージョンを実装しているかを示します:

```console
$ tyra --version
tyra 0.11.0
implementing language spec 0.11
```

Tyra が v0.x の間は **MINOR バージョンアップで破壊的変更を許容** します。v1.0 以降は Rust の Edition モデルに似た方式で破壊的変更を管理します。

## 貢献

Tyra の現段階で最も価値のある貢献は:

1. **仕様を読み**、曖昧さや矛盾を Issue として報告すること
2. **エッジケースを検証する例題プログラム** を書くこと (`bench/static-corpus/` 参照)
3. **ドキュメントの英訳**

コードの貢献も歓迎しますが、アーキテクチャがまだ固まっていません。[CONTRIBUTING.md](CONTRIBUTING.md) と [AGENTS.md](AGENTS.md) をご覧ください。

## 思想

Tyra は、これからの10年のソフトウェアが人間と LLM の協働で書かれることに賭け、その協働には専用に設計された言語が値する、という主張です。AI ツーリングを後付けされた既存言語ではなく。

これはトレードオフを受け入れることを意味します:

- 推論が曖昧さを生むなら、冗長さを取る
- 同等な書き方が複数あるより、1つに絞る
- 賢いショートカットより、明示的な注釈を取る
- 強力で表現力豊かな言語より、小さく学びやすい言語を取る

「言語が予測可能に振る舞ってほしい」「読んだコードが見た目通りの意味であってほしい」「LLM の最初の推測が正しくあってほしい」と感じたことがあるなら、Tyra はあなたのために作られています。

## ライセンス

Apache License 2.0. [LICENSE](LICENSE) を参照。

## 謝辞

Tyra の設計は、仕様策定の過程で AI アシスタントとの反復的なレビューと議論から恩恵を受けました。最終的な設計判断とプロジェクトの方向性はメンテナの責任のもとにあります。

---

[English](README.md) | **日本語**
