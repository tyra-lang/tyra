# Tyra 成長戦略：2 → 1000 stars（〜6ヶ月）

- **Status**: Active（実行計画）
- **Last updated**: 2026-06-20
- **Scope**: 流通・ローンチ・コンテンツ実行計画。「なぜ Tyra が存在するか」は `docs/strategy.md`（v2.0 ポジショニング）が SoT。本書はその上に立つ「どう広めるか」。
- **Framing**: readability-first / AI-as-proof。多エージェント・リサーチ（PL ローンチ事例 A1 / HN×AI 訴求 A2 / 資産監査 A3 / 仕様レバー A4 / GitHub×AI-SEO A5）＋ 完全性クリティックを経た版。

---

## 1. 結論（TL;DR）

- **正直な確率評価：6ヶ月で 1000 stars は「aspirational（野心的）」。** 比較対象の現実：Mojo は1日（Lattner の知名度）、Roc は1ヶ月（講演）、Gleam は約3年（slow-burn）、Inko は11年経っても約 1,281 stars（ローンチ不在）。**base case は 400〜700 stars。1000 到達には「第1ローンチ」と「第2スパイク（methodology 記事）」の両方が front page に乗ることが必要条件。** どちらか一方だけなら 400〜700 に着地する公算が高い。
- **第1レバー＝ローンチスパイク。** 数字は band で持つ（§9.1）：HN front page（150〜300pt）が PL repo に歴史的にもたらすのは **72h で 150〜400 stars**。Lobsters/Reddit/Zenn の multi-channel 増幅は**独立した第2の 300 ではなく、同じ訪問者プールへの +20〜40%**。したがって **launch-week の現実的 band＝250〜500 stars、base case 350、stretch 600。**
- **フレーミングは readability-first 固定。** AI ベンチ（88.7%）はタイトルに**絶対入れない**、本文の再現可能な proof point に留める。2026年は AI マーケ疲れがピーク（r/programming は LLM 全話題を禁止、Nanolang 232pt のトップコメは「unfalsifiable」批判、A2）。
- **WASM Playground が最大の不公平な武器。** Roc/Gleam の HN スレで最も賞賛されたのが「ブラウザで即動く」こと（A1）。ローンチ投稿の主役。ただしローンチ gate を通すのは「deep-link で /playground に飛ばすボタン」だけで十分（hero 埋め込みインライン実行は P1、§4.4）。
- **最大のリスク（単一）＝ローンチ時の自滅。** 現状検証済：(a) GitHub の social-preview カードが全シェアで「2 Stars / 0 Forks」を広告、(b) README L3 が「AI-friendly」で開き、直後の L5 が v0.11.0 の LLM 用語チェンジログ塊、(c) サイト Hero が「The language LLMs get right on the first try」、(d) 2つの codegen ICE（scalar type alias / Result 内 tuple）でデモが壊れうる。**この4つを潰さずにローンチすると、最も注目される瞬間に最も悪い印象を与える。**
- **判断基準：front-page miss は失敗ではなく median（中央値）の結果。** HN が3hで100pt未満なら horizon を即延長せず、§3.8 のリカバリ・プレイブックを発動。horizon の正直な再評価は **Week 8 の go/no-go** で行う。

---

## 2. 現状診断：強い製品、ゼロの流通

| 項目 | 状態 |
|---|---|
| **製品成熟度** | 高い。v0.11.0、完全型システム（no null, Result/Option, ADT+exhaustive match, traits vs abilities, value/reference, Swift 式引数ラベル, Ruby 式 end）、LLVM codegen + Boehm GC、stdlib、LSP + VS Code 拡張、DAP デバッガ、formatter、test runner、`tyra mod`、`tyra new`、AGENTS.md 生成 |
| **流通** | ほぼゼロ。2 stars / 0 forks、**未ローンチ**。失敗ではなく「正常な未ローンチ状態」 |
| **差別化** | "interpretive consistency"（同じ入力が人間にも LLM にも同じ意味）。ベンチ：tyra+spec 88.7% mean first-try（3 seeds × 100 prompts, v0.11.0） |
| **隠れた資産** | WASM Playground、`--error-format json`（agent self-correction ループ用、A4「underexploited differentiator」）、llms.txt/AGENTS.md |
| **既知の地雷** | social-preview が「2 Stars」表示、homepageUrl 空、README に badge/playground リンクなし＆L3 AI-first lead＆L5 changelog 塊、サイト Hero が AI-first、codegen ICE 2件、HTTP クライアント GET-only |

