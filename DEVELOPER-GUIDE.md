# Developer Guide

このリポジトリに手を入れる人のための方向づけ。見出しは英語、本文は日本語とする。

**設計判断の根拠はここには書かない。** [ROADMAP.md](ROADMAP.md) の §6「確定した設計方針」に決定 1〜10 として
記録してあり、二重に持てば必ずずれる。ここが持つのは「どこに何があるか」「壊してはいけないもの」「ここでの
作法」の 3 つに限る。合格率や行数といった数値も持たない。README と ROADMAP にあり、変わるものを 3 箇所に
置かない。

## Layout

13 クレート。依存は下から上への一方向で、各層は下の層しか知らない。

| Crate | 責務 |
|---|---|
| `xylogue-core` | 全層が使う語彙。エラーと位置、XML の文字クラス、インターンされた名前、RFC 3986 の URI、文字デコード |
| `xylogue-parser` | XML 1.0 のプルパーサ。DTD（内部・外部サブセット、パラメータ実体）、実体解決、SAX 相当の push アダプタ、sans-I/O コア |
| `xylogue-validate` | スキーマ非依存の `Validator` / `ErrorListener` と、その最初の実装である DTD 検証器 |
| `xylogue-dom` | アリーナ木。`Vec<NodeSlot>` + `Copy` な `NodeId`。W3C DOM Level 3 Core の名前を保つ |
| `xylogue-serialize` | DOM 部分木から整形式 XML へ。エスケープ、名前空間修復、StAX 相当の `XmlWriter` |
| `xylogue-xinclude` | `xi:include` の展開。XPointer の framework / `element()` / `xmlns()` |
| `xylogue-xdm` | XPath データモデル。`Model` トレイトと DOM 実装 |
| `xylogue-xpath` | XPath 1.0。字句、構文、評価器、コア関数、拡張関数の登録機構 |
| `xylogue-xslt` | XSLT 1.0。パターン、スタイルシート、エンジン、`xsl:output` |
| `xylogue-exslt` | EXSLT 各モジュール。エンジンには組み込まず、拡張関数として登録する |
| `xylogue` | ファサード。全層の再輸出と `javax.xml.transform` 相当の `transform` モジュール |
| `xylogue-cli` | コマンドライン。実行ファイル名は `xylogue` |
| `xylogue-fuzz` | ファジングで検査する性質と、その種コーパス |

`fuzz/` はワークスペース外に置く。libFuzzer のターゲットは nightly を要するため、通常のビルドに巻き込まない。

この表は [`crates/xylogue/tests/guide.rs`](crates/xylogue/tests/guide.rs) が検査する。クレートを増減させた
まま表を放置すればテストが落ちる。

## Where to start reading

**`xylogue-xdm` から読む。** 全クレート中で最も小さいが、この設計の中心的な主張がそこにしかない — XPath は
DOM の上ではなく**データモデル**の上で動く。隣接テキストの併合、名前空間ノードの合成、文書順の全順序。ここが
腑に落ちればエンジンの 2,000 行超が読め、落ちなければどこも読めない。

以降は `xylogue-dom` → `xylogue-xpath` → `xylogue-xslt` の順。**`xylogue-parser` は最後でよい。** 最も
難しいが、最も外部検証が効いており、実装を疑う理由ができるまで読む必要はない。

各クレートの `lib.rs` 冒頭は方向づけとして書いてある。まずそこを読み、そこが指すモジュールへ進む。

**テスト名は文で書いてある。** `cargo test -p xylogue-xdm -- --list` のように並べれば、その層が何を約束して
いるかを名前だけで追える。

## Invariants

変更が壊してはいけないもの。いずれも意図的な選択であり、根拠は ROADMAP の決定表にある。

- **ツリーはアリーナ。** ノードは `Copy` な `NodeId` で指す。読み取りは `&Document`、変更は `&mut Document`。
  親ポインタによる循環を作らないこと、文書順を整数比較に保つことがこの形の目的である。
- **XPath は DOM を知らない。** 評価器は `Model` トレイトに対して動く。DOM への直接の依存を評価器やエンジンに
  持ち込まない。結果木断片や将来のストリーミング実装が同じ評価器で扱えなくなる。
- **パーサは I/O を持たない。** `Parser` はバイト列を与えられて前進する。同期・非同期のドライバはその上に載る。
- **言われなければ取りに行かず、書きに行かない。** 外部実体は `UriResolver`、スタイルシートモジュールは
  `Loader`、`document()` は `DocumentSource`、`exsl:document` は `ResultSink`。既定はいずれも「何もしない」で
  あり、黙って無視するのではなく理由を挙げて拒否する。XXE は設定項目ではない。
- **`unsafe` は禁止**（`unsafe_code = "forbid"`）。
- **クレート間依存は default features を off にする。** 各クレートは必要な feature を明示的に名指す。feature を
  全て落としたビルドも正当であり、CI がそれを組む。
- **実装していない構文は黙って飛ばさずエラーにする。** `element-available()` / `function-available()` は
  レジストリと実際の分岐に問い合わせて答える。一覧を手で同期させない。
