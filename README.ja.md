# Tyra

バックエンドサービス、CLI ツール、業務アプリケーションのための、AI フレンドリーな静的型付け言語。

> **v0.11.0 — AI self-correction** — import したモジュール呼び出しを完全に型検査 (新診断 E0318/E0319。`String + string.from_byte(x)` が codegen でクラッシュしなくなりました)、`Err` を返す main は stderr 報告 + exit 1 (ADR-0029)、`tyra check/build --error-format json` がエージェントループ向け NDJSON 診断を出力 (ADR-0026)、USV 文字 API + `list.sort`/`sort_str` (ADR-0027)、`to_upper`/`to_lower` は `to_ascii_upper`/`to_ascii_lower` にリネーム (破壊的変更)。修正後のマルチシードスイープ結果: **tyra+spec 88.7% mean** (3 seeds × 100 プロンプト、v0.11.0)。本番利用前に [既知の制限](#既知の制限) をご確認ください。

---

## Tyra とは

Tyra は、人間と LLM がコードを共同編集する時代に向けて、ゼロから設計された汎用プログラミング言語です。すべての設計判断は **解釈の一貫性** を最優先します。同じ入力は、人間にとっても AI にとっても、同じ構文木、同じ型、同じ意味を持つべきです。

```tyra
import fs
import string

fn word_count(path: String) -> Result<Int, fs.FsError>
  let text = fs.read_to_string(path)?
  Ok(string.split_whitespace(text).len())
end

fn main() -> Unit
  match word_count("notes.txt")
  when Ok(n)
    print("#{n} words")
  when Err(e)
    print("error: #{e}")
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
export fn main() -> Unit
  print("hello, tyra")
end
```

## 型システムの一端

```tyra
# 代数的データ型と網羅的パターンマッチ
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

# エラーを値として扱い、? で伝播
fn read_port() -> Result<Int, ConfigError>
  let text = fs.read_to_string("app.conf")?
  parse_int(text)?
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

```tyra
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
  let m = m.insert("apple",  1)
  let m = m.insert("cherry", 3)
  for k, v in m
    print("#{k}: #{v}")   # apple, banana, cherry — 昇順が保証される
  end
end
```

**`LinkedMap.from`** — タプルリストから構築:

```tyra
import linked_map

fn main() -> Unit
  let m = LinkedMap.from([("a", 1), ("b", 2), ("c", 3)])
  print("len=#{m.len()}")   # 3
end
```

## 開発状況

**v0.11.0 で安定** — サポート済み・テスト済み:

| コンポーネント | 備考 |
| --- | --- |
| 言語仕様 v0.11 | ✅ 完成 |
| Lexer / Parser / 型検査器 | ✅ 完成 |
| Hindley-Milner 型推論 (rank-1)、E9001 ICE ガード | ✅ 完成 (v0.8.0+) |
| E0308 ヒューリスティック iv — ADT バリアント提案 | ✅ 完成 (v0.8.0+) |
| `LinkedMap<K,V>` / `LinkedSet<T>` — 挿入順保持 永続コレクション | ✅ 完成 (v0.8.0+) |
| `LinkedMap.from([(k,v), ...])` — タプルリストから構築 | ✅ 完成 (v0.10.0+) |
| タプル型 `(A, B)` — let/match/for 分構束縛 (ADR-0022) | ✅ 完成 (v0.10.0+) |
| `SortedMap<K,V>` / `SortedSet<T>` — キーソート永続コレクション (ADR-0024) | ✅ 完成 (v0.10.0+) |
| E0314 — 非表示型の文字列補間コンパイル時診断 | ✅ 完成 (v0.10.0+) |
| Windows MSVC ABI サポート (ソースレベル) | ⚠️ 実験的 (v0.8.0+; CI は LLVM-free crates の `cargo check` のみ) |
| LLVM codegen + Boehm GC runtime | ✅ macOS arm64 / Linux x86_64 (glibc + musl) |
| 標準ライブラリ (例: string, list, map, set, fs, io, json, assert, time, log, sorted_map, sorted_set, linked_map, http) | ✅ 完成 |
| `tyra check / run / build / fmt / test / new / mod / bench` CLI | ✅ 完成 |
| `tyra test --timeout` / `--jobs N` / `--coverage` | ✅ 完成 |
| `tyra bench <dir>` — 汎用 wall-clock ベンチランナー | ✅ 完成 |
| ラムダ / クロージャ (spec §9.4, ADR 0011) | ✅ 完成 |
| ジェネリック `List<T>` + `map`/`filter`/`fold` | ✅ 完成 |
| `Tyra.lock` + floating `branch` 制約 + 推移的依存解決 | ✅ 完成 |
| `Tyra.toml` マニフェスト + `tyra mod` 依存管理 (`--locked` CI モード) | ✅ 完成 |
| HAMT 永続 `Map<K,V>` / `Set<T>` + `for k, v in m` / `for v in s` | ✅ 完成 (v0.7.0+) |
| DAP デバッガ (DWARF + lldb-dap + VS Code ブレークポイント / ローカル変数) | ✅ 完成 (v0.6.0+) |
| `tyra test --coverage` — ライン / 関数カバレッジレポート | ✅ 完成 (v0.6.0+) |
| LSP サーバ (`tyra-lsp`) + VS Code 拡張 | ✅ 開発インストール可 |
| 静的適合コーパス (42 本 + エラー事例 25 本) | ✅ CI ゲート済み |

**実験的** — 含まれているが本番利用不可:

| コンポーネント | 備考 |
| --- | --- |
| `http.server` 標準ライブラリ | ⚠️ 基本 GET/POST ルーティングのみ、本番利用不可 |

**配布・エコシステム**:

| コンポーネント | 備考 |
| --- | --- |
| Homebrew tap (`tyra-lang/tap`) | ✅ v0.10.0+ |
| registry-backed SemVer リゾルバ、`tyra publish` | ⏳ 将来予定 |
| Windows ARM64 / MSVC PDB デバッグシンボル | ⏳ 将来予定 |
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

- **[言語仕様 (日本語)](docs/spec/ja/language-spec.md)** — 唯一の正典
- **[言語仕様 (英語)](docs/spec/en/language-spec.md)** — 翻訳。最新版から遅れることがあります
- **[設計判断記録](docs/design/)** — なぜそう決めたかの記録 (ADR)
- **[RFC](docs/rfcs/)** — 将来バージョンへの変更提案

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

## ソースからのビルド

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
