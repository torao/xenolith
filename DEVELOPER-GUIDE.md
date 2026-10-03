# Developer Guide

このリポジトリに手を入れる人のための方向づけ。見出しは英語、本文は日本語とする。

**設計判断の根拠はここには書かない。** [ROADMAP.md](ROADMAP.md) の §6「確定した設計方針」に決定 1〜10 として、
`xenolith` の判断は [`crates/xenolith/src/_design.rs`](crates/xenolith/src/_design.rs) に記録してあり、二重に
持てば必ずずれる。ここが持つのは「どこに何があるか」「壊してはいけないもの」「ここでの作法」の 3 つに限る。
合格率や行数といった数値も持たない。ROADMAP にあり、変わるものを複数箇所に置かない。

## Layout

7 クレート。依存は下から上への一方向で、各層は下の層しか知らない。

ワークスペースのメンバーは `xenolith` と `xenolith-fuzz` の 2 つだけである。ほかの 5 つは、`xenolith` の
イベント語彙が固まるまでワークスペースの外（ルートの `Cargo.toml` の `exclude`）に置いてあり、ビルドされず、
作り直す前の語彙のままである。語彙が固まったものから 1 つずつ戻す。

| Crate | 状態 | 責務 |
|---|---|---|
| `xenolith` | メンバー | 本体。エラーと位置、XML の文字クラス、インターンされた名前、RFC 3986 の URI（平場）、イベント語彙（producer / consumer / transformer、`Dispatcher`）・strict 検証・スキーマ非依存の `Schema`/`Validator` 契約と `xml:id` 検証（`event`）、XML 1.0 のプルパーサ・文字デコード・実体解決と書き出し（`WriterSource` と `XmlWriter`）（`io`）、DTD の宣言モデル・構文解析・検証器（`dtd`）、アリーナ木（`dom`）、パイプラインの一段としての XInclude（`xinclude`）、XPointer とそれで選ぶ filter（`xpointer`）、それらを繋いだ `Reader` / `Writer`（クレート直下） |
| `xenolith-fuzz` | メンバー | ファジングで検査する性質と、その種コーパス |
| `xenolith-xdm` | 外している | XPath データモデル。`Model` トレイトと DOM 実装 |
| `xenolith-xpath` | 外している | XPath 1.0。字句、構文、評価器、コア関数、拡張関数の登録機構 |
| `xenolith-xslt` | 外している | XSLT 1.0。パターン、スタイルシート、エンジン、`xsl:output` |
| `xenolith-exslt` | 外している | EXSLT 各モジュール。エンジンには組み込まず、拡張関数として登録する |
| `xenolith-cli` | 外している | コマンドライン。実行ファイル名は `xenolith` |

`fuzz/` はワークスペース外に置く。libFuzzer のターゲットは nightly を要するため、通常のビルドに巻き込まない。

この表は [`crates/xenolith/tests/guide.rs`](crates/xenolith/tests/guide.rs) が検査する。クレートを増減させた
まま表を放置すればテストが落ちる。

## Where to start reading

**`xenolith` の `event` から読む。** パーサ、木の走査、ビルダー、検証器、writer はすべてこの語彙
（`EventRef`、`EventProducer` / `EventConsumer`、`Dispatcher`、`Flow`）でつながる。ここが腑に落ちれば、
ほかのモジュールはその語彙の生産者か消費者として読める。

以降は `dom`（`build` と `DomSource`）→ `io::write` → `xinclude`（transformer の実例）の順。**`io::parse` は
最後でよい。** 最も難しいが、最も外部検証が効いており、実装を疑う理由ができるまで読む必要はない。

外しているクレートが戻ったら、そこは `xenolith-xdm` から読む。全クレート中で最も小さいが、XPath は DOM の上では
なく**データモデル**の上で動く、という設計の主張がそこにしかない。隣接テキストの併合、名前空間ノードの合成、
文書順の全順序。以降は `xenolith-xpath` → `xenolith-xslt` の順。

各クレートの `lib.rs` 冒頭は方向づけとして書いてある。まずそこを読み、そこが指すモジュールへ進む。

**テスト名は文で書いてある。** `cargo test -p xenolith -- --list` のように並べれば、その層が何を約束して
いるかを名前だけで追える。

## Invariants

変更が壊してはいけないもの。いずれも意図的な選択であり、根拠は ROADMAP の決定表にある。

- **ツリーはアリーナ。** ノードは `Copy` な `NodeId` で指す。読み取りは `&Document`、変更は `&mut Document`。
  親ポインタによる循環を作らないこと、文書順を整数比較に保つことがこの形の目的である。
- **XPath は DOM を知らない**（外しているクレート）。評価器は `Model` トレイトに対して動く。DOM への直接の依存を
  評価器やエンジンに持ち込まない。結果木断片や将来のストリーミング実装が同じ評価器で扱えなくなる。
- **パーサは I/O を持たない。** `Parser` はバイト列を与えられて前進する。同期・非同期のドライバはその上に載る。
- **言われなければ取りに行かず、書きに行かない。** 外部実体、外部 DTD サブセット、`xi:include` のリソースは
  `UriResolver`。外しているクレートでは、スタイルシートモジュールは `Loader`、`document()` は
  `DocumentSource`、`exsl:document` は `ResultSink`。既定はいずれも「何もしない」であり、黙って無視するのでは
  なく理由を挙げて拒否する。XXE は設定項目ではない。