- **印字したものは同じ木に解析される。** XPath 式の `Display` は解析結果を可視化するためにあり、別の木に読み
  戻される出力は不正とみなす。

## Conventions

- **書式**は `rustfmt.toml` に従う。120 桁、インデント 2 空白、改行は LF。CI が未整形を拒否する。
- **公開項目には doc コメントを付ける**（`missing_docs = "warn"`）。通常の利用に属するものには実行される
  `# Examples` を付ける。doctest は `cargo test` が走らせる。
- **仕様への言及は版固定 URL で書く。** `/TR/xml/` は動くが `/TR/2008/REC-xml-20081126/` は動かない。節番号は
  実装した規則の隣に書き、レビュアが原文と突き合わせられるようにする。
- **コメントは「何を」ではなく「なぜ」を書く。** コードが示していることを繰り返さない。選ばなかった選択肢と
  その理由、仕様のどの一文がそう決めているかを書く。
- **テスト名は主張を文で書く。** `a_doctype_is_reported_before_the_root_element_whatever_precedes_it` のように、
  失敗したときに何が壊れたかが名前で分かるようにする。
- **散文は少しヨーロッパ寄りの英語で書く。** 短縮形を使わず、格式を保つ。識別子と仕様からの引用は原綴のまま。
- **コミットはフェーズ単位。** 何を変えたかではなく、なぜそう変えたかと、何がそれを守るかを書く。

## Checks

CI と同じものをローカルで走らせられる。プッシュ前にこれを全て通す。

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace --all-features
cargo test --workspace --exclude xylogue-cli --exclude xylogue-fuzz --no-default-features
cargo doc --workspace --no-deps --all-features
cargo build --workspace --all-features    # MSRV 1.85 のツールチェインで
```

いずれも `RUSTFLAGS=-D warnings`（doc は `RUSTDOCFLAGS`）を付ける。**警告の有無で CI と食い違うため、
ツールチェインは最新の stable に保つ。**

外部資産を要するものは env var で指す。

```bash
# W3C XML 適合スイート（パーサと検証器）
XMLCONF=xmlconf cargo test -p xylogue-parser --test conformance -- --nocapture

# OASIS/Xalan XSLT 適合スイート
git clone --depth 1 https://github.com/apache/xalan-test.git xslt-conformance
XSLTCONF=xslt-conformance cargo test -p xylogue-xslt --test conformance -- --nocapture

# Java との差分テスト（JDK 11 以降が要る）
XYLOGUE_JAVA=java cargo test -p xylogue-xpath --test differential -- --nocapture

# 仕様が未規定の箇所について、この実装が何を返すかの実測レポート
cargo test -p xylogue --all-features --test behaviour -- --nocapture

# ベンチマーク
cargo bench -p xylogue
```

ファジングは nightly と cargo-fuzz を要し、**Windows では libFuzzer ランタイムが読み込めない**ため WSL か
Linux で走らせる。

```bash
./fuzz/short-run.sh 60
```

検査している性質そのものは `crates/xylogue-fuzz` にあり、種コーパスを同じ性質に通すテストは stable の
全プラットフォームで走る。ファザーが見つけた入力は種コーパスへ追加し、以後そのテストが再発を防ぐ。

**修正を入れたら、その修正を外してテストが落ちることを確かめる。** 通ることだけでは、そのテストが本当にその
欠陥を捕らえているかは分からない。

## What each layer is measured against

自前のテストのほかに何が支えているか。テストを足す場所を決めるときの材料になる。

| Layer | 外部の証拠 |
|---|---|
| `xylogue-parser` | W3C XML 適合スイート（整形式判定） |
| `xylogue-validate` | 同スイートの invalid 群。検出できない 8 件は理由付きで `KNOWN_DEVIATIONS` に記録 |
| `xylogue-xpath` | JDK の `javax.xml.xpath` との差分テスト、プロパティテスト、ファジング |
| `xylogue-xslt` | OASIS/Xalan 適合スイート |
| `xylogue-dom` / `xylogue-serialize` | ファジングの往復性質（書いたものが読み戻せ、同じ木になる） |
| `xylogue-xdm` / `xylogue-xinclude` / `xylogue-exslt` / `xylogue-cli` | 自前のテストのみ |

## Adding to the workspace

**クレートを足す場合。** ワークスペースの `[workspace.dependencies]` に `default-features = false` で登録し、
各利用側で必要な feature を名指す。ライブラリの feature を丸ごと名指すクレート（`xylogue-cli`、
`xylogue-fuzz`）は、CI の `--no-default-features` 実行から除外する。feature の合流で最小構成が組まれなくなる
ためである。Layout の表と `crates/xylogue/tests/guide.rs` も更新する。

**XSLT の命令を足す場合。** `engine.rs` の `instruction` に分岐を足し、同じ名前を `INSTRUCTIONS` にも足す。
`element-available()` はその配列から答える。**分岐の中身は関数呼び出しにする** — デバッグビルドでは分岐ごとの
局所変数がスタックフレームに確保され、再帰の 1 段ごとに命令セット全体を計上することになる。

**拡張関数を足す場合。** `Functions` に登録する。エンジンに組み込まない。EXSLT がその機構の最初の利用者で
あり、同じ道を通る。
