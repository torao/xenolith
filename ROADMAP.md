# xylograph ロードマップ

Rust による XML 1.0 / XPath 1.0 / XSLT 1.0 の実装計画。Java の XML API（JAXP: DOM, SAX, StAX, javax.xml.xpath, javax.xml.transform）相当の機能セットを目標とする。

---

## 1. スコープ

### 準拠仕様

実装の根拠となる文書は**版を固定した URL**で示す（決定 10）。「最新版」URL ではなく日付入り URL を使うのは、レビューアが「実装時にどの本文を読んだか」を後から確認できるようにするため。

| 仕様 | 版・勧告日 | 位置づけ |
|---|---|---|
| [XML 1.0 (Fifth Edition)](https://www.w3.org/TR/2008/REC-xml-20081126/) | W3C REC 2008-11-26 | 必須（ゴール） |
| [Namespaces in XML 1.0 (Third Edition)](https://www.w3.org/TR/2009/REC-xml-names-20091208/) | W3C REC 2009-12-08 | 必須（XPath/XSLT の前提） |
| [XPath 1.0](https://www.w3.org/TR/1999/REC-xpath-19991116/) | W3C REC 1999-11-16 | 必須（XSLT の前提） |
| [XSLT 1.0](https://www.w3.org/TR/1999/REC-xslt-19991116) | W3C REC 1999-11-16 | 必須（ゴール） |
| XML Serialization (xsl:output の xml/html/text) | [XSLT 1.0 §16](https://www.w3.org/TR/1999/REC-xslt-19991116#output) | 必須 |
| DTD 妥当性検証（Validity Constraints 全件） | [XML 1.0 §2–§5](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-documents) | **必須**（決定 1、`xylograph-validate`） |
| [DOM Level 3 Core](https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/) | W3C REC 2004-04-07 | インタフェースを踏襲（決定 3） |
| [XInclude 1.0 (Second Edition)](https://www.w3.org/TR/2006/REC-xinclude-20061115/) | W3C REC 2006-11-15 | **必須**（決定 6、feature + 実行時切替） |
| [XPointer Framework](https://www.w3.org/TR/2003/REC-xptr-framework-20030325/) / [`element()`](https://www.w3.org/TR/2003/REC-xptr-element-20030325/) / [`xmlns()`](https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/) | W3C REC 2003-03-25 | 必須（XInclude の `@xpointer` に要る） |
| [XML Base (Second Edition)](https://www.w3.org/TR/2009/REC-xmlbase-20090128/) | W3C REC 2009-01-28 | **必須**（決定 6） |
| [xml:id 1.0](https://www.w3.org/TR/2005/REC-xml-id-20050909/) | W3C REC 2005-09-09 | **必須**（決定 6） |
| [RFC 3986 (URI)](https://www.rfc-editor.org/rfc/rfc3986) | IETF STD 66, 2005-01 | 必須（基底 URI 解決） |
| W3C XML Schema (XSD) 1.0 | W3C REC | **将来トラック**（決定 8、設計の余地のみ確保） |
| RELAX NG | ISO/IEC 19757-2 | **将来トラック**（決定 8、微分アルゴリズムで `Validator` に嵌まる） |
| EXSLT (common / strings / math / sets / dates-and-times) | 業界標準（W3C 勧告ではない） | **必須**（決定 5） |
| HTML / loose parsing | WHATWG | **対象外**（下記） |
| XML 1.1 / Namespaces 1.1 | — | 対象外 |
| XPath 2.0+ / XSLT 2.0+ | — | 対象外 |
| XSD 1.1 | — | 対象外（XSD 1.0 の設計余地には乗る） |

表中の URL は実際に取得して題名・版・勧告日が一致することを確認済み。XInclude は当初「3rd Edition」と記していたが、**存在するのは Second Edition (2006) までである**ことを確認して訂正した。

### 非対象: HTML / loose parsing

「XML として不正な HTML を寛容にパースする」機能は**実装しない**。パーサは常に整形式を要求する。

理由:

- スクレイピング用途で意味があるのは「ブラウザと同じ木」であって「エラーを飲み込む木」ではない。WHATWG の tree construction は insertion mode が 20 以上あり、`<table>` 内の `<tbody>` 暗黙挿入や adoption agency algorithm（誤ネストした装飾要素の再構築）まで含む。**中途半端な寛容さは、ブラウザで確認した XPath が静かに違う結果を返す**という最悪の形で表面化する
- その完全準拠版は `html5ever` が既に提供している。再実装の価値がない

**HTML を扱いたい場合**: Phase 3 で DOM が入った後、`html5ever` の出力を xylograph の DOM に流し込めば、XPath / XSLT 資産はそのまま使える。パーサは strict なまま保てる。これは利用側の統合であって、本ロードマップの作業項目ではない。

### 非機能要件（初期から意識するもの）

- **セキュリティ**: XXE、外部エンティティ参照の既定無効化、entity expansion 爆弾（billion laughs）対策、実体展開・再帰深さ・ノード数の上限。JAXP の `FEATURE_SECURE_PROCESSING` 相当を既定 ON。
- **`no_unsafe`（原則）** と `#![forbid(unsafe_code)]`。依存は「自前で書くと仕様準拠が保証できないもの」に限る（Unicode 照合と文字符号化。決定 2・4）。
- 重量級の依存（ICU, encoding_rs）は **feature フラグで切り離せる**こと。既定 ON、`--no-default-features` で UTF-* + コードポイント順のみの最小構成になる。
- **エラー**は panic させず `Result` で返す。位置情報（行・列・システム ID）を必ず持つ。
- **ストリーミング**とツリー構築の両方を提供（SAX/StAX 相当とDOM相当）。

---

## 2. 機能の洗い出し

### 2.1 XML 1.0 パーサ層

<details open>
<summary>字句・構文</summary>

- 文字範囲チェック（`Char` production、不正な制御文字の拒否）
- `Name` / `NmToken` / `NCName` の production（XML 1.0 **5th ed** は Unicode 由来の定義。4th ed とは別物なので注意）
- XML 宣言、テキスト宣言（外部実体の `<?xml version encoding?>`）
- 要素・属性・空要素タグ、タグの対応チェック
- 属性値正規化（CDATA とそれ以外で規則が異なる → DTD 情報が必要）
- 文字参照 `&#nn;` / `&#xnn;`、定義済み実体、一般実体、パラメータ実体
- CDATA セクション、コメント、処理命令
- `xml:space` / `xml:lang` の解釈
- 改行正規化（`\r\n`, `\r` → `\n`）

</details>

<details open>
<summary>DTD（妥当性検証まで含めて必須 — 決定 1）</summary>

まず、「妥当性検証をしない」場合でも以下のために DTD 処理そのものが要る:

- 既定属性値の補完（XPath から属性が見える／見えないが変わる）
- 属性型 `ID`（`id()` 関数と `xsl:key` 相当の挙動）、`IDREF`、`NMTOKEN`、`ENTITY`
- 記法宣言・未解析実体（`unparsed-entity-uri()`）
- 属性値正規化規則の切り替え

実装項目:

- 内部／外部サブセット、`ELEMENT` / `ATTLIST` / `ENTITY` / `NOTATION` 宣言
- パラメータ実体の展開（宣言中への埋め込みを含む。ここが最も厄介）
- 条件セクション `INCLUDE` / `IGNORE`
- `standalone="yes"` の制約チェック

そのうえで **妥当性検証（validating parser）** を実装する:

- 内容モデル `EMPTY` / `ANY` / Mixed / children の照合。children は正規表現を **DFA へコンパイル**して照合する
- **決定性制約（VC: Deterministic Content Model, 付録 E）** の検査。NFA→DFA 変換時に曖昧さを検出する形で自然に実装できる
- 要素・属性の宣言済みチェック、属性の必須／`#FIXED` 一致
- 列挙型・`NMTOKEN(S)` / `ENTITY(IES)` / `NOTATION` の値検査
- `ID` の一意性、`IDREF(S)` の解決先存在（文書末での遅延検査）
- ルート要素名と DOCTYPE 名の一致
- 妥当性エラーは **recoverable**（`ErrorListener` に通知して継続可能）とし、整形式エラーは fatal とする — Java の `DocumentBuilderFactory.setValidating(true)` と同じ切り分け

</details>

<details open>
<summary>非 UTF エンコーディング（決定 4）</summary>

- 自前実装は UTF-8 / UTF-16 (LE/BE) / US-ASCII / ISO-8859-1 まで
- それ以外（Shift_JIS, EUC-JP, ISO-2022-JP, windows-125x, GBK, Big5 …）は **`encoding_rs` に委譲**。`feature = "encodings"`（既定 ON）
- 境界は `trait Decoder { fn decode(&mut self, src: &[u8], dst: &mut String) -> DecodeResult }` として抽象化し、`encoding_rs` はその一実装に留める（将来の差し替えとビルドサイズ削減のため）
- 出力側も同様に `xsl:output/@encoding` を `encoding_rs` のエンコーダへ委譲。**符号化できない文字は文字参照へフォールバック**する（XSLT 1.0 §16 の要求）

</details>

<details open>
<summary>入出力</summary>

- エンコーディング自動判定（BOM、`<?xml encoding=`）
- 外部実体・DTD の解決（Java の `EntityResolver` 相当のトレイト）。既定は解決拒否
- 相対 URI 解決（RFC 3986）と base URI 管理 → `document()` に必要

</details>

<details open>
<summary>名前空間</summary>

- 接頭辞スコープ管理、既定名前空間、`xmlns=""` による解除
- 予約接頭辞 `xml` / `xmlns` の扱い、名前空間名の妥当性
- 名前空間ノード（XPath データモデル固有。DOM には存在しない）の生成

</details>

### 2.2 API 層（Java パリティ）

| Java | xylograph 対応 | 備考 |
|---|---|---|
| SAX2 `ContentHandler` ほか | `Handler` トレイト（push） | 実装は pull の上に薄く載せる |
| StAX `XMLStreamReader` | `Reader` イテレータ（pull） | **こちらを一次 API とする** |
| StAX `XMLStreamWriter` | `Writer` | シリアライザと共用 |
| DOM Level 3 Core | `dom` モジュール | **W3C IDL をそのまま踏襲**（決定 3） |
| `DocumentBuilderFactory` | `DocumentBuilder` + ビルダーパターン | |
| `javax.xml.xpath` | `XPath::compile` / `evaluate` | |
| `javax.xml.transform` | `Transformer`, `Source`, `Result` | Stream/DOM/SAX の各 Source/Result |
| `URIResolver` | `UriResolver` トレイト | `document()`, `xsl:import/include` |
| `ErrorListener` | `ErrorListener` トレイト | warning / error(recoverable) / fatalError の 3 段。妥当性エラーはここへ |

**DOM の忠実度（決定 3）**: W3C DOM Level 3 Core のインタフェース（`Node`, `Document`, `Element`, `Attr`, `CharacterData`, `NodeList`, `NamedNodeMap`, `DOMException` …）はメソッド名・引数・例外コードまで規定どおりに写す。名前だけ Rust の慣習に寄せる（`getNodeName` → `node_name`、getter/setter は `foo()`/`set_foo()`）。

W3C が規定していない部分（パーサ設定、変換の駆動、XPath 式のコンパイル、エラー通知、リゾルバ、CLI）は Rust らしく再設計する:

- ファクトリ地獄（`DocumentBuilderFactory.setFeature(String, bool)`）は使わず、型付きのビルダー（`ParserConfig::new().validating(true).external_entities(false)`）
- 例外ではなく `Result<T, DomError>`。ただし `DomError` は DOM の例外コード（`HIERARCHY_REQUEST_ERR` 等）を保持する
- `NodeList` は Rust のイテレータも実装する（`item(i)` も残す）
- ライブ性: `NodeList` / `NamedNodeMap` の live 性は仕様どおり維持する（アリーナ + 世代カウンタで実現）

### 2.3 XPath 1.0

- **字句解析の特殊規則**: `div`/`mod`/`and`/`or` と名前の曖昧性、`*` の乗算とワイルドカードの区別（直前トークンによる文脈依存規則）
- 文法: `LocationPath`, `FilterExpr`, 述語、`|`、比較・算術・論理演算子、略記構文（`//`, `.`, `..`, `@`）
- **13 軸**: child, descendant, parent, ancestor, following-sibling, preceding-sibling, following, preceding, attribute, namespace, self, descendant-or-self, ancestor-or-self
- ノードテスト: 名前テスト、`node()`, `text()`, `comment()`, `processing-instruction()`
- 4 つのデータ型: node-set / boolean / number(IEEE 754 double, NaN) / string と相互変換規則
- **比較演算のノードセット意味論**（存在量化）
- 文書順・逆文書順、述語における position/last の軸方向依存
- コア関数ライブラリ 27 個
- 動的コンテキスト（コンテキストノード・位置・サイズ、変数束縛、関数ライブラリ、名前空間宣言）
- ユーザ定義関数の拡張点（`function-available()` と連動）

### 2.4 XSLT 1.0

<details open>
<summary>スタイルシート処理</summary>

- `xsl:stylesheet` / `xsl:transform`、簡略構文（リテラル結果要素をスタイルシートとする形式）
- `xsl:import` / `xsl:include`、**インポート優先順位**の計算
- 前方互換処理（`version > 1.0`）、`xsl:fallback`
- 拡張要素・拡張関数（`extension-element-prefixes`）、`element-available()` / `function-available()`
- `exclude-result-prefixes`、`xsl:namespace-alias`
- スタイルシート自身の空白除去（`xsl:text` 以外の空白テキストノード除去）
- 属性値テンプレート `{...}`

</details>

<details open>
<summary>テンプレートと実行</summary>

- パターン言語（XPath のサブセット + `id()` / `key()`）とマッチ判定
- **既定優先度の算出**（-0.5 / -0.25 / 0 / 0.5）と競合解決、`xsl:template/@priority`
- モード、組み込みテンプレート規則
- `xsl:apply-templates`, `xsl:apply-imports`, `xsl:call-template`
- `xsl:param` / `xsl:with-param` / `xsl:variable`、スコープと循環参照検出
- **結果ツリーフラグメント（RTF）** — XSLT 1.0 固有の型。`node-set()` 拡張（EXSLT）の扱いも決める
- `xsl:for-each`, `xsl:if`, `xsl:choose`/`when`/`otherwise`
- `xsl:sort`（`data-type`, `order`, `case-order`, `lang`）— **ICU による照合**（決定 2、下記）。安定ソートであること
- `xsl:key` と `key()`
- `xsl:number`（`level=single/multiple/any`, `count`, `from`, `format`, `lang`, `letter-value`, `grouping-separator/size`）
- `xsl:decimal-format` と `format-number()` のピクチャ文字列
- `xsl:message`（`terminate="yes"`）
- `document()`（複数引数・base URI・断片識別子・同一 URI のノード同一性保証）
- `generate-id()`（同一変換内で安定、ノード同一性と 1:1）、`current()`, `system-property()`, `unparsed-entity-uri()`

</details>

<details open>
<summary>出力</summary>

- `xsl:output`: `method` = xml / html / text、`version`, `encoding`, `omit-xml-declaration`, `standalone`, `doctype-public/system`, `cdata-section-elements`, `indent`, `media-type`
- 出力メソッドの既定判定（結果ツリーのルート要素が `html` なら HTML メソッド）
- **HTML 出力規則**: 空要素タグ、真偽属性の最小化、`<script>`/`<style>` 内の非エスケープ、URI 属性の %エスケープ、meta の挿入
- `disable-output-escaping`（DOM/SAX 出力時は無視される旨の規定を含む）
- **名前空間の修復（namespace fixup）**: `xsl:element`/`xsl:copy` 等で生成した要素に必要な宣言を出力時に補う
- `xsl:strip-space` / `xsl:preserve-space` とソースツリーの空白除去（`xml:space` との相互作用）
- `xsl:attribute-set`、`xsl:copy` / `xsl:copy-of` / `xsl:element` / `xsl:attribute` / `xsl:text` / `xsl:comment` / `xsl:processing-instruction`

</details>

<details open>
<summary>照合順序 / ICU（決定 2）</summary>

`xsl:sort` の `lang` と `case-order` は Java の `java.text.Collator`（CLDR 準拠）と揃える必要がある。**`icu_collator`（ICU4X）に依存する**。

- `feature = "icu"`（既定 ON）。OFF 時は Unicode コードポイント順にフォールバックし、`lang` 指定は警告を出して無視する
- CLDR データはコンパイル時同梱（ICU4X の `DataProvider`）。バイナリサイズが問題になる場合はロケール絞り込みビルドを用意する
- `case-order="upper-first"/"lower-first"` は ICU の caseFirst 設定へ写す
- `lang()` 関数（XPath）と `xml:lang` の言語タグ照合にも BCP 47 パーサとして流用する
- ICU4X は `no_unsafe` 方針と両立する（純 Rust）

</details>

<details open>
<summary>EXSLT（決定 5 — 最初から入れる）</summary>

XSLT 1.0 の実用上ほぼ必須。名前空間ごとにモジュール化し、`function-available()` / `element-available()` に正しく応答する。

| モジュール | 名前空間 | 優先度 |
|---|---|---|
| `exsl:node-set()`, `exsl:object-type()`, `exsl:document` | `http://exslt.org/common` | **最優先**（RTF → node-set 変換。これが無いと二段変換が書けない） |
| strings | `http://exslt.org/strings` | 高（`tokenize`, `replace`, `split`, `padding`） |
| math | `http://exslt.org/math` | 高（`max`, `min`, `highest`, `lowest`, `abs`, `power`） |
| sets | `http://exslt.org/sets` | 高（`difference`, `intersection`, `distinct`, `has-same-node`, `leading`, `trailing`） |
| dates-and-times | `http://exslt.org/dates-and-times` | 中（ISO 8601 の実装量が大きい。時刻取得は決定的テストのため注入可能にする） |
| functions (`func:function`/`func:result`) | `http://exslt.org/functions` | 中（ユーザ定義関数。拡張関数機構の実証にもなる） |
| regular-expressions | `http://exslt.org/regular-expressions` | 低（JavaScript 正規表現準拠が要件。`regex` クレートとは意味論が異なる点に注意） |

`exsl:node-set()` を実装する以上、**RTF は「node-set へ昇格できる独立ツリー」として設計する**必要がある（§3 の設計判断 6 と直結）。

</details>

### 2.5 XInclude / XML Base / xml:id（決定 6）

いずれも **パース結果のツリーに対する後処理層**として実装し、Cargo feature と実行時フラグの両方で有効・無効を切り替えられるようにする。

<details open>
<summary>XML Base</summary>

- `xml:base` 属性による基底 URI の上書き。ノードごとの基底 URI = 祖先の `xml:base` を RFC 3986 で順に解決したもの
- 基底 URI の起点は「実体の URI」であって「文書の URI」ではない（外部実体内の相対 URI は当該実体からの相対）。**パーサが実体ごとの system ID をツリーに残す必要がある** → Phase 1 の実体スタックに system ID を持たせておく
- 影響範囲: XInclude の `@href` 解決、XSLT の `document()` / `xsl:import` / `xsl:include`、`unparsed-entity-uri()`
- 実装コストは小さいが、**XInclude より先に必要**。基底 URI 追跡自体は常時 ON（無効化できるのは `xml:base` 属性による上書きの解釈のみ）

</details>

<details open>
<summary>xml:id</summary>

- `xml:id` 属性を DTD 宣言なしに ID 型として扱う。`id()` 関数・XPointer の短縮ポインタ・DOM の `getElementById` から見える
- 値は `NCName` でなければならない。違反は **「xml:id error」** であり整形式エラーではない（報告して ID 扱いを取り消す）
- 属性値正規化は宣言型に関わらず ID 相当（前後空白の除去）を行う
- DTD で `xml:id` に `ID` 以外の型が宣言されていた場合も xml:id error
- 一意性違反の検出（DTD 検証の ID 一意性チェックと同じ機構を共用）

</details>

<details open>
<summary>XInclude 1.0</summary>

- `xi:include` の `href` / `parse`(`xml`|`text`) / `xpointer` / `encoding` / `accept` / `accept-language`、`xi:fallback`
- `href` 省略時は同一文書内参照（`xpointer` 必須）
- **XPointer**: 短縮ポインタ（bare name → ID を持つ要素。**xml:id / DTD の ID 型に依存する**）、`element()` scheme、`xmlns()` scheme。`xpointer()` scheme は対象外とし、未知 scheme は仕様どおり次の候補へフォールバック
- **base URI fixup**: 取り込んだ要素に `xml:base` を付与して元の基底 URI を保存する（無効化オプションあり）
- **language fixup**: 同様に `xml:lang` を保存する
- **再帰的処理**と**インクルージョンループの検出**（`href` の絶対 URI + xpointer をスタックで追跡）
- エラー時は最も内側の `xi:fallback` へ。fallback が無ければ致命的エラー
- `parse="text"` のときの符号化決定（`@encoding` → BOM → 既定 UTF-8）と不正文字の扱い
- 取り込み結果の整合性: 属性ノードや複数トップレベル要素が来た場合の制約、DOCTYPE の扱い
- **セキュリティ**: 外部エンティティと同じリスク（SSRF / ローカルファイル読取）。`UriResolver` を必ず経由し、**実行時の既定は無効**（JAXP の `setXIncludeAware(false)` に合わせる）

</details>

<details open>
<summary>有効化の方式</summary>

**2 段階**にする。Cargo feature でコードごと落とせるようにしつつ、リンクされていれば実行時に切り替えられる。

```rust
// Cargo.toml: default = ["xinclude", "xml-base", "xml-id", ...]

let doc = ParserConfig::new()
    .xinclude(XIncludeConfig::new()          // 実行時に有効化。既定は無効
        .base_uri_fixup(true)
        .language_fixup(true)
        .max_depth(64))
    .xml_id(true)                            // 既定 有効
    .xml_base(true)                          // 既定 有効
    .uri_resolver(my_resolver)               // XInclude を有効にするなら実質必須
    .parse_file("doc.xml")?;
```

- feature OFF のままビルドしたバイナリで実行時に有効化しようとした場合は、黙って無視せず `Err(UnsupportedFeature)` を返す
- XSLT 側からも同じ設定を通す（`TransformerConfig` → ソース文書と `document()` が読む文書の両方に適用）
- CLI は `--xinclude` / `--no-xml-id` 等のフラグで対応
- `system-property()` および `element-available()` 相当の問い合わせで、有効化状態を検査できるようにする

</details>

---

## 3. アーキテクチャ

### クレート構成（Cargo ワークスペース）

```
xylograph/
├── xylograph-core/       # QName, 名前プール, URI, エラー, 文字クラス, エンコーディング
├── xylograph-parser/     # pull パーサ, DTD の構文解析, 実体解決, 名前空間スタック
├── xylograph-validate/   # 検証レイヤー: スキーマ非依存の Validator/ErrorListener + DTD 検証器
├── xylograph-relaxng/    # 将来トラック: RELAX NG 検証器（微分アルゴリズム、Validator を実装、決定 8）
├── xylograph-xsd/        # 将来トラック: XSD 検証器（Validator を実装、決定 8）
├── xylograph-dom/        # DOM Level 3 Core（W3C IDL 準拠）のツリー
├── xylograph-xinclude/   # XInclude + XPointer(framework/element/xmlns)。ツリー後処理
├── xylograph-xdm/        # XPath データモデルのビュー（DOM 等の背後実装を抽象化するトレイト）
├── xylograph-xpath/      # 字句・構文・意味解析・評価器・コア関数
├── xylograph-serialize/  # xml/html/text シリアライザ, 名前空間修復
├── xylograph-xslt/       # スタイルシートコンパイラ + 実行器
├── xylograph-exslt/      # EXSLT 各モジュール（拡張関数機構の上に載る）
├── xylograph-cli/        # xylo transform / xylo xpath / xylo validate
└── xylograph/            # ファサード（JAXP 相当の入口）
```

### 主要な設計判断（Phase 0 で確定させる）

1. **ツリー表現**: アリーナ（`Vec<NodeData>` + `NodeId(u32)`）を採用する。`Rc<RefCell<Node>>` は DOM の親ポインタで循環参照になり、XPath の文書順比較も遅い。アリーナなら文書順は「(ドキュメントID, 進入順序番号)」の整数比較で O(1)、`generate-id()` も自明に実装できる。
   - 代償: DOM API の「ノードを他文書へ移動」「ノードだけを保持し続ける」が素直に書けない。**採用（Phase 3a で確定）: コンテキスト方式**。アリーナは `Document` が所有し、読み取り・走査は `&Document`、変更は `&mut Document` 経由（`doc.append_child(parent, child)` など）。`tag_name()` 等が `&str` を返し高速・イディオマティック（indextree / xot 系）。ノードを単独で持ち回る必要（XPath/XSLT の読み取り主体フェーズ）は、`Arc<Document>` を束ねた自己完結ハンドル `NodeRef` で満たす。ツリーは一意アクセスで構築・変更し、読み取り時に `Arc` で共有する。
2. **文字列**: 名前（要素名・属性名・名前空間 URI）はインターン。テキストは入力バッファからの借用（`Cow`）をパーサ層では許し、DOM 構築時に所有へ倒す。
3. **XDM とツリーの分離**: XPath 評価器は `trait Node` に対して動く。これにより DOM ツリー・RTF・将来のストリーミング実装を同じ評価器で扱える。名前空間ノードは XDM 側で合成する（DOM には持たせない）。
4. **XPath は「コンパイル」する**: AST をそのまま歩くのではなく、軸走査＋述語の閉包（あるいは簡易バイトコード）へ落とす。`xsl:key` とパターンマッチはここで最適化余地が大きい。
5. **パターンのインデックス化**: テンプレート規則は「最下位ステップの名前」でバケットに分け、候補を絞ってから優先度順に照合する。
6. **RTF の表現**: 独立した小さなアリーナ（`DocumentFragment` ではなく専用ルート）とし、型システム上 node-set と区別する。ただし `exsl:node-set()`（決定 5）でゼロコピーに node-set へ昇格できるよう、**内部表現は通常のツリーと同一**にして型タグだけで区別する。
7. **拡張関数の登録機構を先に作る**: EXSLT を最初から入れる（決定 5）なら、EXSLT 自身をその機構の最初の利用者にする。組み込みハードコードにしない。
8. **パーサ本体は I/O を持たない（Sans-I/O）**: 決定 7。下記。
9. **検証はスキーマ非依存レイヤー（決定 8）**: 検証は DTD だけではない。`xylograph-validate` に**スキーマ言語に依存しない `Validator` / `ErrorListener`** を置き、DTD 検証器をその最初の実装とする。妥当性エラーは recoverable、整形式エラーは fatal（Java の `setValidating(true)` と同じ切り分け）。XSD は同じ `Validator` を実装する将来トラック（`xylograph-xsd`、本線 XSLT 1.0 完了後、まず Structures + 主要 datatypes の実用サブセット、identity constraint / redefine / PSVI は当初除外）。この設計により XSD を後付けしても既存を作り直さない。

---

## 4. 想定される難所

| 項目 | なぜ難しいか |
|---|---|
| パラメータ実体の展開 | 宣言のテキスト中に展開され、字句境界をまたぐ。パーサを実体スタック上の文字ストリームとして書き直す必要がある。後から足すのは高コスト → **最初から実体スタック前提で設計する** |
| 属性値正規化 | DTD の属性型に依存するため、DTD 処理と本文パースの順序結合がある |
| XPath の字句規則 | 文脈依存。トークナイザに直前トークンの状態を持たせる |
| ノードセット比較 | `=` / `!=` / `<` の意味論が直感に反する（`!=` は `not(=)` ではない） |
| 内容モデルの決定性検査 | 付録 E の制約。NFA→DFA 変換で曖昧さを検出する実装にすれば照合器と一体で作れるが、`(a|b)*a` のような例を正しく弾けるか要テスト |
| 検証と実体展開の相互作用 | 内容モデル照合は実体境界をまたぐ。VC: Proper Group/PE Nesting など、実体の境界そのものを制約するものがある |
| ICU4X のデータサイズ | CLDR 全ロケール同梱はバイナリを肥大させる。feature とロケール絞り込みの設計が要る |
| 基底 URI の起点 | 「文書の URI」ではなく「その実体の URI」が起点。実体ごとの system ID をツリーに残す設計を Phase 1 で入れておかないと後付けが高くつく |
| XInclude の base URI fixup | 取り込んだ木に `xml:base` を挿入するため、**変換結果や再直列化で属性が増える**。テストの期待値比較で必ず問題になるので、fixup の ON/OFF を最初からテスト軸に入れる |
| XInclude と検証の順序 | 妥当性検証は XInclude 展開の**前**に行う（仕様上、展開後の木は元の DTD に照らして妥当とは限らない）。設定の組み合わせを明示的に定義する |
| `format-number()` | ピクチャ文字列の解析と丸め。Java の `DecimalFormat` 準拠が要求される |
| `xsl:number` の `level="any"` | 素朴に書くと文書全体の走査で O(n²) になる |
| HTML 出力メソッド | 規定が細かく、テストスイートでの差分が出やすい |
| 名前空間修復 | `xsl:element` / `xsl:copy` / リテラル結果要素で生成された木に対し、出力時点で宣言を推論する |
| `document()` のノード同一性 | 同一絶対 URI は同一ノードを返さねばならない → 変換単位のドキュメントプールが要る |
| 実体展開爆弾 | 展開回数・総文字数の予算制を最初から入れる |

---

## 5. ロードマップ

各フェーズは「テストが通ること」を完了条件とする。W3C XML Conformance Test Suite（xmlconf）と XSLT 1.0 テストスイート（OASIS / Xalan）を早期に取り込む。

### Phase 0 — 基盤（前提） ✅ 完了

- ワークスペース（edition 2024 / MSRV 1.85 / MIT OR Apache-2.0）、`unsafe_code = "forbid"`
- CI: fmt / clippy(`-D warnings`) / doc / 3 OS テスト / feature 組み合わせ / MSRV ビルド
- `xylograph-core`
  - `error`: `Error` / `ErrorKind` / `Severity` / `Location`（**実体**単位の system ID + 行・列・オフセット）
  - `chars`: XML 1.0 5th ed の `Char` / `NameStartChar` / `NameChar` / `PubidChar`、`Name` / `NCName` / `Nmtoken` 検証、QName 分解
  - `name`: `NamePool`（インターン、予約名の `NameId` は定数）、`ExpandedName`、`QName`
  - `uri`: `UriReference` と RFC 3986 §5.3 の解決（§5.4 の全例をテスト）、`escape_uri`
  - `encoding`: `Decoder` トレイト、UTF-8 / UTF-16 / US-ASCII / ISO-8859-1 の自前実装、`encoding_rs` バックエンド（feature `encodings`）、Appendix F の符号化判定
- **成果物**: 何もパースしないが、他クレートが依存できる土台（テスト 42 件）

### Phase 1 — 整形式 XML の pull パーサ

**1a. 文字ストリームと実体スタック** ✅ 完了
- `CharStream`: `Decoder` の上に載る文字ソース。符号化判定（Appendix F）、`\r\n` / `\r` の改行正規化（**チャンク境界をまたぐ CR LF を含む**）、`Char` 検査、行・列・オフセット追跡、消費済みバッファの圧縮
- **消費は `advance` を呼ぶまで起きない**。不完全トークンは何もせず次の入力を待ち、トークン先頭から再スキャンする（決定 7 の再開方式）
- `Entity` / `EntityStack`: 実体ごとの system ID と基底 URI、位置報告は最内実体、`Limits`（深さ・展開回数・展開文字数）、WFC "No Recursion" の検出
- **成果物**: `xylograph-parser` クレート（テスト 26 + doctest 7）

**1b. Sans-I/O トークナイザ／パーサコア** ✅ 完了
- `scan`: トークン境界の探索のみを行う純粋関数。未完なら何も消費せず「入力不足」を返す
- `Parser`: `feed` / `advance` と `Progress`（`Event` / `NeedMoreInput` / `Eof`。`NeedEntity` は Phase 2 で追加するため `#[non_exhaustive]`）
- 要素・属性・テキスト・CDATA・コメント・PI・DOCTYPE（Phase 2 まで未解釈のまま保持）・XML 宣言
- 文字参照と定義済み実体、属性値正規化（§3.3.3）、名前空間解決、`xml:space` / `xml:lang`
- 整形式制約: タグの対応、単一ルート、属性の一意性、予約接頭辞、`]]>`／`--`、参照の妥当性
- **入力の分割方法によって結果が変わらないこと**を全ケースで検証（1・2・3・7 バイト刻み）
- **成果物**: テスト 69 + doctest 10

**1c. ドライバとイベント API** ✅ 完了
- カーソル API（`Parser` のアクセサが借用を返す）を一次、`Event`（所有）と `Iterator` をその上に
- `Reader<R: Read>` 同期ドライバ、`AsyncReader<R: AsyncRead>`（feature `tokio`、既定 OFF）
- `Limits::max_element_depth` を追加（要素ネストの上限。1b では未保護だった）
- **入力の刻み方とドライバの種類によって結果が変わらないこと**を全ケースで検証（1・2・3・5・64 バイト刻み × 3 ドライバ）
- xmlconf ハーネス（`XMLCONF` 環境変数で実行、CI では毎回取得）。DOCTYPE を含むケースは Phase 2 まで skip として計上
- **成果物**: テスト 92 + 統合 12 + doctest 16

### Phase 2 — DTD 処理（2a）と妥当性検証（2b）

**2a-i. 内部サブセットと実体解決** ✅ 完了
- `dtd` モジュール: `Dtd` データモデルと内部サブセットパーサ。実体・要素・属性リスト・記法宣言、**内容モデルの厳密パース**（`children`/`Mixed`/`choice`/`seq`、混在の誤りを検出）、内部パラメータ実体の展開（宣言間のみ、WFC: PEs in Internal Subset）
- 一般実体の解決: **本文では実体スタックに push**（マークアップを含む実体を透過展開、テキストは実体境界をまたいで連結）、**属性値では再帰文字列展開**。未宣言・再帰・未解析/外部実体の参照を検出
- 属性デフォルトの補完、属性型に応じた正規化（tokenized の空白畳み込み）、デフォルト値内の実体の宣言順検査
- WFC: 要素の開始・終了タグは同一実体内、`]]>`/`--`、実体展開予算（billion laughs 対策）
- パーサ再構成: `scan` に Reference トークンを追加、pending-text バッファでテキストの極大性を維持
- **完了条件**: xmlconf の not-wf **701 件**・DTD 依存 valid **243 件**を通す（外部実体・5th ed 非該当ケースは skip として計上）

**2a-ii. NeedEntity 機構と外部一般実体** ✅ 完了
- 決定 7 の I/O 境界: `Progress::NeedEntity` + `EntityRequest` + `UriResolver`（同期）/ `AsyncUriResolver`（非同期）
- パーサは外部一般実体の参照で停止し `pending_entity()` を公開、ドライバが解決して `provide_entity(bytes)` / `decline_entity()` を呼ぶ。パーサは符号化判定とテキスト宣言の除去・検証を行う
- `Reader::with_resolver` / `AsyncReader::with_resolver`。**リゾルバ未設定なら外部実体は拒否**（XXE 対策の安全既定）
- 外部実体は現状「一括読み込み」（本文はストリーム、外部実体は全読み）
- **完了条件**: xmlconf の外部一般実体ケースを通す（valid 266 / not-wf 761、0 失敗）

**2a-iii. 外部サブセットと外部パラメータ実体** ✅ 完了
- 外部 DTD サブセットと外部パラメータ実体の解決（`NeedEntity` を DTD パーサにも通す。RequestKind に `ExternalSubset` / `ParameterEntity` を追加）
- 宣言内のパラメータ実体展開（`<!ATTLIST %e; ...>`、内容モデル・属性型・デフォルト値・entity value 内）、ネストした PE、条件セクション `INCLUDE`/`IGNORE`
- **standalone 制約**: 外部サブセット宣言の一般実体参照・属性デフォルトを `standalone="yes"` で拒否
- **WFC: PE Between Declarations / Proper PE Nesting**: 宣言・コメントが PE 境界をまたぐ場合を検出
- 外部一般実体を宣言する外部サブセットも解決可能に（`not-sa` ケース）
- **完了条件**: xmlconf の外部サブセット依存ケースを通す（valid 353 / not-wf 830、0 失敗）
- **既知の逸脱 3 件**（文書化）: entity value 内の外部 PE（引用符埋め込みで融合が破綻）×2、XML 仕様自身の ~100KB DTD（空 PE を含む内容モデルのネスト）×1

**実装方式（決定: 完全ストリーミング / 再パース）**: DTD パーサを **DTD バッファに対する再入可能関数**にする。内部 PE 参照はその場で値を融合（`%ref;` を値で置換）して読み進める。**外部 PE / 外部サブセットの取得点で `NeedExternalPe` / `NeedExternalSubset` を返して中断**し、ドライバが `NeedEntity` 経由で取得、取得内容をバッファに融合してから**先頭から再パース**する。再パースにより宣言途中での中断・再開を回避しつつ、宣言内 PE も文字列融合で扱える（DTD は小さいので再パースのコストは許容）。内部サブセット（PE は宣言間のみ、条件セクション不可）と外部サブセット（PE は宣言内も可、条件セクション可）はバッファ境界で区別する。

**2b. 妥当性検証**（決定 1・8、`xylograph-validate`）✅ 完了
- **スキーマ非依存の検証インタフェース**: `Validator`（イベント列／後の DOM 木を受け取る）と `ErrorListener`（warning / error(recoverable) / fatal）。DTD 検証器をその最初の実装に
- parser が DTD モデル（`Dtd` / `ContentSpec` / `AttDef` …）を公開 API として提供（`pub`、`Dtd: Clone`）
- 内容モデルの Glushkov オートマトンによるコンパイルと照合、決定性制約（付録 E）の検査
- 属性の妥当性、ID の一意性・IDREF 解決、ルート要素名の一致
- `ErrorListener` 経由の recoverable エラー報告と継続。`CollectErrors` / `FailFast` を同梱
- ファサードから `xylograph::validate` として再エクスポート
- **成果物**: `xylograph-validate` クレート（`Validator` / `Schema` / `ErrorListener` / `DtdValidator`。テスト 5 + 統合 10 + doctest 1）
- **完了条件**: xmlconf の invalid 群 **89/97 を検出**（0 失敗）。残る 8 件は特殊な妥当性制約として `KNOWN_DEVIATIONS` に理由付きで記録 — Proper Group / PE Nesting ×5、standalone トークン化正規化 ×2、既定値中の実体宣言順 ×1（いずれも本線の検証は全通過）

**2c. XML Base / xml:id**（決定 6）✅ 完了
- ノードごとの基底 URI 計算（起点は実体の system ID、`xml:base` で上書き）。`xml:base` 値は RFC 3986 §5.3 で親の基底に対して解決。`Parser::base_uri()` で取得
- `xml:id` の ID 型扱い（トークン化正規化を適用）と `Parser::xml_id()`。NCName 検査・一意性検査は検証層が担い、**2b の ID 機構（`ids` テーブル）を共用** — DTD 宣言の `ID` と同じ ID 空間で衝突を検出。DTD の有無を問わず検査（DTD あり: `DtdValidator`、なし: `XmlIdValidator`）
- `ParserConfig` の実行時フラグ（`set_config` / `Reader::with_config`）と feature `xml-base` / `xml-id`（既定オフ。有効時はフラグ既定オン）
- **成果物**: パーサに `ParserConfig` と `base_uri()` / `xml_id()`、検証に `XmlIdValidator` と共用 ID 検査（パーサテスト +3、検証統合テスト +6）
- **完了条件**: XML Base（system ID 起点・`xml:base` 継承・相対解決・オフ切替）と xml:id（一意 NCName 受理・重複検出・非 NCName 検出・正規化・未宣言でも非エラー・宣言 ID との衝突）を、仕様の例に沿った的を絞ったテストで確認
- **既知の範囲**: 外部実体境界での基底 URI（外部実体内の要素が実体自身の URI を基底とする XML Base §4 の規定）は未対応 — 外部実体は既定無効かつ resolver 必須のため優先度は低い。W3C の xml:id 1.0 / XML Base 公式スイートの取り込みは後続（他スイート同様 env var 方式で）

### Phase 3 — DOM とシリアライザ

規模が大きいため、Phase 1・2 と同様にサブフェーズへ分割し、各サブフェーズの終わりにコミットする。

**3a. アリーナツリーとノードモデル**（決定 1・3、`xylograph-dom`）✅ 完了
- アリーナ（`Vec<NodeSlot>` + `NodeId(u32)`）を `Document` が所有。ノードハンドルは `Copy` の `NodeId`
- ノード種別（`NodeType`、DOM `nodeType` コード）と種別ごとのペイロード（Element/Text/CDATA/Comment/PI/Document/DocumentType/DocumentFragment）
- 構築（`create_element` / `create_element_ns` / `create_text_node` / `create_comment` / `create_cdata_section` / `create_processing_instruction` / `create_document_type` / `create_document_fragment`）
- 走査（parent / first_child / last_child / previous_sibling / next_sibling / children / document_element / doctype）と、連鎖読み取り用の借用ハンドル `NodeRef`
- 名前・値（node_name / node_value / local_name / prefix / namespace_uri / text_content）、属性の get/set/remove（最小）
- 変更の基礎（append_child / insert_before / remove_child、サイクル・コンテナ・reference 検査）と `DomException` コード体系
- **成果物**: `xylograph-dom` クレート（テスト 12 + doctest 3）。ファサードから `xylograph::dom`
- **完了条件**: 木の構築・走査・値取得・基本的な変更が通る（本サブフェーズのユニット/doctest）

**3b. 変更 API・live コレクション・名前空間**（`DOMException` を全面適用）✅ 完了
- 変更 API の完全化: `replace_child`、DocumentFragment の展開挿入（フラグメントの子を順に移し、フラグメントは空になる）、Document 直下の子制約（ルート要素 1 つ・doctype 1 つ・直下のテキスト不可、フラグメント挿入は全体を事前検査）
- **Attr をアリーナノードとして扱う**（`NodeType::Attribute`、owner・is_id を保持）: `create_attribute(_ns)` / `get_attribute_node(_ns)` / `set_attribute_node` / `remove_attribute_node`。既存の属性 API も属性ノード経由に
- **live コレクション**: `NodeList`（`child_nodes` / `get_elements_by_tag_name(_ns)`、`*` ワイルドカード対応）と `NamedNodeMap`（`attributes`）。借用ごとに木を評価して live 性を得る
- `get_element_by_id`（`set_id_attribute` で印を付けた ID を文書順で探索。3c で DTD / xml:id を印付け）
- `create_element_ns` / `create_attribute_ns` / `set_attribute_ns` の名前空間検査（NAMESPACE_ERR: 接頭辞のみで名前空間なし・`xml` / `xmlns` の不整合）
- **成果物**: `xylograph-dom` に変更 API・コレクション・名前空間検査を追加（テスト 25 + doctest 4）
- **完了条件**: 変更・コレクション・名前空間・ID の各操作が本サブフェーズのテストで通る

**3c. DocumentBuilder（パース → DOM）**✅ 完了
- パーサのイベント列から DOM を構築（`xylograph-dom` の `build` モジュール、feature `parse`）: 要素＋属性、テキスト／CDATA／コメント／PI ノード、`DOCTYPE`。パーサが解決した名前空間を要素・属性名へ引き継ぐ
- **ID 型属性を構築時に印付け**: `xml:id`、および DTD が `ID` 宣言した属性を `set_id_attribute` でマーク → `get_element_by_id` が動く
- **基底 URI の取り込み（XML Base）**: 各要素の実効基底 URI（`xml:base` と system id から解決済み）を interned で記録し、`Document::base_uri()`（DOM `baseURI`）で取得。属性は所有要素、テキスト等は最寄り要素の基底を継承、無ければ文書の基底へフォールバック
- `parse(source)` / `parse_reader(reader)`（resolver・limits・system id は reader 経由）。ファサードから `xylograph::dom::build`（既定で有効）
- feature 構成: `parse`（parser 依存＋`xml-base`/`xml-id` を引き込む）、`encodings`（weak 転送）。DOM 単体（parser なし）ビルドは維持
- **成果物**: `build` モジュールと `base_uri`（ユニット +1、統合テスト 10）
- **完了条件**: 代表的な文書（名前空間・DTD・PI/コメント/CDATA・xml:id・xml:base）が DOM に載り、走査・ID 検索・基底 URI 取得が通る

**3d. シリアライザ**（`xylograph-serialize`）✅ 完了
- `Serializer`（ビルダー: `with_xml_declaration` / `with_standalone` / `with_indent`）で DOM 部分木を XML テキストへ。`to_string` と `write<W: io::Write>`
- **エスケープ**: テキスト（`& < >`、`\r`）と属性値（`& < "`、`\t \n \r` を文字参照）、CDATA の `]]>` 分割
- **名前空間修復**: 宣言が in-scope に無い接頭辞・既定名前空間を要素に補って整形式化（`create_element_ns` のみで作った木も直列化可能）。既存の xmlns 属性は重複させない
- **indent**: 要素内容のみ字下げ（文字データを含む要素はインラインで折り返さない）、PI・コメント・DOCTYPE
- **成果物**: `xylograph-serialize` クレート（ユニット 3 + 統合 9 + doctest 2）。ファサードから `xylograph::serialize`
- **完了条件**: パース → DOM → 直列化が妥当な XML を出し、代表的な構造・名前空間・エスケープが往復する
- **後続に回した点**: 出力は **UTF-8**。非 UTF / UTF-16 のバイト出力（`encoding_rs` の `encode` は UTF-16 を UTF-8 化するため独自処理が要る）は別途。DOCTYPE の public/system id は現状ビルダーが取り込まない（パーサが未公開）ため往復で欠落 → 3e で解消候補

**3e. push/StAX アダプタとラウンドトリップ**✅ 完了
- **SAX 相当の push アダプタ**（`xylograph-parser` の `sax` モジュール）: `Handler` トレイト（既定実装つき）と `drive(reader, handler)`。要素イベントは `&Parser` を渡し、名前・名前空間・属性をアクセサで読む
- **StAX Writer**（`xylograph-serialize` の `XmlWriter`）: `write_start_element` / `write_attribute` / `write_characters` / `write_cdata` / `write_comment` / `write_processing_instruction` / `write_end_element`。開始タグ直後の終了は `<a/>` に畳む。エスケープは自動
- **DOCTYPE の外部 ID 往復を解消**: パーサに `doctype_public_id()` / `doctype_system_id()` を追加し、ビルダーが DocumentType に取り込む → シリアライザで往復
- **成果物**: `sax` モジュール（ユニット 2 + doctest 1）、`XmlWriter`（ユニット 5 + doctest 1）、往復統合テスト
- **完了条件（Phase 3 全体）✅**: パース → DOM → 直列化のラウンドトリップが情報を落とさない（名前空間・属性・混在内容・コメント・PI・CDATA・DOCTYPE 外部 ID を往復で確認）
- **なお未対応**: DOCTYPE の内部サブセット（DOM `internalSubset` 未モデル）は往復で欠落。非 UTF/UTF-16 バイト出力も別途（3d 参照）

### Phase 3.5 — XInclude（決定 6）

Phase 3 の DOM（アリーナ）と Phase 2c の基底 URI / ID の上に載る後処理層。規模が大きいためサブフェーズに分割する。

**3.5a. XInclude コア**（`xylograph-xinclude`）✅ 完了
- `xi:include` の `parse="xml"`（全体・文書要素を取り込み）/ `parse="text"`（`encoding` で復号）
- **href を基底 URI に対して解決**（3c の `base_uri` の上に載る）。取り込みは `import_node`（DOM に追加したクロス文書ディープコピー）で
- **`xi:fallback`**: 取得失敗（リソースエラー）時に使用。fallback 内の `xi:include` も展開。fallback 無しの失敗・取り込みループ・誤配置 fallback は致命（`ErrorKind::XInclude` を追加）
- **再帰処理**（取り込んだ文書内の `xi:include` も展開）、ループ検出、深さ・取得数の上限（`with_max_depth` / `with_max_includes`）
- **base URI fixup**（`with_base_fixup`、既定 ON/OFF 双方をテスト）: 取り込んだ要素に `xml:base` を付与して基底を保存
- **取得は `Loader` トレイト経由**（既定で何も取得しない = fetch 攻撃面を持たない）。crate = feature `xinclude`（ファサードで opt-in）
- **成果物**: `xylograph-xinclude` クレート（統合テスト 9 + doctest 1）、DOM に `import_node` と `build::parse_with_system_id`、パーサ doctype 外部 ID 公開
- **完了条件**: 取り込み・再帰・text・fallback・ループ・上限・base fixup(ON/OFF) が通る

**3.5b. XPointer**（部分選択）✅ 完了
- **短縮ポインタ**（bare NCName → ID で要素選択、`get_element_by_id` を利用）
- **`element()` scheme**: `element(id/2/1)`（ID 起点の子シーケンス）/ `element(/1/2)`（ルート起点、各ステップは子「要素」の位置）
- **`xmlns()` scheme** はパースする（scheme ベースの列に含められる）が、位置指定の `element()` には不要
- `xi:include` の `xpointer` で部分リソースを選択: **外部リソース**（`href`＋`xpointer`）と**同一文書**（`href` 無し＋`xpointer`、`clone_node` で複製し複製内の include も展開、自己ループ検出）。選択できなければ fallback。`parse="text"` との併用は致命
- **支援した追加**: DOM に `clone_node`（同一文書ディープコピー = `cloneNode`）
- **成果物**: `xpointer` モジュール、統合テスト +6
- **完了条件**: 短縮/element()/同一文書/未選択→fallback/text 併用拒否 が通る

**3.5c. fixup 完全化・検証順序**✅ 完了
- **language fixup（`xml:lang`）**: 取り込んだ要素が元の言語（祖先から継承した `xml:lang`）を保つよう、取り込み先の言語と異なり要素自身が `xml:lang` を持たない場合に付与。`with_language_fixup`（既定 ON、ON/OFF をテスト）
- **base URI fixup 精緻化**: 元要素の実効基底（`base_uri`）と取り込み点の基底が異なるときのみ付与（3.5a から継続、xpointer 選択要素の基底も考慮）
- **検証との順序を明文化**: XInclude は解析・検証と独立したパスで、順序は呼び出し側が選択。推奨は **解析 → 展開 → 検証**（展開後の文書を検証）。crate doc に記載
- **成果物**: `fix_language` と `effective_language`、統合テスト +3（計 18）
- **完了条件**: language fixup(ON/OFF/一致時スキップ) と検証順序ドキュメントが通る
- **後続に回した点**: W3C XInclude 1.0 公式テストスイートの取り込み（他の公式スイート同様、未 vendored。env var 方式のハーネスは今後）。独自テストで機能面は網羅

### Phase 4 — XPath 1.0

規模が大きいためサブフェーズに分割する（決定 3: XDM をツリーと分離し `trait` に対して評価器を動かす／名前空間ノードは XDM 側で合成。決定 4: AST を軸走査＋述語へ）。

**4a. XDM（データモデル）**（`xylograph-xdm`）✅ 完了
- 7 ノード型（root/element/attribute/namespace/text/comment/PI）を表す `Model` トレイト（`type Node` でツリー実装を抽象化）。`NodeKind` / `ExpandedName`
- **DOM 実装 `DomModel`**（`&Document` を借用、書き換えない）: 隣接 text/CDATA を 1 テキストノードに結合、**名前空間ノードの合成**（in-scope 宣言＋暗黙の `xml`、xmlns 属性は属性軸から除外）、各ノード型の string-value と expanded-name、属性の親＝所有要素
- **文書順**を構築時に全ノードへ付番（要素→名前空間ノード→属性→子の順）、`document_order` で比較
- DOM に `owner_element`（属性の所有要素）を追加。ファサードから `xylograph::xdm`
- **成果物**: `xylograph-xdm` クレート（統合テスト 8 + doctest 1）
- **完了条件**: 親子・兄弟・属性・名前空間・文書順・string-value・expanded-name がトレイト越しに辿れる

**4b. 字句・構文解析**（`xylograph-xpath`）✅ 完了
- **トークナイザ**: XPath 1.0 §3.7 の文脈依存規則を字句段階で解決 — `*` は名前テスト位置なら wildcard・オペランドの後なら乗算、`and`/`or`/`div`/`mod` は演算子位置でのみ演算子、`::` が続けば軸名、`(` が続けば NodeType か関数名。パーサは曖昧さのないトークンだけを見る
- **文法 → AST**（再帰下降、優先順位テーブル）: OrExpr〜UnaryExpr、UnionExpr、PathExpr／FilterExpr、13 軸、全ノードテスト、述語、関数呼び出し、変数参照
- **略記を AST 構築時に展開**: `//`→`descendant-or-self::node()`、`.`→`self::node()`、`..`→`parent::node()`、`@x`→`attribute::x`、軸省略→`child::`。評価器（4c）は単一の平坦な形だけを扱えばよい
- **`Display`** が展開後の形を XPath として書き戻す（二項式は括弧付きで優先順位が見える）→ テストの検証手段
- エラーは `ErrorKind::XPath`（core に追加）で式中の位置を示す
- **成果物**: `xylograph-xpath` クレート（ユニット 7 + 統合 13 + doctest 3）。ファサードから `xylograph::xpath`
- **完了条件**: 略記・軸・ノードテスト・述語・優先順位・関数/変数/リテラルが AST になり、エラーが位置を示す

**4c. 評価器コア**✅ 完了
- **4 値型と型変換**（`Value`、§3・§4）: `boolean()` / `number(model)` / `string(model)`。XPath の数値書式（指数なし、`NaN` / `Infinity`、`-0`→`0`）と数値解析（指数・先頭 `+` を認めない）を厳密に実装
- **評価コンテキスト**: `Context`（コンテキストノード・位置・サイズ）と `Environment`（変数束縛・接頭辞→名前空間）。`Context` は参照のみ保持で `Copy`
- **13 軸**を軸順（前方軸は文書順、逆方向軸 `ancestor`/`ancestor-or-self`/`preceding`/`preceding-sibling` は逆文書順）で走査。述語の位置は §2.4 のとおり軸順で数えるので、呼び出し側は軸の向きを意識しない
- **ノードテスト**: 名前テスト（軸の principal node type で絞る）・`node()`/`text()`/`comment()`/`processing-instruction(literal?)`。接頭辞は `Environment` で解決（未束縛はエラー）
- **述語**: 数値結果は位置テスト（§3.3）、それ以外は boolean 変換。複数述語は順に適用し、位置は直前の述語が残した集合内で数える
- **演算子**: 算術（IEEE 754、`mod` は切り捨て除算の剰余）、比較（§3.4 の node-set / boolean / number / string の変換規則を網羅）、`or`/`and` の短絡、`|` の合併（文書順・重複排除）
- 関数は述語に必要な 6 個（`position`/`last`/`count`/`not`/`true`/`false`）のみ。残りは 4d
- **成果物**: `value` / `context` / `axis` / `eval` / `functions` モジュール、`evaluate` / `evaluate_with`（ユニット +2、統合 14、doctest +2）
- **完了条件**: ロケーションパス・全軸・ノードテスト・述語・演算子式・変数が評価できる
- **決定 4 について**: 現状は AST の再帰評価。閉包／バイトコードへのコンパイルは意味論が固まりテストで守られた後の最適化として Phase 8 に回す（正しさを先に確定させるため）

**4d. コア関数ライブラリ（27 関数）**✅ 完了
- **node-set（7）**: `last` / `position` / `count` / `id` / `local-name` / `namespace-uri` / `name`
- **string（10）**: `string` / `concat` / `starts-with` / `contains` / `substring-before` / `substring-after` / `substring` / `string-length` / `normalize-space` / `translate`
- **boolean（5）**: `boolean` / `not` / `true` / `false` / `lang`
- **number（5）**: `number` / `sum` / `floor` / `ceiling` / `round`
- **仕様の細部を実装**: `round()` は半数を正の無限大方向へ（`round(-1.5)` = -1、`f64::round` とは異なる）／`substring()` は両引数を丸めてから浮動小数点で範囲判定するので `NaN`・無限大の境界例（仕様の 6 例）がそのまま出る／文字数はバイトではなく文字で数える／`lang()` は最も近い `xml:lang` を見て大小文字を無視しサブ言語 (`en-GB` は `en` に応答) を認める／`translate()` は `from` の最初の出現が優先、対応がなければ削除
- モデルに `qualified_name`（`name()` 用の接頭辞込みの名前）と `element_by_id`（`id()` 用、ID 型付けのない木は既定で `None`）を追加。DOM 実装は DTD / `xml:id` で印を付けた属性を使う
- 引数個数・型の誤りは関数名と期待を示すエラー
- **成果物**: 統合テスト 8（`tests/functions.rs`）、ユニット +5
- **完了条件**: 全 27 関数がテストで通る

**4e. 公開 API（JAXP 準拠）**✅ 完了
- **`javax.xml.xpath` の型構成に合わせる**（決定 3。当初 `XPath` をコンパイル済み式の名前にしていたが、Java では `XPath` は環境・`XPathExpression` がコンパイル済み式であり**逆の意味**になっていたので是正）:

  | `javax.xml.xpath` | 本クレート |
  |---|---|
  | `XPathFactory.newInstance().newXPath()` | `XPath::new` |
  | `XPath.setNamespaceContext(…)` | `XPath::with_namespace` |
  | `NamespaceContext` | `Namespaces` |
  | `XPath.compile(String)` | `XPath::compile` |
  | `XPathExpression` | `XPathExpression` |
  | `XPathExpression.evaluate(item)` | `XPathExpression::evaluate` |
  | `XPathVariableResolver` | `Variables` |
  | `XPathConstants.NODESET` / `.STRING` / … | `Value` と変換メソッド |
  | `XPathExpressionException` | `ErrorKind::XPath` |

- **`Environment` を `Namespaces` / `Variables` に分割** — Java が `NamespaceContext` と `XPathVariableResolver` を別インタフェースにしているのに合わせた。設計上も筋が良い: 名前空間束縛は文字列 2 つで木に依存しないのでコンパイル時に確定でき、変数はノード集合を持ちうるので確定できない
- **意図的な差異 2 点**（クレート doc に明記）: Java は戻り値型を先に指定してキャストするが、本実装は `Value` が型を持ち要求時に変換する（XPath の変換規則は演算子が使うものと同一なので自然）。`XPathFunctionResolver` 相当は未提供 — 拡張関数は XSLT と同時に入れる
- `Value::nodes()` / `into_nodes()` を追加。`parse` / `evaluate` は AST を自分で保持する呼び出し側（XSLT）向けの下位 API として残す。ファサードから `xylograph::xpath`
- **プロパティテスト**（proptest）: 任意テキストで字句解析が panic しない（バイト境界の保証）／数値の書式↔解析が有限値で可逆／AST を印字して再解析すると同じ木になる。加えて「`Infinity` は書けるが読み戻せない」という**仕様自身の非対称性**を明示的に記録
- **差分テストの土台**（`tests/differential.rs`、90 式の corpus）: corpus が本実装で評価できることは常時検証。**比較の照合先は Java**（`javax.xml.xpath`）とし、ライブラリ完成時に組む（面が動いている最中に組んでも追従コストが高いため）。libxml2 版は手動実行用の暫定で **CI では走らせない**
  - 比較設計で決めたこと: 式は `concat('[', string(E), ']')` で包んで両者とも 1 個の文字列にする（ノード直列化形式は仕様が定めないので比較しない）。`1 div 3` のように**十進表現が有限でない数**は除外 — §4.2 が桁数を規定しておらず、実装差は**どちらも適合したまま**生じる。仕様が許す差分を報告するテストは無視されるようになる
- **成果物**: `XPath` / `XPathExpression` / `Namespaces` / `Variables`、`tests/properties.rs`、`tests/differential.rs`（テスト計 67）
- **完了条件**: JAXP と対応の取れた公開 API が使える
- **Phase 4 全体について**: XPath 1.0 には W3C 公式の単独テストスイートが存在しない（XQTS は 2.0 向け）。網羅的な外部資産は **OASIS/Xalan の XSLT 1.0 スイート**で、XSLT が動いてから実行できる → **Phase 6 で取り込む**。CLI の `xylo xpath` は Phase 7

### Phase 5 — XSLT 骨格

規模が大きいためサブフェーズへ分割する。**拡張関数機構は ROADMAP 当初 Phase 5 冒頭に置いていたが 5d へ移した** — 利用者（`exsl:node-set()` と実行エンジン）が現れる前に設計すると要求を外すため。5c が実際に必要とした形で組む。

**5a. パターン**（`xylograph-xslt`）✅ 完了
- **`Pattern`**: `xsl:template` の `match` が持つテスト。`|` の各代替は XSLT 上それぞれ独立したテンプレート規則なので `alternatives()` で個別に取り出せる
- **マッチング意味論**（§5.2）: 仕様は「ある祖先を文脈として評価したとき、そのノードが選択されるか」と定義するが、これは計算手順ではない（全祖先から評価すると破滅的）。**ステップを右から左へ**辿り、`/` で親へ、`//` で祖先を探索する実装にした。答えは同じで、触るのは上へ向かう経路上のノードだけ
- **述語**は「兄弟の中での位置」を問うので、親から当該ステップを評価してノードが含まれるかで判定（XPath 側に `evaluate_step` を公開）。述語が無い場合はノード単体の検査で済ませる
- **XPath 部分集合の検証**: パターン専用の文法を second grammar として持つと XPath 側と乖離するので、**XPath パーサで読んでから部分集合か検査**する方式。`child`/`attribute` 軸以外、`id()`/`key()` 以外の先頭式は理由付きで拒否
- **既定優先度**（§5.5）: 名前 0 / `prefix:*` -0.25 / それ以外のノードテスト -0.5 / それより複雑なもの 0.5
- `key()` は構文として受理するがキー表が未実装なので何にもマッチしない（**6b-2 で解消** — 5c と書いていたが、キー表が入るのはこのフェーズだった）
- **成果物**: `xylograph-xslt` クレート、`ErrorKind::Xslt`、統合テスト 13 + doctest
- **完了条件**: 各種パターンのマッチングと既定優先度が通り、部分集合外が拒否される

**5b. スタイルシートのモデルとコンパイル**✅ 完了
- **`Stylesheet::compile` / `compile_with`**: スタイルシート文書 → テンプレート規則とトップレベル変数・パラメータ。テンプレート本体は**そのまま文書の要素として残す**（実行は 5c の仕事で、5b が決めるのは「どの本体が動くか」）。そのため `Stylesheet` が全モジュール文書を所有する
- **`xsl:import` / `xsl:include`**: `Loader` トレイト経由で取得（`NoLoader` は単一文書用で、モジュールを名指されたら `compile_with` を案内するエラー）。href は基底 URI に対して解決。同一モジュールの二重取り込みは拒否（循環も止まる）
- **インポート優先順位**（§2.6.2）: インポート木の**後行順走査**で採番 — モジュールは自分がインポートしたものより高く、後のインポートは先のより高い。`xsl:include` はインポート木に載らず（本文がその場に書かれたのと同じ扱い）**取り込み元と同じ優先順位**を共有し、被 include 側のインポートは取り込み元のインポートとして扱う
- **衝突解決**（§5.5）: インポート優先順位 → 優先度 → 宣言順（最後）。最後の同点は仕様が「エラーだが最後を選ぶ回復を認める」としており、その回復を実装。優先度比較は `total_cmp` なので `NaN` が入っても順序が壊れない
- `|` の各代替が独立した規則になる（5a の設計がここで効く）ので、`match="a|b/c"` は既定優先度 0 と 0.5 の 2 規則
- **パターン中の接頭辞は、それが書かれた要素の in-scope 宣言で解決**して各規則に保存 — 文書側が同じ名前空間に別の接頭辞を使っていても正しく一致する
- **本フェーズが読む top-level 要素**は `import` / `include` / `template` / `variable` / `param` のみ。`xsl:output`・`xsl:key` などは**黙って読み飛ばす**（後続フェーズ担当。`compile_with` の doc に明記）
- **成果物**: `Stylesheet` / `Template` / `Variable` / `Loader`（統合テスト 15 + doctest）
- **完了条件**: 宣言の読み取り、import/include の優先順位、規則の衝突解決が通る

**5c. 実行エンジン**✅ 完了
- **`transform` / `Transform`**: ソース木＋スタイルシート → 結果木（`ResultTree`）。結果は**文書フラグメント**にぶら下げる — 結果木は 1 要素とは限らず、テキストのみもあり得るが、文書ノードは直下にテキストを持てないため
- **命令**: `apply-templates`（select / mode / with-param）・`call-template`・`for-each`・`if`・`choose`/`when`/`otherwise`・`value-of`・`variable`・`param`・`with-param`・`text`
- **組み込み規則**（§5.8）: root と要素は子へ委譲、テキストと属性は自身の文字列、コメント・PI・名前空間ノードは何も出さない
- **リテラル結果要素**: 名前と名前空間を保って複製し、属性は AVT 展開。**名前空間宣言は複製しない** — 要素が名前空間を保っていればシリアライザの名前空間修復が必要な宣言を書くので、スタイルシート自身の `xmlns:xsl` を除外する処理も要らなくなる
- **AVT**（§7.6.2）: `{式}`、`{{`/`}}` は literal brace。文字列リテラル中の `}` は式の一部として扱う
- **空白除去**（§3.4）: スタイルシート中の空白のみテキストノードを除去（`xsl:text` の中身は保持）。これが無いとスタイルシートの字下げが結果に出る
- **`position()` / `last()`**: XSLT の「現在ノードリスト」での位置。XPath 側に `evaluate_in`（呼び出し側が組み立てた `Context` で評価）を追加して実現
- **式のパース結果と in-scope 名前空間をキャッシュ** — ループ内のパスを毎回読み直さない
- **RTF は当面その文字列値で保持**。XSLT 1.0 が RTF に許すのは文字列化と `xsl:copy-of` だけで、後者は Phase 6。`exsl:node-set()` が来るまで観測可能な差は無く、木としての表現を先に作ると利用者不在の設計になる
- **未実装命令はエラー**（黙って読み飛ばさない）。どのフェーズで来るかを ROADMAP 参照として示す
- **再帰の深さ上限**: 既定 200。実装当初 1000 にしていたところ**ガードより先にスタックが尽きた** — 効かないガードはガードではないので、debug ビルドの 2MB スレッドスタックで安全な値まで下げ、`Transform::with_max_depth` で変更可能にした（ただしこのとき余裕を**測らずに**決めたため 6a で再発する。下記 6a 参照）
- **成果物**: `engine` / `avt` モジュール、`ResultTree` / `Transform`（統合テスト 23 + ユニット 5 + doctest）
- **完了条件**: 単純なスタイルシートが動く

**5d. 拡張機構と出力**✅ 完了
- **拡張関数の登録機構**（決定 5 の受け皿）: `Function` トレイトと `Functions` レジストリを `xylograph-xpath` に。**Java の `XPathFunction` / `XPathFunctionResolver` そのもの**で、これにより JAXP 対応表の最後の空欄が埋まった
  - クロージャに `Function` のブランケット実装があるので、登録に専用の型は要らない
  - 拡張関数は必ず接頭辞付き（XPath は無接頭辞をコア関数のために予約している）。接頭辞は**展開名**に解決してから引くので、スタイルシート側が好きな接頭辞を選べる
  - `Context` に `functions` を持たせ、`Context::with_functions` で渡す。`Transform::run_with` が XSLT からこれを供給
  - 未登録の関数は**登録済みの一覧を添えて**エラー（何が使えるか分からないまま落ちない）
- **出力メソッド**: `xsl:output method` を読んで `OutputMethod`（Xml / Html / Text）。**text メソッドを実装**（`ResultTree::text()` = 結果の文字データのみ）。xml/html の細部（indent・宣言・doctype）は Phase 6。書けないメソッドはエラー
- **`exsl:node-set()` は Phase 6.5 へ移した**: RTF を木として持つ表現が要り、それは `xsl:copy-of`（Phase 6）と同時に入れるのが筋。5c で「利用者が現れてから設計する」と決めた方針をそのまま適用し、**機構だけ先に用意して実装は利用者と同時**にする
- **成果物**: `extension` モジュール（xpath）、`Transform::run_with`、`OutputMethod`（統合テスト 9 + doctest 2）
- **完了条件（Phase 5 全体）✅**: 単純なスタイルシートが動く

### Phase 6 — XSLT 完全化

サブフェーズへ分割する。**順序の根拠**: `document()` と RTF の木表現は、どちらも「1 つのノード空間に複数の文書」を要求する（前者は取得した文書、後者は結果木の断片）。これは `DomNode` / `DomModel` の公開 API に及ぶ変更なので、**要求が出揃ってから一度だけ意図的に**行う（6c）。それを要しない部分を先に片付ける。

**6a. 結果木を作る命令**（§7）✅ 完了
- **`xsl:element` / `xsl:attribute`**: `name` と `namespace` は AVT なので実行時に決まる。`xsl:attribute` は「今開いている要素」に付けるもので、開いている要素が無ければ**エラー**（フラグメント直下に属性は置けず、黙って捨てると結果が静かに間違う）
- **本文をテキストに畳む命令**: `xsl:comment` / `xsl:processing-instruction` / `xsl:attribute` の値は、どう作られたものであれ文字データ。専用のフラグメントに本文を流してその `text_content` を取る（`captured_text`）
- **`xsl:copy`（§7.5）は浅く、`xsl:copy-of`（§11.3）は深く**。root は「自分自身のノードが無い」ので `xsl:copy` では何も作らず本文だけを走らせ、`xsl:copy-of` では子を並べる。node-set 以外を `xsl:copy-of` に渡した場合はその文字列値（§11.3 の規定どおり）
- **名前空間宣言は複製しない** — 5c のリテラル結果要素と同じ判断。要素が名前空間を保っていればシリアライザの名前空間修復が必要な宣言を書く
- **`xsl:attribute-set`（§7.1.4）は「選ぶ」のではなく「併合する」**: 同名の宣言は全て走らせ、**インポート優先順位の低い方から**適用して高い方が上書きされずに残るようにする。集合自身の `use-attribute-sets` も辿り、**使われる側を先に**置いて使う側の属性が勝つようにした。自分自身を（間接にでも）使う集合は適用中の連鎖を持って検出しエラー
- **`xsl:message`** は結果木ではなく `ResultTree::messages()` に貯める（見る人向けのものであって結果の一部ではない）。`terminate="yes"` は貯めずにその文言を持つエラーになる
- **恒等変換**（`match="@*|node()"` + `xsl:copy` + `apply-templates select="@*|node()"`）が入出力一致することをテストに置いた — 個々の命令の単体確認より、外部から見て意味のある証拠になる
- **`xsl:copy-of "$rtf"` はまだ正しくない**（RTF は文字列値のまま）。木としての表現は 6c
- **再帰ガードの再発と、今度は測った上での対処**（CI が 3 OS 全てでスタックオーバーフロー）
  - 症状: 手元では通り CI では落ちる。原因は**テストハーネスがテスト 1 件だけならメインスレッド（8MB）、複数なら生成スレッド（2MiB）で走らせる**こと。テストが「与えられたスタック」に依存していたので、走らせ方で結果が変わっていた
  - 実測: `xsl:call-template` 1 段が **10,048 バイト**（debug）。200 段で 1,962 KiB ＝ 2MiB スタックほぼ丸ごとで、**余裕はゼロだった**。5c で 200 に下げたときに測っていなかったツケ
  - 原因: `instruction` の巨大な `match` を**インラインの本体**で書いていたこと。debug ビルドは**どの腕を通るかに関わらず全腕のローカルにフレーム領域を割り当てる**ので、命令セット全体のコストが再帰の各段に課金されていた。全腕をメソッド呼び出しにして **10,048 → 3,728 バイト/段**（200 段で 728 KiB）
  - 対処 1: `the_depth_guard_is_reached_before_the_stack_is`（engine のユニットテスト）が**実際に再帰させてスタック番地を読み、1 段あたりのバイト数を測って** `DEFAULT_MAX_DEPTH × 実測値 ≤ 1 MiB` を検証する。将来フレームが太ったら**スタックオーバーフローで異常終了する前に、数値付きの読めるメッセージで落ちる**
  - 対処 2: 統合テスト側は**スタックサイズを明示したスレッド**（2 MiB）で走らせる。走らせ方に結果が左右されない
  - **教訓**: 「ガードが先に効く」は定数を下げれば成り立つ性質ではなく、**測って初めて言える**性質。次のフェーズがフレームを太らせても壊れないよう、測定自体をテストにした
- **成果物**: `AttributeSet`、`ResultTree::messages()`、engine の結果木命令群（統合テスト 19 + ユニット 1）
- **完了条件**: §7 の命令と属性集合が通り、恒等変換が入出力一致する

**6b. sort / key / number / decimal-format**

規模が大きいのでさらに分割する。**順序の根拠**: `key()` も `format-number()` も「無接頭辞の XSLT 関数」なので、まずその口（6b-1）を作る。ICU 依存の導入（6b-3）は外部依存が増える唯一の回なので、それを要しないものを先に片付ける。

**6b-1. XSLT 関数の口と §12.4 / §15 の関数** ✅ 完了
- **設計上の要**: XPath は無接頭辞をコア関数のために予約しているが、**XPath を「ホストする」言語が足す関数も無接頭辞**（`current()` / `key()` / `format-number()`）。そこで `Functions` の**空名前空間**をホスト言語の置き場と定め、コア関数を先に引いてから空名前空間を見る順序にした（登録関数がコア関数を隠せない）
- `Transform::run_with` は `Functions` を**借用ではなく受け取る**ように変更 — XSLT が自分の関数を足すため。「その変換のためだけの集合」という意味にもなる
- **`Model::Node` に `'static` を追加**: 登録された関数は変換の間ずっと生き、ノードを跨いで保持する（`current()` の現在ノード、`generate-id()` の採番表）。木を借用するハンドルではこれができない。ハンドルは元々「同一性だけを表す索引」なので制約としては自然
- **`current()`**（§12.4）: 述語の中では文脈ノードが候補を渡り歩く一方、現在ノードは命令が置いた場所に留まる。この差こそ `current()` の存在理由なので、**式の評価開始時**に記録する（述語に入っても動かない）。`count(//a[@id = current()/@id])` が 1、`.` にすると 2 になることをテストで対比
- **`generate-id()`**（§12.4）: 仕様の要求は「同一ノードには毎回同じ、異なるノードには異なる、英数字で先頭は英字」だけ。**訊かれた順に採番**する（何かから導出したふりをしない）。ハッシュは衝突しうるので使わない
- **`system-property()`**（§12.4）: 仕様が名指しする 3 つ（`xsl:version` は**数値** 1.0、`xsl:vendor`、`xsl:vendor-url`）のみ。未知のプロパティは**エラーではなく空文字列**（§12.4 の規定）
- **`element-available()` / `function-available()`**（§15）: これは「XSLT の建前」ではなく**この実装が実際に何を走らせるか**を答えるべきもの。`element-available()` は dispatch の隣に置いた `INSTRUCTIONS` から答え、**両者が食い違わないことをテストで検査**（各名前を実際に走らせ「未実装」エラーにならないことを確認）。`function-available()` は**レジストリに直接訊く**ので、呼んだら動くかどうかと答えが原理的にずれない
- **成果物**: `xylograph-xslt` の `functions` モジュール、`xylograph_xpath::is_core_function`（統合テスト 13 + ユニット 1）
- **完了条件**: §12.4 / §15 の 5 関数が通り、`element-available()` が実装の実態と一致する

**6b-2. `xsl:key` と `key()`**（§12.2）✅ 完了
- **表は変換開始時に一括構築**する。構築には「木全体を歩いてパターンを試す」ためスタイルシートとモデルが要るが、**登録された関数はそのどちらも保持できない**（どちらも借用）。表さえできれば `key()` は引くだけになり、ノードハンドルは何も借用しないので `Running` に置ける
- 構築順は**大域変数 → キー表**。大域変数が `key()` を呼ぶことも、キーの `use` が大域変数を読むこともあるため
- **同名の宣言はすべて寄与する**（§12.2 は「1 つが勝つ」ではなく「足し合わせる」）。よって `Key` にインポート優先順位は持たせない
- `use` が node-set を返す場合は**各ノードの文字列値それぞれで**引けるように登録。`key()` の第 2 引数が node-set なら**各値の和**（join になる）。結果は文書順・重複なし
- キー名は QName なので、**書かれた場所の in-scope 宣言で解決**して展開名で索く（スタイルシートと呼び出し側で接頭辞の綴りが違ってよい）
- **5a の宿題を解消**: パターン先頭の `key()` は「構文としては受理するが何にもマッチしない」状態だった。`KeyTable` トレイトを切り、パターンが表を受け取れるようにした（`Pattern::matches_using` / `Stylesheet::template_for_using`）。表を持たない呼び出し側には空の実装が渡り、従来どおりの意味になる
- **成果物**: `Key`、`KeyTable`、`Stylesheet::keys()`（統合テスト 14）
- **完了条件**: `key()` とキー先頭パターンが通り、同名宣言が足し合わされる

**6b-3. `xsl:sort`**（§10、ICU 照合、決定 2）✅ 完了
- `icu_collator` / `icu_locale_core` を feature `icu`（既定 ON）で導入。**MSRV は問題なし**（`icu_collator` の rust-version は 1.83 で、本プロジェクトの 1.85 より低い）
- **`icu` は「速さ」ではなく「答え」を変える feature**。§10 は照合順序を処理系に委ねているので、有無のどちらも準拠。よって **CI の feature 行列で両方走らせ**、**振る舞いレポートも両ビルドで出力**する（`icu` 無しだと de と sv が同じ順序に潰れることが観測できる）
- `case-order` は**CLDR の `kf` としてコレータの設定に渡す**。後段のタイブレークにはできない — コレータは既定の強度で `A` と `a` を区別してしまうので、「等しいときだけ効く」実装では一度も効かない（テストで発覚）
  - `icu` 無しの経路では、コード位置順が先に大文字と小文字を分けてしまうため、`case-order` が指定されたときだけ**小文字化して比較 → 同点なら case で決める**
- **決定 9 の対象を 2 件追加**（レポート出力）
  - テキストソートの順序が **仕様未定義**（照合順序を定義していない）
  - 数値ソートで **NaN がどこに来るかが仕様未定義**。§10 は `number()` 変換までしか言わない。**先頭に置く**選択をした（XSLT 2.0 が後に採用した順序であり、当時の処理系もそうだった）
- ソートキーは**動かす前に全件算出**する。§10 のキー評価は「選択された時点での位置」を文脈位置とするので、並べ替え途中のリストから読んではいけない。副産物として比較のたびに再評価しなくて済む
- **安定ソート**（§10 の要求）。同点は選択順を保つ
- **成果物**: `collate` モジュール、`language_aware_collation()`、feature `icu`（統合テスト 17 + ユニット 3）
- **完了条件**: §10 の 4 属性が通り、`icu` の有無で振る舞いレポートが正しく変わる

**6b-4. `xsl:number`**（§7.7）✅ 完了

当初 `format-number()` と同じサブフェーズに置いていたが**分割した** — 「数値を英字/ローマ数字で書く」書式系と「十進パターンで整形する」書式系は共有するものがほとんど無く、一緒にする理由が無い。`format-number()` は 6b-5。

- **書式文字列の解釈**（§7.7.1）を `number` モジュールに独立させた。トークン（英数字の極大列）と区切り（それ以外の極大列）の交替。ユニットテストだけで検証できる純粋な部分なので、木の走査と混ぜない
- **番号列の算出**（§7.7）: `level` single / multiple / any、`count` / `from`
  - `count` 省略時は「現在ノードと同じ種別、（名前があれば）同じ展開名」。これは**パターンとして書き下せない**（全種別を表現できない）ので、パターンに変換せず直接判定する
  - `multiple` は内側から集めて**最後に反転**する（§7.7 は外側から書く）
  - `any` は属性・名前空間ノードを除外した文書順の走査。`from` があれば「直前に `from` に一致したノード以降」から数え直す
- **決定 9 の対象を 2 件追加**（レポート出力）
  - トークン `i` / `I` が **`letter-value` 無しで何を意味するかは仕様未定義**。§7.7.1 は「`letter-value` が曖昧性を解消する」としか言わない。**ローマ数字を採る**（英字の 9 文字目のつもりなら `a` と書けるが、ローマ数字にはそれしか書きようがない）
  - **その数列で表せない値**（ローマ数字の 4000 以上、英字の 0 以下）**の扱いも未定義**。十進にフォールバックする（何も出さない／エラーにするのではなく）
- **成果物**: `number` モジュール（統合テスト 19 + ユニット 14）
- **完了条件**: §7.7 の `level` 3 種と書式トークンが通る

**6b-5. `xsl:decimal-format` と `format-number()`**（§12.3）✅ 完了
- **§12.3 は独自の書式言語を定義していない**。「JDK 1.1 `DecimalFormat` クラスの構文」と外部参照するだけなので、**XSLT が決めていない点は Java の挙動を正**とした（本ライブラリが対照すべき相手そのもの）
- パターンは「接頭辞・桁位置の並び・接尾辞」を `pattern-separator` で最大 2 つ（正/負）。**各文字は `xsl:decimal-format` で改名できる**ので、パターン自体もその文字で書かれる（`zero-digit='@'` なら `?@@` が `#00`）
- **引用符の扱いで一度間違えた**: 桁位置の開始位置を探す際に引用を見ておらず、`'#'0` の `#` を桁位置と誤認した。引用状態を追う走査に修正
- **`#,##,##0` のグループ幅**は 2 ではなく 3。`DecimalFormat` はグループ幅を 1 つしか持たず、**小数点に最も近い区切り間隔**を採る。テスト側の期待が間違っていた（Java の実挙動に合わせて修正）
- **丸めは half-even**（`DecimalFormat` の既定）。§12.3 は丸めについて何も言わないので**決定 9 の対象**としてレポートに実測を出す。Java と「近い」ではなく「一致する」ことを狙う
- 同名の `xsl:decimal-format` が食い違う場合は**エラー**（§12.3。last-one-wins にはしない）。一致する重複は許す
- **成果物**: `decimal` モジュール、`Symbols` / `Formats`（統合テスト 15 + ユニット 12）
- **完了条件（Phase 6b 全体）✅**: sort / key / number / decimal-format と無接頭辞 XSLT 関数が揃う

**6c. 複数文書のノード空間**（本フェーズの設計上の要）✅ 完了
- **`DomNode` が所属文書を持つ**ようになった（`DocumentId`）。`DomModel` は「構築元の 1 文書」＋「後から加わった文書群」を見る。variant がタプルから構造体になる破壊的変更だが、**`DomNode` の variant は `dom.rs` の外で構築も分解もされていなかった**ので波及は無かった
- **設計の要点は「関数はモデルを借用できない」こと**。`document()` は変換開始前に登録され変換全体より長く生きるので、`&DomModel` を持てない。そこで**共有ハンドル `Documents`（`Rc` 内部可変）** を導入し、モデルと `document()` の実装が同じものを指すようにした。`DomModel::with_documents` で結び付ける
- 取得は `DocumentSource` トレイト（既定は `NoDocuments` = 何も無い）。**I/O は `Loader` と同じくオプトイン** — 既定で外部文書を取りに行かない。`LoadedDocuments` が `Loader` と `Documents` を繋ぐ
- 同一 URI は 1 回だけ取得し、以後同じノードを返す（§12.1。`document('a') | document('a')` が 1 ノードであることをテスト）
- **`Model::root()` はノード自身の文書の根を返す** — `document()` で得たノードに適用したテンプレート中の `/` はその文書の根を意味する
- **文書間の順序は XPath 1.0 §5 が実装依存と明言**しているので**決定 9 の対象**。文書単位でまとまる（交互にならない）順序を採用し、レポートに実測を出す
- **RTF は結局この節点空間を要さなかった**: §11.1 が RTF に許すのは「文字列」と「`xsl:copy-of`」の 2 つだけ。前者は従来どおり文字列値で足り、後者は**エンジン自身の結果文書内の複製**で済む（ソースモデルを一切経由しない）。よって `Binding` に `fragment: Option<NodeId>` を持たせるだけで済んだ
  - RTF を運べる式は変数参照しかない（§11.1）ので、`xsl:copy-of` が `$name` 形だけを見るのは近道ではなく**場合分けの全体**
  - **`exsl:node-set()`（Phase 6.5）は依然として要 `Documents`**: RTF を node-set に昇格させるには結果木の断片をモデルの節点空間に置く必要がある。その置き場は 6c で用意できたので、6.5 は「断片を `Documents` に加える」だけになる
- **既知の逸脱**: 相対 URI は**常にスタイルシート要素の基底 URI**に対して解決する。§12.1 は node-set 引数の場合「各ノードの基底 URI」と規定するが、XDM の `Model` はノードごとの基底 URI を持たない。第 2 引数も同様。ドキュメントに明記
- **成果物**: `DocumentId` / `Documents` / `DomModel::with_documents`（xdm）、`DocumentSource` / `NoDocuments` / `LoadedDocuments` / `Transform::run_with_documents`（xslt）（統合テスト 16 + doctest 2）
- **完了条件**: `document()` が別文書の木を走査・照合でき、`xsl:copy-of "$rtf"` が木を複製する

**6d. 処理の制御と出力**

2 分割する。「スタイルシートが処理について言うこと」（6d-1）と「結果をどう書き出すか」（6d-2）は、触る場所（engine / serializer）も検証の仕方も別。

**6d-1. 処理の制御**（§3.4、§7.1.1、§2.5、§15）✅ 完了
- **`xsl:strip-space` / `xsl:preserve-space`**（§3.4）: 既定は「ソースの空白は保つ」。競合解決は §5.5 と同じ（名前 0 / `prefix:*` -0.25 / `*` -0.5、インポート優先順位が先）。`xml:space="preserve"` はスタイルシートに優先し、**最も近い宣言が決める**
  - **既知の逸脱**: 除去は**エンジンがノードリストを取る箇所**（組み込み規則の子・`apply-templates`・`for-each`）で行う。仕様上は「ソース木そのものから除去」なので、`count(//text())` のような **XPath 式の内部**からは除去前のノードが見える。モデル側で濾すには `Model` をラップする必要があり、`Functions<M>` が `M` に固定されているため型が合わなくなる。6e の適合スイートで影響を測る
- **`xsl:namespace-alias`**（§7.1.1）: リテラル結果要素・属性の名前空間を差し替える。`#default` は既定名前空間を指すが、**`in_scope_namespaces` は既定名前空間を意図的に落としている**（XPath の接頭辞は既定名前空間を指せないため）ので、別途 `default_namespace` を引く
- **`exclude-result-prefixes` は実質不要**だった: 5c の判断で**名前空間宣言をそもそも複製していない**（要素は名前と名前空間を保ち、シリアライザが必要な宣言だけ書く）。除外すべきものが最初から結果に出ない。テストで確認済み
- **前方互換処理**（§2.5）+ **`xsl:fallback`**（§15）: `version` が 1.0 より大きいモジュールでは、未知の XSLT 要素は**到達して初めて**問題になり、`xsl:fallback` があればそれを走らせる。数値として読めない `version` は「後の XSLT」ではないので 1.0 扱い（未知要素はエラーのまま）
- **成果物**: `SpaceRule` / `NameTest` / `NamespaceAlias`（統合テスト 20）
- **完了条件**: §3.4 の競合解決と `xml:space`、§7.1.1 の別名、§2.5 の前方互換が通る

**6d-2. 出力**（§16）✅ 完了
- **`xsl:output` は「1 つが勝つ」ではなく属性ごとに併合**（§16）。あるモジュールが encoding を、import 元が indent を決めてよい。同じ属性を 2 つが設定したらインポート優先順位が高い方。**`cdata-section-elements` だけは全宣言の和**
- **HTML メソッドは XML ではない**（§16.2）。空要素は `<br>`（自己終了スラッシュ無し）、`script` / `style` の中身は**エスケープしない**（HTML パーサが復元しないので、エスケープすると意味が変わる）、PI は `>` で閉じる、XML 宣言を書かない
- **`disable-output-escaping`**（§16.4）: 印は**テキストではなくノードに付ける**。同じ文字列が複製やリテラルで結果に来た場合はきちんとエスケープされる
- **字下げ**は「兄弟が全て要素」の場所にだけ入れる。テキストの隣に改行を足すとそのテキストが変わってしまうため。先頭に余計な改行を出さない条件も入れた（テストで発覚）
- **出力エンコーディング**: UTF-8 以外は `encodings` feature（xslt 側にも追加）。**無い場合はエラーで feature 名を告げる** — 宣言が Shift_JIS と言っているのに UTF-8 バイトを書く、という静かに壊れた出力を出さない。パーサの入力側と同じ方針
- **成果物**: `output` モジュール、`Output`、`ResultTree::serialize()` / `to_bytes()`（統合テスト 21 + doctest 1）
- **完了条件**: 3 メソッドと §16 の属性が通り、エンコーディングは書けるか名前付きで断るかのどちらか

**6e. 適合スイート** ⚠️ ハーネスのみ完了・**測定は未実施**
- OASIS/Xalan の XSLT 1.0 テストスイートを **`XSLTCONF` env var 方式**で取り込むハーネスを実装（W3C XML スイートと同じ方式）。カタログ（`catalog.xml`）を xylograph 自身で読み、`major-path` / `file-path` から各ケースのパスを組み立て、`operation`（standard / compile-error / execution-error）に応じて判定する
- **比較の正規化**: 適合ケースの期待結果はファイルで与えられるが、同じ結果木を別の書き方で書いた処理系同士は**バイト比較では不一致になる**（`<a/>` と `<a></a>` など）。そこで XML 比較では**両辺を本実装のシリアライザで書き直してから**比較する — 差が出たらそれは書き方ではなく木の差。テキスト比較は正規化不要なので厳密比較。HTML / Manual 比較のケースは**スキップして計上**（黙って通さない）
- **閾値は測ってから**: `XSLTCONF_MAX_FAILURES` を指定したときだけ失敗数を assert する。**誰も測っていない閾値は閾値ではない**ため、既定ではレポート出力のみ
- **ハーネス自体は検証済み**: スイートは本リポジトリに無いので通常の実行では常にスキップされ、**壊れたハーネスときれいなスキップは見分けが付かない**。そこで小さなスイートをテスト内で組み立て、カタログ読み取り・パス組み立て・実行・一致/不一致の判定（および上記の正規化が書き方の差を吸収すること）を検査する
- **残っていること**: 実スイートを入手して走らせ、**合格率を測り、非合格を既知の逸脱として文書化する**こと。6d-1 の空白除去の逸脱がどれだけ効くかもここで分かる。ハーネスが用意できたので、あとはスイートを `XSLTCONF` に指す作業
- **完了条件（Phase 6 全体）**: 合格率を公表できる水準（目標 95%+、非合格は既知の逸脱として文書化）— **未達**

### Phase 6.5 — EXSLT（決定 5）

**順序を変更した**: ROADMAP 当初は common → strings → math → sets の順だったが、**`exsl:node-set()` だけがエンジンの協力を要する**（結果木の断片をモデルの節点空間に渡す必要がある）。値だけで完結するモジュールを先に片付け、エンジン側の受け渡しを要するものを後に回す。

**6.5a. クレートと値だけで完結するモジュール** ✅ 完了
- 新クレート `xylograph-exslt`。**エンジンに組み込まない** — 決定 5 のとおり、Phase 5d の拡張関数レジストリの**最初の利用者**として、通常の拡張関数として登録する。「レジストリの設計が正しかったか」の検査でもある
- **モジュールごとに feature**（`common` / `math` / `sets`、既定 ON）。`function-available()` は**レジストリに直接訊く**ので、feature 状態と自動的に一致する（手で同期させるものが無い）。CI で各 feature 単独ビルドを走らせて確認
- **`math`**: min / max / lowest / highest / abs / sqrt / power / exp / log / 三角関数 / atan2 / constant
  - `min` / `max` は空 node-set で NaN、`lowest` / `highest` は空 node-set（前者は数、後者はノードを返すため）
  - **数でないノードが 1 つでもあれば全体が NaN** — その上で算術した場合と同じ答え
  - `constant` は EXSLT の綴り `SQRRT2` と通常の `SQRT2` の両方を受ける
- **`sets`**: difference / intersection / distinct / has-same-node / leading / trailing
  - **同一性による比較と文字列値による比較を混同しない**。difference / intersection / has-same-node は**ノードの同一性**、distinct は**文字列値**（これがグループ化に使える理由）。取り違えると「もっともらしいが間違った」答えになるので、各関数にどちらかを明記
- **`common`**: `object-type()` のみ。**`node-set()` は未実装**（`function-available()` が false と答える）— 断片の文字列で答えるのは「静かに間違い」なので、無いことを正しく主張する
- **feature が全て OFF のビルドも正当**なので、共有ヘルパは使う feature で個別に `#[cfg]` する（`-D warnings` の dead_code を避けるため）
- **成果物**: `xylograph-exslt` クレート、`register()` / `modules()`（統合テスト 19 + ユニット 4 + doctest 3）
- **完了条件**: 3 モジュールが XSLT から呼べ、feature 単独ビルドが通る

**6.5b. `exsl:node-set()` と `exsl:document`**
- RTF を `Documents` に加えてモデルの節点空間に載せる受け渡しをエンジンに追加（6c が置き場を用意済み）
- `exsl:document` は複数結果文書の書き出し

**6.5c. `strings` / `functions` / `dates-and-times` / `regular-expressions`**
- **完了条件（Phase 6.5 全体）**: 各モジュールの EXSLT 公式サンプルが通る。libxslt との差分テスト

### Phase 7 — 統合 API とツール

- `javax.xml.transform` 相当のファサード（Source/Result の組み合わせ、`URIResolver`, `ErrorListener`, パラメータ受け渡し）
- CLI（`transform` / `xpath` / `validate` / `format`）
- ドキュメント、Java からの移行ガイド

### Phase 8 — 品質と性能

- ファジング（`cargo-fuzz`）、パーサとXPathに対する差分テスト（libxml2 / Xalan と比較）
- ベンチマーク（XSLTMark 相当）、パターンインデックスとキーのチューニング
- メモリ使用量の削減

### Phase 9 — 拡張（スコープ外。需要が高い順）

- XML Catalog（OASIS）
- XPointer `xpointer()` scheme（XPath 1.0 ベースなので Phase 4 の資産で実装可能）
- DOM Level 3 の Load & Save、Traversal & Range
- ストリーミング XPath のサブセット
- XPath 2.0 / XSLT 2.0 への道筋検討

---

## 6. 確定した設計方針

| # | 論点 | 決定 | 影響 |
|---|---|---|---|
| 1 | DTD 妥当性検証 | **ゴールに含める** | Phase 2 を 2a（DTD 情報）/ 2b（検証）に分割。検証は `xylograph-validate` に独立（決定 8）。完了条件に xmlconf invalid 群の全件検出を追加 |
| 2 | `xsl:sort` の照合 | **ICU（ICU4X）依存で可** | `icu_collator` を feature `icu`（既定 ON）で導入。`lang` / `case-order` を CLDR 準拠に。`lang()` 関数の BCP 47 処理にも流用。データサイズ対策としてロケール絞り込みビルドを用意 |
| 3 | API 設計 | **W3C 規定のインタフェースは踏襲、それ以外は Rust 的に再設計** | DOM Level 3 Core はメソッド名・例外コードまで規定どおり（命名のみ snake_case）。live NodeList も維持。パーサ設定・変換駆動・エラー通知・CLI は型付きビルダーと `Result` で再設計し、ファクトリ + 文字列 feature 方式は採らない |
| 4 | 非 UTF エンコーディング | **外部ライブラリに委譲** | 自前は UTF-8/16・ASCII・Latin-1 まで。以降は `encoding_rs`（feature `encodings`、既定 ON）。`Decoder` トレイトで抽象化し差し替え可能に。出力側は符号化不能文字を文字参照へフォールバック |
| 5 | EXSLT | **最初から入れる** | Phase 5 で拡張関数の登録機構を先に作り、EXSLT をその最初の利用者にする。RTF は `exsl:node-set()` でゼロコピー昇格できる内部表現にする。Phase 6.5 として common → strings → math → sets → functions → dates → regex の順に実装 |
| 6 | XInclude / XML Base / xml:id | **必須。feature + 実行時フラグで切替** | XML Base と xml:id は Phase 2c、XInclude は Phase 3.5（XPointer framework / `element()` / `xmlns()` を含む）。基底 URI の起点は実体の system ID なので **Phase 1 の実体スタックに system ID を持たせる**。XInclude の実行時既定は無効（JAXP 準拠）、XML Base / xml:id は既定有効 |
| 7 | パーサの I/O とイベント API | **Sans-I/O コア + 同期／非同期ドライバ。カーソル API が一次、所有イベント `Iterator` はその上のラッパ** | 下記「決定 7 の詳細」。`tokio` は feature（既定 OFF）に隔離 |
| 8 | 検証と XSD | **検証はスキーマ非依存レイヤー（`xylograph-validate`）。XSD は将来トラックとして設計余地のみ確保** | `Validator` / `ErrorListener` を DTD・XSD 共通に。DTD 検証器が最初の実装。XSD（`xylograph-xsd`）は本線完了後、実用サブセットから。後付けで既存を作り直さない設計 |
| 9 | 未規定動作の扱い | **「仕様が未定義」「本実装が選択」「ビルド/環境依存」を文書上はっきり区別し、実測レポートをテストとして出力する** | 下記「決定 9 の詳細」 |
| 10 | 準拠仕様の出典 | **各クレートの doc に、実装の根拠となる仕様書を版固定 URL で明示する** | 下記「決定 10 の詳細」 |

### 決定 10 の詳細 — 何に基づいて実装したかを示す

準拠すべき公式文書がある実装には、**どの文書のどの版に基づくか**をクレートの doc コメント先頭に記す。レビューアが「§4.4 の本文と、この関数の挙動は一致しているか」を自分で確かめられるようにするため。

規約:

- **日付入り URL を使う**（`https://www.w3.org/TR/2008/REC-xml-20081126/`）。「最新版」URL（`/TR/xml/`）は指す先が将来変わるので、実装時に読んだ本文を特定できない
- 版・勧告日を併記する
- 各クレートの `lib.rs` 冒頭に `# Specifications` 節を置き、そのクレートが実装している文書だけを挙げる
- 条項に依存する実装には、コード近傍のコメントで節番号を示す（既に `§3.7`、`§4.2` のように記述している）
- **URL は追加時に実際に取得して**題名・版・勧告日が一致することを確認する。誤った出典はレビューアの信頼を損なうので、書きっぱなしにしない

W3C 勧告でないもの（EXSLT など）は、その旨を明記して業界標準として扱う。

### 決定 9 の詳細 — 未規定動作を区別して記録する

XML・XPath は少なからぬ点を開いたままにしている。**その 3 種類を混ぜて書かない**ことを規約とする。

| 区分 | 意味 | 読者が取るべき行動 |
|---|---|---|
| **仕様が未定義** | 仕様が定めていない、または「implementation-dependent」と明言している | 依存した文書は移植性がない。他実装で異なりうる |
| **本実装が選択** | 仕様が幅を許しており、本実装がその中の 1 点を選んだ | プラットフォーム間で安定。変更時は changelog に記載 |
| **ビルド/環境依存** | 同じライブラリでもビルドによって変わりうる（feature、外部クレートの有無など） | ビルドを固定しないと依存できない |

仕様が**固定している**動作はここに入れない。それはアサーションを持つ通常のテストに属する。

**実測レポート**: `crates/xylograph/tests/behaviour.rs` が上記 3 区分でレポートをコンソールへ出力する。文書の再掲ではなく**実際に動かして観測**するので、コードが変わればレポートも変わる（記述と実装が乖離しない）。CI で毎回出力し、適合率の数字と並べて記録に残す。

```bash
cargo test -p xylograph --all-features --test behaviour -- --nocapture
```

レポート自身の健全性テスト（観測が空でない・仕様の記述が空でない）も同居させ、腐りを防ぐ。実際、初回作成時にこの自己チェックが「既定名前空間があると `/r` が一致しない」という**私自身の式の誤り**を捕まえている。

### 決定 7 の詳細 — Sans-I/O パーサ

パーサ本体は I/O を持たず、バイト列を与えられて状態を進めるだけの機構にする。実際に読むのはドライバだけ。

```
xylograph-parser                        I/O を一切持たない状態機械
  ├── Reader<R: Read>                   同期ドライバ（既定）
  ├── AsyncReader<R: AsyncRead>         非同期ドライバ（feature = "tokio"、既定 OFF）
  └── SliceReader<'a>                   メモリ上のバイト列
```

```rust
// コア: 誰がバイトを運んでくるかを知らない
parser.feed(&bytes, last)?;             // バイトを渡す
match parser.next()? {
  Progress::Event(_)      => { /* アクセサで borrow して読む */ }
  Progress::NeedMoreInput => { /* ドライバが補充する */ }
  Progress::NeedEntity(r) => { /* ドライバが解決して feed する */ }
  Progress::Eof           => {}
}
```

**この形にする理由は非同期対応だけではない。** 外部実体、`xsl:import` / `xsl:include`、XInclude、`document()` はいずれも**パースの途中で新しいリソースの取得**を必要とする。コアが「実体 X が要る」と返して呼び出し側が取得する形なら、`UriResolver` に同期版と非同期版の両方を後から被せられる。パーサが内部で直接 `Read` を呼ぶ構造にすると、非同期対応はパーサの書き直しになる。

**代償と緩和策**: トークナイザが任意のバイト境界で中断・再開できる必要がある。完全な suspendable state machine は実装量が大きいので、**「不完全なトークンはバッファに残し、補充後にトークン先頭から再スキャンする」**方式を採る。再スキャン量はトークン長で抑えられ（実体展開と同じ予算管理に乗る）、実装とデバッグが大幅に楽になる。

**イベント API は 2 層**（決定 3 の「W3C 規定は踏襲、それ以外は Rust 的に」に沿う）:

| 層 | 形 | 位置づけ |
|---|---|---|
| カーソル | `reader.next()? -> Option<Event<'_>>`、値はリーダのバッファから借用 | 一次 API。割当なし。Java の `XMLStreamReader` に対応 |
| 所有イベント | `reader.into_events() -> impl Iterator<Item = Result<OwnedEvent>>` | 薄いラッパ。`Iterator` が要る場面と、イベントを溜めたい場面向け |
| push | `Handler` トレイトへの送出 | SAX 相当。カーソルの上に載せる |

### 決定 8 の詳細 — スキーマ非依存の検証フレームワーク

検証は「イベント列に対する制約検査」であり、スキーマ言語に依存しない。`xylograph-validate` に共通インタフェースを置き、各スキーマ言語はその実装として載る。Java の `Schema` / `ValidatorHandler` と同型。

```rust
// xylograph-validate（スキーマ言語非依存）
pub trait Validator {
  fn start_element(&mut self, name: QName, attrs: &[AttributeRef<'_>]) -> Result<()>;
  fn characters(&mut self, text: &str) -> Result<()>;
  fn end_element(&mut self, name: QName) -> Result<()>;
  fn finish(&mut self) -> Result<()>;              // 文書末の検査（IDREF 解決など）
}

pub trait Schema {                                  // コンパイル済みスキーマ
  fn validator(&self) -> Box<dyn Validator>;        // 文書ごとに新しい検証器
}

pub trait ErrorListener {                           // warning / error(recoverable) / fatal
  fn error(&mut self, e: &Error) -> ControlFlow<()>;
  // ...
}
```

`ValidatingReader = Reader + Box<dyn Validator>` が、読み取ったイベントをパースと検証の両方へ流す。この `Validator` を実装すれば何でも検証器になる。

**想定する実装例（拡張性の実証）**:

| 実装 | クレート | 位置づけ | 備考 |
|---|---|---|---|
| DTD 検証器 | `xylograph-validate` | Phase 2b（最初の実装） | parser が公開する DTD モデルを読む |
| RELAX NG | `xylograph-relaxng`（想定） | 将来トラック | **微分（Brzozowski derivative）アルゴリズム**がイベント列検証そのもので、`Validator` に素直に写る。XSD より小さく綺麗に嵌まる良い候補 |
| XSD 1.0 | `xylograph-xsd`（想定） | 将来トラック | 実用サブセットから（識別制約・redefine・PSVI は当初除外） |
| ユーザ独自 | 利用側 | 常時可能 | 「`<price>` は正数」等のルールや Schematron 風の検証を `Validator` 実装として差し込める |

**設計の試金石**: 「RELAX NG の微分アルゴリズムがこのトレイトに素直に写るか」。Phase 2b で `Validator` を定義する際、DTD 都合でインタフェースを歪めない（DTD 固有の内容モデルや ATTLIST はインタフェースに露出させない）ことを、この写像で検証する。

**境界（DTD の特殊性）**: DTD は検証と解析が絡む（実体展開・属性デフォルト・ID 型付けは「解析側」＝ Phase 2a で実装済み）。フレームワークの差し替え対象は制約検査のみで、実体・デフォルトの補完ではない。RELAX NG・XSD・ユーザ独自はイベント列 + 名前空間だけを必要とするため、この境界により綺麗に載る。

### 決定から導かれる依存クレート

| クレート | 用途 | feature | 既定 |
|---|---|---|---|
| `encoding_rs` | 非 UTF 符号化 | `encodings` | ON |
| `icu_collator` + `icu_locale` | 照合順序・BCP 47 | `icu` | ON |
| `tokio`（`AsyncRead` のみ） | 非同期ドライバ | `tokio` | **OFF** |
| （検討）`icu_properties` | Unicode 文字クラス表の生成元 | build 時のみ | — |

`--no-default-features` でこれらを外した最小構成でもビルドと XSLT 変換が通ること（`lang` 指定と非 UTF 符号化のみ機能低下）を CI で検証する。

### feature フラグ一覧

| feature | 内容 | 既定（ビルド） | 既定（実行時） |
|---|---|---|---|
| `validating` | DTD 妥当性検証 | ON | 無効（明示的に有効化） |
| `xml-base` | `xml:base` の解釈 | ON | 有効 |
| `xml-id` | `xml:id` の ID 型扱い | ON | 有効 |
| `xinclude` | XInclude + XPointer | ON | **無効**（SSRF/ファイル読取のリスクがあるため） |
| `encodings` | 非 UTF 符号化（`encoding_rs`） | ON | 有効 |
| `icu` | ICU 照合（`xsl:sort/@lang`） | ON | 有効 |
| `exslt-*` | EXSLT 各モジュール | ON | 有効 |
| `tokio` | 非同期ドライバ | **OFF** | — |

原則: **ビルド時 ON / 実行時は安全側**。外部リソースを取りに行く機能（XInclude・外部実体）だけ実行時既定を無効にする。feature OFF のビルドで実行時に有効化を要求した場合は `Err(UnsupportedFeature)` を返し、黙って無視しない。

### 次の着手

Phase 0。ワークスペース雛形、`xylograph-core` のエラー型・`QName`・文字クラス表・URI、`Decoder` トレイトと `encoding_rs` バックエンド。