**結論：ボトルネックは言語ではなく「ローンチ面（launch surface）」。** 新機能は star を動かさない。動かすのは ①デモ可能性、②摩擦ゼロの初回実行、③ベンチの防御可能性 の3つだけ。

---

## 3. ローンチ作戦（the spike）— #1レバー

### 3.1 チャネルとシーケンス（24〜48h 枠に集中）

```
T+0h   HN story submission（主砲）  ── Sun 19:00 ET / Mon 00:00 UTC
T+2-4h Lobsters（HN が catch してから、注意を分散）
T+3-6h r/ProgrammingLanguages（feedback-seeking framing）
同週    Zenn（日本語、同週着地で cross-amplification）
       ※ r/programming には絶対投稿しない（2026年 LLM 全面禁止）
```

velocity 集中の狙い：Tyra の baseline は約0 stars/日なので、控えめなスパイクでも velocity 比が高く、GitHub Trending（実用閾値〜500 stars/24h）に topic-filtered で載りうる。ただし独立した star 源ではなく §9.1 band を stretch 側に寄せる乗数として扱う（二重計上しない）。

### 3.2 タイミング（188k 投稿分析、A2）

- **最良：日曜 19:00 ET（月曜 00:00 UTC）= 50pt 超の確率 10.8%**
- 次点：土曜 02:00 UTC（9.8%）、土曜 19:00 UTC（9.2%）
- 最悪：平日早朝 UTC（木 06:00 UTC, 2.6%）
- **最初の60分が勝負**：1時間で約30〜50 upvote、30分で10 upvote が3時間分散より効く

### 3.3 HN は Show HN ではなく story submission

A1：Gleam/Roc/Crystal/Unison/Mojo の front-page hit はすべて告知/「The X Language」blog 記事の**通常 story 投稿**で、Show HN PL 投稿（多くは 300pt 未満）を上回った（Crystal 306/254, Unison 410, Gleam 331）。→ **告知エッセイ URL（docs サイト上）を story 投稿**。タイトルは factual・readability-led・hype/AI/superlative ゼロ。

### 3.4 候補タイトル（AI をタイトルに入れない）

**HN（いずれか1つ）：**
1. `Tyra: a readable, statically-typed, Ruby-flavored compiled language`
2. `The Tyra programming language: Ruby-like syntax, static types, no null, LLVM-compiled`
3. `Tyra: a Ruby-flavored compiled language with no null, Result/Option, and exhaustive match`

**Lobsters**（tags: `plt`, `compilers`）：
- `Tyra: a readable, statically-typed compiled language (Ruby-flavored, LLVM)`

**r/ProgrammingLanguages**（この層だけ AI を design-thesis としてタイトルに出せる）：
- `Designing a language for interpretive consistency — same code means the same thing to humans and tools (incl. LLMs)`
- `Tyra: a Ruby-flavored statically-typed language — and what we learned measuring first-try correctness`

### 3.5 pre-launch checklist（**全項目 TRUE になるまで投稿しない**）

> 行番号は記載しない（編集で陳腐化）。**安定アンカー（CSS クラス名・grep 対象文字列・関数名）で指定**。括弧内の行番号はヒントに過ぎず、編集前に必ず現物を確認（CLAUDE.md 整合）。

