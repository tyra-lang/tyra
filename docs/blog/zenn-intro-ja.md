# Tyra: Rubyの読みやすさと静的型・LLVMネイティブコンパイルを両立する言語

Ruby を書いていると、`end` で閉じるブロックや `#{...}` の文字列補間が体に馴染んでいる。あの読みやすさはそのままに、静的型・no null・ネイティブバイナリが手に入ったら——というのが Tyra です。

Tyra は読みやすさを重視した静的型付け言語で、LLVM 経由でネイティブバイナリにコンパイルします（メモリ管理は Boehm GC）。想定する用途は Web/API バックエンド、CLI ツール、社内業務アプリ。作者は私（Kiyoshi）一人、Apache-2.0、現在 v0.11.0 の pre-1.0 です。

この記事は Rubyist 向けに、Tyra が何を借りて何を変えたのか、なぜ作ったのか、そして「その設計は実際に効くのか」を正直に紹介します。

## まず、動くコードを見てください

これは Tyra の canonical showcase です（CI で `$0/mo` / `$100/mo` / `$600/mo` を出力することが検証されています）。

```tyra
# A tiny pricing model in Tyra: algebraic data types, exhaustive
# matching, errors as values (Result), and no null anywhere.
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

Rubyist の目で読むと、馴染む部分とそうでない部分があるはずです。

馴染む部分:
- ブロックは `end` で閉じる。インデントには意味がありません。
- 文字列補間は `#{...}`。`"discount #{discount}% is out of range"` の通りです。
- 論理演算子は `and` / `or` / `not` のキーワード（`&&`/`||`/`!` ではありません）。

Ruby と変わる部分:
- すべての値に静的な型がつきます。`fn monthly_cost(plan: Plan) -> Result<Int, String>` のように、引数と戻り値の型を書きます。
- `null` / `nil` は存在しません。値の不在は `Option<T>`、失敗は `Result<T, E>` で表します。truthy/falsy もなく、`if` の条件は必ず `Bool` です。
- ADT（代数的データ型）と網羅的な `match`。`Plan` の variant を一つでも書き忘れると、コンパイルが通りません。
- 関数呼び出しは Swift 風の引数ラベル。`Plan.Pro(seats: 5)`、`Enterprise(seats: 50, discount: 20)` のように、呼び出し側でラベルを書きます（`_` を付けた引数は位置引数になります）。

ADT 構築は `Plan.Free` のように型名で修飾します（`Some`/`None`/`Ok`/`Err` は prelude なので無修飾）。`match` の中ではパターンを無修飾で書きます。

### `?` 演算子で失敗を畳む

`Result` と `Option` のどちらにも `?` 演算子が効きます。早期 return が一行で書けます。

```tyra
fn find_user(_ id: Int) -> Option<String>
  if id == 1
    Some("alice")
  else
    None
  end
end

fn user_greeting(_ id: Int) -> Option<String>
  let name = find_user(id)?
  Some("hello, #{name}")
end
```

`find_user(id)?` は、結果が `None` ならその場で `None` を返し、`Some(v)` なら中身を取り出します。例外ではなく、値として失敗を扱う設計です。

## なぜ作ったか — interpretive consistency

Tyra の設計の軸は一つです。**同じ入力が、人にとってもツールにとっても、同じ意味になること**。これを interpretive consistency（解釈の一貫性）と呼んでいます。

曖昧さは、人にとってもツールにとっても敵です。truthy/falsy、暗黙の型変換、`nil` がどこからでも紛れ込む可能性——こうした「文脈で意味が変わる」要素は、読み手に推測を強います。Tyra はその推測を減らす方向に倒しています。だから `if` の条件は `Bool` だけ、変換は明示、不在は `Option` だけ。冗長になる場面はありますが、それは「推論が曖昧さを生むなら冗長を選ぶ」という意図的なトレードオフです。

この方針で生まれた、Tyra の独自な部分が trait と ability の分離です。

- **trait** は差し替え可能な振る舞い。`impl` で実装します。
- **ability** はコンパイラが知っている構造的な性質——`Eq` / `Hash` / `Ord` / `Debug` の4つ。これらは `impl` で実装することが**できず**、規則に従って自動導出されます。