- **API の境界で panic しない。** 入力の誤りも呼び出し側の誤りも `Err` で返す。panic は非公開の不変条件の
  破れに限る。
- **`unsafe` は禁止**（`unsafe_code = "forbid"`）。
- **クレート間依存は default features を off にする。** 各クレートは必要な feature を明示的に名指す。feature を
  全て落としたビルドも正当であり、CI がそれを組む。
- **実装していない構文は黙って飛ばさずエラーにする。** 評価できる part のない XPointer は `Error::XPointer` で
  拒否し、XInclude では resource error になる。外しているクレートでは、`element-available()` / `function-available()` は
  レジストリと実際の分岐に問い合わせて答える。一覧を手で同期させない。
- **印字したものは同じ木に解析される。** writer が書いたものは読み戻せ、同じテキストを書く（ファジングの
  性質）。外しているクレートでは、XPath 式の `Display` は解析結果を可視化するためにあり、別の木に読み戻される
  出力は不正とみなす。

## Conventions

- **書式**は `rustfmt.toml` に従う。120 桁、インデント 2 空白、改行は LF。CI が未整形を拒否する。
- **MSRV は 1.85。** let チェーン（`if let … && let …`）は使えない。CI が 1.85 でビルドする。
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
cargo test --workspace --exclude xenolith-fuzz --no-default-features
cargo test --workspace --exclude xenolith-fuzz --no-default-features --features encodings
cargo test -p xenolith --no-default-features --features async
cargo test -p xenolith --no-default-features --features tokio
cargo doc --workspace --no-deps --all-features
cargo build --workspace --all-features    # MSRV 1.85 のツールチェインで
```

いずれも `RUSTFLAGS=-D warnings`（doc は `RUSTDOCFLAGS`）を付ける。**警告の有無で CI と食い違うため、
ツールチェインは最新の stable に保つ。**

外部資産を要するものは env var で指す。

```bash
# W3C XML 適合スイート（パーサと検証器）
XMLCONF=xmlconf cargo test -p xenolith --test parser_conformance -- --nocapture
XMLCONF=xmlconf cargo test -p xenolith --all-features --test validate_conformance -- --nocapture
```

外しているクレートの検査は、そのクレートとともに戻す。OASIS/Xalan の XSLT 適合スイート（`xenolith-xslt`）と
Java との差分テスト（`xenolith-xpath`、JDK 11 以降が要る）がそれで、差分テストの CI ジョブは `if: false` で
止めてある。仕様が未規定の箇所の実測レポート（`behaviour`）とベンチマークは、イベントモデルへの統合で削除した
ため、今はない。

ファジングは nightly と cargo-fuzz を要し、**Windows では libFuzzer ランタイムが読み込めない**ため WSL か
Linux で走らせる。

```bash
./fuzz/short-run.sh 60
```

検査している性質そのものは `crates/xenolith-fuzz` にあり、種コーパスを同じ性質に通すテストは stable の
全プラットフォームで走る。ファザーが見つけた入力は種コーパスへ追加し、以後そのテストが再発を防ぐ。

**修正を入れたら、その修正を外してテストが落ちることを確かめる。** 通ることだけでは、そのテストが本当にその
欠陥を捕らえているかは分からない。

## What each layer is measured against

自前のテストのほかに何が支えているか。テストを足す場所を決めるときの材料になる。

| Layer | 外部の証拠 |
|---|---|
| `xenolith` の `io::parse` | W3C XML 適合スイート（整形式判定） |
| `xenolith` の `dtd::validate` | 同スイートの invalid 群。検出できない 8 件は理由付きで `KNOWN_DEVIATIONS` に記録 |
| `xenolith` の `dom` / `io::write` | ファジングの往復性質（書いたものが読み戻せ、同じテキストを書く） |
| `xenolith` の `xinclude` | 自前のテストのみ |
| `xenolith-xpath`（外している） | JDK の `javax.xml.xpath` との差分テスト、プロパティテスト、ファジング。いずれも今は止まっている |
| `xenolith-xslt`（外している） | OASIS/Xalan 適合スイート。今は止まっている |
| `xenolith-xdm` / `xenolith-exslt` / `xenolith-cli`（外している） | 自前のテストのみ |

## Adding to the workspace

**クレートを足す場合。** ワークスペースの `[workspace.dependencies]` に `default-features = false` で登録し、
各利用側で必要な feature を名指す。ライブラリの feature を丸ごと名指すクレート（`xenolith-cli`、
`xenolith-fuzz`）は、CI の `--no-default-features` 実行から除外する。feature の合流で最小構成が組まれなくなる
ためである。Layout の表と `crates/xenolith/tests/guide.rs` も更新する。

**クレートを戻す場合。** ルートの `Cargo.toml` の `exclude` から `members` へ移し、今の語彙へ移行する。
そのクレートとともに止めた CI の手順（`.github/workflows/ci.yml` のコメントに残してある）も戻す。Layout の表の
状態も更新する。

**XSLT の命令を足す場合**（外しているクレート）。 `engine.rs` の `instruction` に分岐を足し、同じ名前を `INSTRUCTIONS` にも足す。
`element-available()` はその配列から答える。**分岐の中身は関数呼び出しにする** — デバッグビルドでは分岐ごとの
局所変数がスタックフレームに確保され、再帰の 1 段ごとに命令セット全体を計上することになる。

**拡張関数を足す場合**（外しているクレート）。 `Functions` に登録する。エンジンに組み込まない。EXSLT がその機構の最初の利用者で
あり、同じ道を通る。