**REPO（A5 P0）**
- [ ] Settings > Social preview にカスタム画像。**`website/public/og.png`（実測 1200×630）をそのままアップロードしてよい。** GitHub 推奨最小は 1280×640 だが 1200×630 は問題なくレンダリングされる。「1280×640 に作り直す」は不要。アップ後 `https://opengraph.githubassets.com/1/tyra-lang/tyra` を fetch し、**カードに画像が表示され、かつ overlay に star 数が出ないことを目視確認** ← **単一最高 ROI**
- [ ] homepageUrl を `https://tyra-lang.github.io` に設定（現状空）
- [ ] **README 先頭を readability-first に全面再構成（§3.5a、L3 と L5 の両方）**
- [ ] topics 追加：`ruby` `gc` `cli` `systems-programming` `native` `ahead-of-time-compilation` `ai-coding`

**SITE（§4 で詳述、A3/A4）**
- [ ] Hero H1（`.hero-tagline`）を readability-first に、88.7% を proof line に降格
- [ ] above-the-fold に install 行（copy ボタン）＋ runnable snippet
- [ ] 88.7% 表記から methodology ページへ**1クリック**で到達
- [ ] /compare を nav/index にリンク、sitemap.xml + robots.txt、site root に llms.txt

**PRODUCT（A4）**
- [ ] head 検証済みの Playground preset 3〜5個。**scalar type alias と Result 内 tuple の codegen ICE、GET-only/ハングする HTTP 例を全 snippet で回避**
- [ ] **canonical showcase snippet を1つ凍結（§3.5b）。** public snippet を `examples/launch/` に集約し CI で HEAD に対し compile+run gate
- [ ] **<60秒 terminal cast**（install→write→compile→run）を asciinema→SVG/GIF で収録し README 冒頭に埋め込み

**WRITE-AHEAD（投稿日前にファイルで用意）**
- [ ] (a) HN author first-comment、(b) skeptic 返信2種、(c) dev.to canonical 記事、(d) Zenn 記事

### 3.5a README 再構成（L3 と L5 の両方を直す）

現状問題は2箇所：**L3 が「A statically-typed, AI-friendly programming language…」**（AI-first lead）、**直後の L5 が v0.11.0 の LLM 用語チェンジログ塊**。launch 訪問者が最初に見る2要素が両方とも AI-first を叫んでいる。

新しい開き順（上から）：
1. 1行 readability-first タグライン（例：*A readable, statically-typed, Ruby-flavored compiled language. Compiles to native binaries via LLVM. No null, Result/Option, exhaustive match.*）
2. **「Try Tyra in your browser (no install)」Playground リンク** ＋ copy-button 付き install 行 ＋ badge 行4〜6個（version / Apache-2.0 / CI / Run in browser）
3. **凍結した showcase snippet（§3.5b）＋ <60秒 terminal cast**
4. THEN 詳細。**v0.11.0 チェンジログ塊は fold より下（"Recent changes" 節）か CHANGELOG へのリンクに移動**

検証：L3 の AI-first lead が消えていること **かつ** changelog 塊が冒頭から退いていること、の両方。

### 3.5b 凍結する canonical showcase snippet と terminal cast

`examples/launch/showcase.ty` に **唯一の正典 showcase（12〜20行）** を author＆凍結。要件：ICE-safe、HEAD で compile+run 検証済、**exhaustive match + Result/Option + 文字列補間** の「wow」を1画面で（fib ではない）。**この同一 snippet を hero / og.png / README / playground default / HN first comment のすべてで使う。** CI gate（§5 P0-guard）が常時検証。`examples/launch/` の「30秒 hello」を **<60秒 terminal cast** に収録し README 冒頭に埋め込み。

### 3.6 day-of ops

1. 最良枠に story 投稿 → 即 **first comment を投下**（URL のみ投稿はペナルティ）
2. first comment = 機能列挙ではなく**「なぜ作ったか」の個人ストーリー**（ソロ維持者、Ruby 親和、interpretive-consistency の設計動機）。Playground と methodology をリンク
3. 最初の3〜4時間、実質的質問に**15分以内**で非防御的に返信
4. 本物の早期読者（Ruby 仲間、r/PL）に「見て」と頼むのは OK。**upvote 要求・downvote 不満は絶対 NG**（投票リング検出）
5. **3hで100pt未満なら §3.8 リカバリ発動**（攻撃的再投稿はしない）