たとえば `Float` には `Eq` ability がありません。IEEE 754 が「NaN は自分自身と等しくない」と定めている以上、`==` を許すと `NaN == NaN` が `false` になる矛盾を書けてしまう。だから Tyra は型レベルで `Float` から `Eq` を外し、`Float` の比較は標準ライブラリの `float` モジュールを通します。この判断の詳細は別記事「Why Float Has No == in Tyra」に書きました。

## 「効くのか」— 設計が効いている一つの証拠

ここまでは言語そのものの話です。ここからは proof point を一つ、正直なフレーミングで。

Tyra には公開された学習データが事実上存在しません。にもかかわらず、**spec をシステムプロンプトに注入すると**、Claude は事前学習なしで初回から正しい Tyra を書きます。

- tyra+spec: 3 seeds × 100 タスクで **平均 88.7% パス**（v0.11.0、"run56"）
- any-seed 98%（3 seed のうち 1 つ以上が通るタスクの割合）
- all-seed 77%（3 seed すべてが通るタスクの割合）

100 タスクは static corpus から、採点は自動ハーネス（生成 → コンパイル/型チェック → 実行）で、人間によるレビューやデバッグは入れていません。

ここで正直に言っておくべきことがいくつかあります。

- **spec 注入は必須です。** 言語 spec とサンプル、標準ライブラリのソースをシステムプロンプトに丸ごと追加しています。これは意図的な前提で、注入なしの Tyra は 0% です（モデルは Tyra を知らないので）。これは「設計のおかげ」ではなく「学習データがない」という事実の裏返しでもあります。逆に言えば、注入された spec の通りに書けば通る、という一貫性が効いているということです。
- **他言語に「勝った」という主張ではありません。** 参考までに別の run での single-seed の点推定として Ruby 99%、Crystal 96%、Go 81%、V 49%、Gleam 37% という数字がありますが、これらは別バイナリ・別条件での単発計測で、方向性の参考でしかありません。同一条件・複数 seed の横断比較はまだ実施しておらず、今後の宿題です。Ruby の 99% は膨大な学習データの反映であって設計の差ではありませんし、Gleam の 37% はテストハーネスが単一ファイルを template プロジェクトに包む都合で実力を低く見積もっている、こちらが開示すべき測定上の制約です。

数字の再現手順は `bench/ai-gen/METHODOLOGY.md` に置いてあります。なお run56 の成果物（result JSON）にはモデル ID ではなく CLI のバージョン文字列 `"2.1.174 (Claude Code)"` が記録されており、ヘッドラインの run で使われた厳密なモデルは artifact 上では pin されていません。これは既知の追跡上の制約で、別途公開する methodology 記事で詳述します。

## 試し方

一番速いのはブラウザです。インストール不要で、上の showcase がそのまま動きます。

- Playground（showcase を実行）: https://tyra-lang.github.io/playground/?sample=showcase&run=1
- サイト: https://tyra-lang.github.io
- リポジトリ: https://github.com/tyra-lang/tyra

手元で動かすなら:

```sh
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
```

または Homebrew:

```sh
brew install tyra-lang/tap/tyra
```

対応は macOS arm64 と Linux x86_64（glibc 動的 / musl 静的）。Windows は experimental です。ツールチェーンは単一バイナリで、`tyra check` / `run` / `build` / `fmt` / `test` / `new` / `mod` が揃っています。LSP サーバと VS Code 拡張、DAP デバッガもあります。

正直な注記として、Tyra は pre-1.0（v0.11.0）です。**v0.x の間は、minor バージョンの更新でも破壊的変更が入ります。** 作者は一人。標準ライブラリの `http.server` も experimental（シングルスレッド、TLS なし、ミドルウェアなし）で本番用途ではありません。実験するには十分ですが、本番投入を約束できる段階ではない、という前提でお願いします。

## Ruby コミュニティへ

Tyra は「Ruby の後継」でも「コンパイルできる Ruby」でもありません。Ruby の読みやすさに影響を受けつつ、より厳格な——`nil` のない、網羅的 `match` を要求する——別の哲学を持つ言語です。動的なメタプログラミングを期待する Ruby ユーザーは、足りないものに気づくはずです。

それでも、`end` で閉じるブロックと `#{...}` の補間が体に馴染んでいる人にとって、Tyra の表面は驚くほど読みやすいと思います。「あの書き心地のまま、型とネイティブコンパイルが欲しかった」という気持ちに、一つの答えを出そうとしている言語です。RubyKaigi で顔を合わせる皆さんに、ブラウザで一度触ってみてほしい。感想や指摘を歓迎します。
