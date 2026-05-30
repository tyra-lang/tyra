# Tyra

バックエンドサービス、CLI ツール、業務アプリケーションのための、AI フレンドリーな静的型付け言語。

> **v0.8.0** — Hindley-Milner 型推論 (rank-1)、E0500 LLVM クラッシュ撲滅 (E9001 ICE ガード)、`LinkedMap<K,V>` / `LinkedSet<T>` (挿入順保持)、E0308 ヒューリスティック iv (ADT バリアント提案)、Windows MSVC ABI 実験的サポート。本番利用前に [既知の制限](#既知の制限) をご確認ください。

---

## Tyra とは

Tyra は、人間と LLM がコードを共同編集する時代に向けて、ゼロから設計された汎用プログラミング言語です。すべての設計判断は **解釈の一貫性** を最優先します。同じ入力は、人間にとっても AI にとっても、同じ構文木、同じ型、同じ意味を持つべきです。

```tyra
import fs

fn word_count(path: String) -> Result<Int, fs.Error>
  let text = fs.read_to_string(path)?
  Ok(text.split(" ").length())
end

export fn main() -> Unit
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
- **公式ツールチェーンが1つ**: `check`、`run`、`build` が現在利用可能。`fmt`、`test`、`deploy` は予定 — すべて単一 CLI

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

## v0.8.0 の新機能 — LinkedMap と LinkedSet

`LinkedMap<K,V>` と `LinkedSet<T>` はイテレーション時に挿入順を保持します。HAMT ベースの `Map` / `Set` がハッシュ順でイテレートするのと異なります。

```tyra
import linked_map

fn main() -> Unit
  let scores: LinkedMap<String, Int> = LinkedMap.new()
  let scores = scores.insert("alice", 95)
  let scores = scores.insert("bob",   87)
  let scores = scores.insert("carol", 92)
  for name, score in scores
    print("#{name}: #{score}")   # alice, bob, carol の順が保証される
  end
  print("len=#{scores.remove("bob").len()}")  # 2
end
```

```tyra
import linked_set

fn main() -> Unit
  let seen: LinkedSet<String> = LinkedSet.new()
  let seen = seen.insert("apple")
  let seen = seen.insert("banana")
  let seen = seen.insert("apple")   # 重複は無視される
  print("len=#{seen.len()}")        # 2
  for item in seen
    print(item)                     # apple, banana の順が保証される
  end
end
```

## 開発状況

**v0.8.0 で安定** — サポート済み・テスト済み:

| コンポーネント | 備考 |
| --- | --- |
| 言語仕様 v0.8 | ✅ 完成 |
| Lexer / Parser / 型検査器 | ✅ 完成 |
| Hindley-Milner 型推論 (rank-1)、E9001 ICE ガード | ✅ 完成 (v0.8.0+) |
| E0308 ヒューリスティック iv — ADT バリアント提案 | ✅ 完成 (v0.8.0+) |
| `LinkedMap<K,V>` / `LinkedSet<T>` — 挿入順保持 永続コレクション | ✅ 完成 (v0.8.0+) |
| Windows MSVC ABI サポート (vcpkg + lld-link, ソースレベル) | ⚠️ 実験的 (v0.8.0+; CI は LLVM-free crates の `cargo check` のみ) |
| LLVM codegen + Boehm GC runtime | ✅ macOS arm64 / Linux x86_64 |
| 標準ライブラリ: string, list, fs, io, float, json, assert, time, log | ✅ 完成 |
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
| 静的適合コーパス (33 本 + エラー事例 21 本) | ✅ CI ゲート済み |

**実験的** — 含まれているが本番利用不可:

| コンポーネント | 備考 |
| --- | --- |
| `http.server` 標準ライブラリ | ⚠️ 基本 GET/POST ルーティングのみ、本番利用不可 |

**バックログ** — 未実装:

| コンポーネント | 備考 |
| --- | --- |
| registry-backed SemVer リゾルバ、`tyra publish` | ⏳ 将来予定 |
| inkwell IR 生成への移行 (writeln! → builder API) | ⏳ v0.9 予定 |
| Windows ARM64 / MSVC PDB デバッグシンボル | ⏳ v0.9 予定 |
| `SortedMap` / `SortedSet` (ソート順コレクション) | ⏳ v0.9 予定 |
| ビルド済みバイナリ (Homebrew, apt) | ⏳ 将来予定 |
| VS Code Marketplace 公開 | ⏳ 将来予定 |

## 既知の制限

- **Windows は v0.8.0 では実験的**: x86_64-pc-windows-msvc 向けのソースレベル MSVC ABI サポートを実装済みで、`tyra build` は `gc.dll` を出力バイナリと同階層に自動コピーします。ただし LLVM 公式 Windows インストーラは `llvm-sys` が必要とする dev ファイル (lib/include) を同梱しないため、`release-gate-windows` CI では LLVM-free crates の `cargo check` しか実行していません。フルコンパイラを Windows でビルドするには LLVM 21 SDK (dev ファイル込み) のローカルインストールが必要です。MinGW GNU ABI、Windows ARM64、ネイティブ PDB デバッグシンボルは v0.9 以降。
- **`LinkedMap.remove` / `LinkedSet.remove` は O(n)**: entries 配列を毎回再構築します。削除が頻繁なユースケースには `Map` / `Set` を使ってください。
- **HM 型推論は保守的**: `types_compatible()` は現在、呼び出しごとに使い捨ての substitution を使っており、チェッカー全体に伝播させていません。完全な推論伝播は v0.9 予定。ほとんどのプログラムには影響ありませんが、稀に予期しない型エラーが出る可能性があります。
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

> Rust 1.88+、LLVM 21、および Boehm GC (`bdw-gc`) が必要です。

事前インストール:

```bash
# macOS
brew install llvm@21 bdw-gc

# Debian / Ubuntu
sudo apt install llvm-21 clang-21 libgc-dev
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
tyra 0.4.0
implementing language spec 0.4
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