### 3.7 skeptic-handling kit（事前に書いておく）

**「なぜ Python/Crystal でない？ LLM は既存言語をドキュメントから学べる」**（Nanolang トップコメ）
> LLM が既存言語をうまく扱えるのは認める。Tyra が狙うのは測定可能な特定の差＝曖昧な構文を排した interpretive consistency。主張ではなく再現可能な 88.7% ベンチ（3 seeds×100 prompts）と methodology を示す。`--error-format json`（agent self-correction ループ用 NDJSON）も提示。

**「また新言語か」**（Inko コホートの空気）
> 自明に check できる長所から：no null, Result/Option, ADT+exhaustive match, traits vs abilities, Ruby 式 end。5秒で Playground で動かせる。AI の話の前に言語自体の merit で勝つ。

**「どのモデル？ どのプロンプト？ 再現できる？」**
> methodology への1クリックリンク＋複製の明示的招待。**精査を信頼の勝ちに転換する。** **現状クロス言語の same-condition baseline は未取得である旨を正直に開示。**

### 3.8 ローンチ失敗時リカバリ・プレイブック（front-page miss は base case）

HN が**最初の3hで100pt未満**なら：
1. **同 URL を削除・再投稿しない**（penalty/flag 対象）
2. **同週内に r/ProgrammingLanguages + Lobsters を主チャネルに pivot**（feedback-framed タイトル、§3.4 の r/PL 案）
3. **4〜6週後に別アングルの HN story（methodology 記事「What 300 LLM prompts taught us about language design」）を「本命」第2ショットとしてスケジュール。** 第1ショットは dry run と位置づける
4. second-chance pool で投稿時刻リセットは可（攻撃的再投稿はしない）

horizon の正直な再評価は Week 8 go/no-go まで保留（§8）。

---

## 4. サイト充実（conversion）— P0 before-launch

すべて `/Users/kiyoshi/Documents/projects/tyra-lang/website` への変更。**位置は安定アンカーで指定。括弧内の行番号は編集前に要確認のヒント。**

### 4.1 Hero を readability-first に（最高レバー、約30分の copy edit）

`index.astro` の **`.hero-tagline`**（おおよそ L103 付近）を：
> **A readable, statically-typed, Ruby-flavored compiled language**

**`.hero-sub`** を readability-led に：
> Tyra reads like Ruby, compiles to native binaries via LLVM, and ships with no null, exhaustive pattern matching, and Result/Option built in. Its design also makes code unusually predictable — with only the spec and no prior training, Claude writes correct Tyra on the first try **88.7% of the time** (3 seeds × 100 prompts).

88.7% は**最後の一文**に。`Base.astro` の default `description`、`index.astro` の `<Base description=>`、`og:image:alt`、`<title>` も readability-first に統一（各々 grep で現物確認）。

### 4.2 above-the-fold に install 行（copy ボタン）

**`.hero-actions` 直下**に（README で検証済のコマンド）：
```
curl -fsSL https://raw.githubusercontent.com/tyra-lang/tyra/main/scripts/install.sh | sh
```
副行：`or: brew install tyra-lang/tap/tyra`。**動かないコマンドを絶対に出さない**（投稿前に実機で curl 実行確認）。

### 4.3 GitHub ボタンを「Star on GitHub ★」に

**hero の GitHub CTA**（`index.astro`、おおよそ L111 付近）を `Star on GitHub ★` に。**star 数をハードコードしない**（ローンチ時「2」＝負の社会的証明）。動的 badge か、数字を出さず動詞「Star on GitHub」を使う。

### 4.4 Hero code を「動く」に — MUST と NICE を分離

- **MUST（ローンチ gate、安価）**：hero の static snippet preview を **`/playground?code=<base64>`（または `?sample=<key>`）への deep-link「Run THIS in your browser」ボタン**にする。HN 投稿・README から同じ deep-link を貼れる
- **NICE（P1、ローンチ後で可）**：`/wasm/tyra_wasm.js` を再利用した最小インライン Run ウィジェット。bundle size / hydration / no-JS fallback / mobile が非自明なため、**ローンチを block しない**。no-JS fallback に static `<pre>` を残す

hero snippet は §3.5b で凍結した showcase を使う。

### 4.5 Playground preset 整備 + deep-link

現状 preset は `hello/fib/string/map-set/json`（差別化を lead していない）。差別化 preset 2〜3個追加（ADT exhaustive match / Result propagation / 補間、うち1つは §3.5b 凍結 showcase）。`?sample=<key>` と `?code=<base64>` で deep-link/共有可能に。CodeMirror が `python()` を使っている点（「未完成」の tell）は最小の Tyra StreamLanguage（`fn/end/when/match/let/mut/import`＋文字列＋`#{}`、約80行）に置換。

### 4.6 SEO 土台（P1、同週）

- `/compare` を nav に追加＋ `/compare/index.astro` 作成、5ページを de-orphan、homepage のベンチ表行からリンク（routing は Base.astro に既存なので作業は小さい）
- `@astrojs/sitemap` 追加＋ `public/robots.txt`
- llms.txt を site root にもデプロイ（Claude.ai retrieval が見る場所）
- privacy-light analytics（Plausible/GoatCounter）＋ launch 投稿に UTM → チャネル別 star 帰属

---

## 5. 仕様追加：star を動かす数項目だけ（DEFER 明示）

**A4 の中心命題：ほとんどの仕様作業は star を動かさない。** 動かす3クラスのみ。

### P0（ローンチ前、star を守る/作る）
1. **Playground を主役に**：差別化 preset（head 検証済）＋ Tyra-aware highlighting。「Copy spec for your LLM」ボタンで llms.txt を copy（AI を banner 主張ではなく try-it action として導入）
2. **摩擦ゼロの初回実行**：above-the-fold install 行＋検証済「30秒 hello」＋ <60秒 terminal cast＋ README badge/homepageUrl/social-preview
3. **ベンチ防御 — 優先度を解決**：旧版は「6言語 same-condition sweep」を P0 ブロッカーにしていたが、これは **6 compilers × 3 seeds × 100 prompts = 1,800 generations + 採点**（〜1〜2日）であり、de-emphasize する数字のためにソロ維持者の3週ウィンドウで最重作業を front-load する矛盾。**解決＝launch では既存 tyra+spec 88.7% methodology ページを公開し、「クロス言語 same-condition baseline は未取得」と正直に disclaimer。** フル6言語 sweep は P1（第2スパイク燃料）に降格。現状の「beats Go (81%)」は別 binary/seed 数で directional なので、**same-condition sweep 完了まで「beats Go」表現は site/投稿から撤回**
4. **P0-guard**：全 public snippet（§3.5b 凍結 showcase 含む）を `examples/launch/` に集約し CI gate

### P1（第2スパイクの燃料）
5. **HTTP gap を最小限解消**：`http.client.post`＋request method/path access ＝「30行 JSON API を Tyra で」チュートリアルが end-to-end で書ける。ハングする HTTP 例を修正。**async/wildcards/TLS は scope 外。** ※ローンチを block しない、スコープ厳守
6. **`--error-format json` self-correction ループをライブデモ化**（asciinema/Loom：generate→`tyra check --error-format json`→feed back→fix→run green）
7. **6言語 same-condition sweep の実行**（P0 から降格）。methodology 記事 #8 と site に反映、「beats Go」表現を復活させるならここで根拠付き再導入
8. **anti-hallucination friction wins**（小・複利）：E0305 に両辺 String なら補間提案、spec-injection で `string.split_whitespace` を surface、docs に Q&A 見出し

### P2（明示的に DEFER — ゼロ star、機会費用の回避）
- 2つの codegen ICE の**恒久修正**（ローンチでは回避で十分、早期 adopter のバグ報告順で）
- multi-line strings、追加コレクション、trait objects、async cancellation、**package registry**、Windows polish ← 「進捗感はあるが star=0」。ソロの bandwidth を守るため非ゴールとして明記

---

## 6. 記事・コンテンツ（sustain）— 6ヶ月 14本

主：dev.to（英）＋ canonical home `tyra-lang.github.io/blog`。副：Zenn/Qiita（日、Ruby 親和）。実証済フォーマット＝「設計判断 deep-dive」（"Why Float Has No == in Tyra"）。全記事に固定 footer（Run in browser / Star on GitHub / Try with your own assistant: llms.txt）。

**P0 — canonical infra（公開前にまず構築）**：`website/src/pages/blog/` を作成（docs/blog/ は空）、各記事を owned domain に先に公開、dev.to/Zenn/Qiita は `canonical_url` で逆参照。既存 "Why Float Has No ==" を canonical 化。

**P0 — ローンチ週コア3本：**
1. （launch エッセイ/EN）**The Tyra Programming Language: designing for interpretive consistency** — why-I-built-this、readability-first、runnable snippet（§3.5b 凍結 showcase）。AI/ベンチは中盤1段落のみ。**これが HN 投稿 URL**
2. （methodology/EN）**Measuring first-try LLM correctness: how we got 88.7% (3 seeds × 100 prompts), and how to rerun it** — 全 methodology、再現手順、**クロス言語 baseline 未取得の正直な注記**。skeptic 返信に貼る記事
3. （JP/Zenn）**Tyra: Ruby の読みやすさと静的型・LLVM ネイティブコンパイルを両立する言語** — 同週着地

**P1 — 比較 series 4本（SEO/AI-SEO 複利、〜2週ごと）：**
4. Tyra vs Crystal: two Ruby-flavored compiled languages, compared（"faster than Crystal" は言わない）
5. Tyra vs Gleam: static types, no null, and the readability tradeoffs
6. Result and Option instead of exceptions and null: a practical guide in Tyra
7. Why Tyra has no implicit conversions (and what you write instead)

**P1 — 第2スパイク anchor＋設計 thought-leadership 3本（weeks 6-10）：**
8. （第2スパイク anchor/EN）**What 300 LLM prompts taught us about language design** — methodology/findings を lead、**6言語 same-condition sweep の結果をここで初公開**、HN＋r/PL に design-thesis framing で投稿（§3.8 の「本命」第2ショットと同一）
9. Traits vs abilities: separating replaceable behavior from structural properties
10. Why argument labels at every call site (and why it helps AI too)

**P2 — user-content seed kit 2本＋template：**
11. Solving 5 Advent-of-Code-style puzzles in Tyra（12月の AoC 期に合わせる）
12. Building a CLI tool in Tyra, end to end（HTTP gap を避け、CLI+fs+json+string で確実に動く）
- ＋ stdlib cheat-sheet と「I tried Tyra」blog template を repo に（Gleam AoC 351pt / Zig AoC review 341pt）

**P2 — JP 深掘り＋RubyKaigi funnel 2本：**
13. なぜ Tyra には null が無いのか — Rubyist のための静的型入門（Zenn）
14. Tyra で CLI ツールを作る + RubyKaigi CFP に向けて（Zenn/Qiita）

> bandwidth 優先：P0 の3本は必須。SEO/thought-leadership は月次に落としてよい。**JP parity はローンチコアより先に切る。**

---

## 7. 継続の複利（GitHub / AI-SEO / community）

- **GitHub hygiene（P0）**：social-preview 画像（1200×630 の og.png そのまま）、homepageUrl、README badge、topics 拡充。launch 間で複利的に効く無料施策
- **awesome-lists（P1）**：3〜4本に PR（Awesome-Programming-Languages 系、awesome-compilers、awesome-llvm、awesome-low-level）。各々が永久 backlink＋発見面＋将来 LLM の corpus
- **AI-SEO の正直な評価**：llms.txt/llms-full.md/AGENTS.md は維持・差別化 talking point として良いが、**現状 star/可視性 driver ではない**（2026年 300k/37,894 ドメイン研究で citation lift ゼロ）。例外＝Claude.ai/Desktop は retrieval で尊重。「llms.txt を出している」は credibility detail であって growth lever ではない
- **実際に LLM が言語を推薦する要因＝third-party footprint**（Reddit/HN/SO/forum）。新言語の制約は「corpus に Tyra がほぼ存在しない」こと → HN/r-PL スレ、dev.to/Zenn 記事、awesome-list、（将来 notability 達成後）Wikidata/Wikipedia stub で corpus を製造。**AI-SEO と人間 SEO は1つの workstream に collapse する**
- **docs GEO formatting（P2）**：高 intent docページを直接回答40-60語先頭・統計・Q&A 見出し・1ページ1比較表・著者名+日付・非宣伝 tone（宣伝 tone は -26%）
- **JP channel（Zenn/Qiita + RubyKaigi CFP）**：英語のみ言語がアクセスできない低競争チャネル。ローンチ週 Zenn で cross-amplification、RubyKaigi CFP は2027年の outsized credibility/spike 資産

---

## 8. 90日アクションプラン（star milestone 付き）

> star 目標（算術は §9.1）：**2 → 50（pre-launch warmup）→ base 350 / stretch 600（launch spike）→ 400〜700 base、1000 は第2スパイクも front page に乗った場合の stretch**。

### Phase 0：T-minus（Week 1-3、目標 2 → 50）
- **Week 1**：REPO hygiene 全部（social-preview = og.png 1200×630 をそのままアップ＋fetch 確認 / homepageUrl / **README readability-first 再構成：L3 lead と L5 changelog 塊の両方** / topics）。サイト Hero 書き換え＋install 行＋Star CTA。P0-guard CI gate 構築。**§3.5b の showcase.ty 凍結 ＋ <60秒 terminal cast 収録**
- **Week 2**：Playground preset 整備（head 検証）＋Tyra highlighting＋deep-link＋hero「Run THIS」**deep-link ボタン（MUST）**。**ベンチは既存 tyra+spec 88.7% methodology ページ公開＋クロス言語 baseline 未取得を disclaimer**（ここで 1,800-run はしない）。/compare de-orphan＋sitemap＋robots＋llms.txt site root
- **Week 3**：blog infra 構築、launch 週コア3本＋skeptic kit を**ファイルで完成**。analytics＋UTM。Ruby 仲間/r-PL の本物の早期読者を数名確保。Zenn 記事下書き。**checklist 全 TRUE 確認**

### Phase 1：Launch week（Week 4、目標 50 → base 350 / stretch 600）
- **Sun 19:00 ET**：HN story 投稿＋即 first comment。最初の3-4h は15分以内返信に張り付く
- **3hで100pt未満なら §3.8 リカバリ発動**（同 URL 再投稿せず、同週 r/PL+Lobsters へ pivot、第2ショットを4-6週後にスケジュール）
- **T+2-4h**：Lobsters。**T+3-6h**：r/PL（feedback-seeking）。**同週**：Zenn 公開→Qiita（canonical）
- 48h：referrer 別 star、Playground run 率、Trending 到達、トップ3コメの sentiment を監視
- 直後：good-first-issue 5-10個＋CONTRIBUTING、awesome-lists 3-4本に PR

### Phase 2：Post-launch sustain（Week 5-13、目標 base 400-700 / stretch 1000）
- **Week 5-6**：比較 series #4-5、AoC/CLI seed kit＋template、blog on-site canonical 稼働。HTTP gap 最小解消に着手。**6言語 same-condition sweep を実行**
- **Week 6-10**：**第2スパイク（＝§3.8 の「本命」ショット）**＝記事 #8「What 300 LLM prompts taught us about language design」を methodology-lead（sweep 結果込み）で HN＋r-PL 投稿。`--error-format json` ライブデモ公開
- **Week 8（go/no-go）**：累積 trajectory を判定。**1000 は「第1＋第2の両方が front page」が必要条件**。両方 miss なら base 400-700 で着地と認め、horizon を正直に延長宣言
- **Week 11-13**：比較 series #6-7、設計記事 #9-10、JP #13-14、RubyKaigi CFP 探索

---

## 9. 計測（metrics）と 中止/見直し基準

### 9.1 launch-week star 射影の算術（point estimate を band に置換）

- **構成要素1：HN front page。** PL repo の歴史的レンジ＝ front page（150〜300pt）で **72h に 150〜400 stars**。stars/upvote は固定定数ではなく repo readiness × audience で **0.3〜2.0 と変動**するため単一比を掛けない
- **構成要素2：multi-channel 増幅（Lobsters/Reddit/Zenn/Trending）。** 独立した第2の 300 stars ではなく、**同じ可視性プールを共有する訪問者への +20〜40%**。加算ではなく係数として扱う
- **合成：launch-week band = 250〜500 stars、base case 350、stretch 600**
- **1000 までの経路：** base 400〜700（第1ローンチ＋sustain）。**1000 は第1ローンチと第2スパイク（#8）の両方が front page に乗った場合の stretch。** 片方なら 400〜700

### 9.2 主要メトリクス
- **launch 48h star delta**（主 signal、band 250-500、base 350、stretch 600）
- HN thread points（目標150+、stretch 300+）と front-page 滞在
- **実測 stars/upvote**（事後算出し次回 band 補正に使う。事前固定値にしない）
- referrer 別 referral traffic（news.ycombinator / lobste.rs / reddit / zenn）
- Playground run 数と run 成功率（shipped サンプルがエラーゼロ）
- GitHub Trending 到達（topic-filtered 含む）
- thread sentiment：トップ3コメが skeptical-AI（"why not Python"）か substantive か
- **第2スパイク14日 star bump**、user 生成「I tried Tyra」投稿数（6ヶ月で目標3-5）
- canonical health：全 syndicated post に canonical_url、4週以内に owned domain が同等以上に rank

### 9.3 gate（ローンチ前 binary pass/fail）
- social-preview：og.png(1200×630) アップ済、fetch 確認でカード描画＋star 数 overlay なし
- 全 public snippet（§3.5b 凍結 showcase 含む）が CI gate 通過（live-ICE ゼロ）
- README readability-first（L3 lead＋L5 changelog 位置の両方）/ homepageUrl / install 行 above-the-fold / Hero readability-first / hero「Run THIS」deep-link ボタン がすべて live
- ベンチ：tyra+spec 88.7% methodology ページ公開済＋クロス言語 baseline 未取得の disclaimer 明記（「beats Go」表現は撤回済）

### 9.4 中止/見直し基準
- **HN 3hで <100pt**：失敗ではなく median。**§3.8 リカバリ発動**（horizon 即延長はしない）
- **トップコメが "why not Python" 系で thread が souring**：framing 見直し、second-chance pool／第2ショットで再挑戦
- **Week 8 で第1・第2スパイクが両方 front-page miss**：1000 を「aspirational」と認め、base 400-700 で horizon を正直に延長。bandwidth を P0 コンテンツに集中（P2 機能は全凍結）
- **benchmark methodology が攻撃され未反論**：それ自体が KPI 失格（target 0 unanswered）。same-condition sweep 未完の間は「beats Go」表現を出さない

---

### 一行サマリ
**製品は完成している。残る仕事は「ローンチ面」だ。** readability-first で HN を打ち、Playground を主役にし、88.7% は再現可能な proof point に留め、social-preview・README L3/L5 の AI-first・codegen ICE という自滅要因を先に潰す。良いスパイクで base 350（band 250-500）、第2スパイクとコンテンツで base 400-700。**1000 は両スパイクが front page に乗った場合の stretch であり、率直に aspirational。** 最大のリスクは戦略ではなく**実行の取りこぼし**（壊れたカード・AI-first な hero/README・front-page での ICE）である。

---

*本書は多エージェント・リサーチ（PL ローンチ事例 / HN×AI 訴求 / 資産監査 / 仕様レバー / GitHub×AI-SEO）＋ 完全性クリティックの成果物。ポジショニングの SoT は `docs/strategy.md`。*
