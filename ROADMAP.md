# xylograph ロードマップ

Rust による XML 1.0 / XPath 1.0 / XSLT 1.0 の実装計画。Java の XML API（JAXP: DOM, SAX, StAX, javax.xml.xpath, javax.xml.transform）相当の機能セットを目標とする。

---

## 1. スコープ

### 準拠仕様

| 仕様 | 版 | 位置づけ |
|---|---|---|
| XML 1.0 (5th Edition) | W3C REC | 必須（ゴール） |
| Namespaces in XML 1.0 (3rd Edition) | W3C REC | 必須（XPath/XSLT の前提） |
| XPath 1.0 | W3C REC | 必須（XSLT の前提） |
| XSLT 1.0 | W3C REC | 必須（ゴール） |
| XML Serialization (xsl:output の xml/html/text) | XSLT 1.0 §16 | 必須 |
| DTD 妥当性検証（Validity Constraints 全件） | XML 1.0 §2–§5 | **必須**（決定 1） |
| DOM Level 3 Core | W3C REC | インタフェースを踏襲（決定 3） |
| EXSLT (common / strings / math / sets / dates-and-times) | 業界標準 | **必須**（決定 5） |
| XInclude 1.0 (3rd Edition) | W3C REC | **必須**（決定 6、feature + 実行時切替） |
| XPointer Framework / `element()` / `xmlns()` scheme | W3C REC | 必須（XInclude の `@xpointer` に要る） |
| XML Base (2nd Edition) | W3C REC | **必須**（決定 6） |
| xml:id 1.0 | W3C REC | **必須**（決定 6） |
| XML 1.1 / Namespaces 1.1 | — | 対象外 |
| XPath 2.0+ / XSLT 2.0+ / XML Schema | — | 対象外 |

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
├── xylograph-parser/     # pull パーサ, DTD, 実体解決, 名前空間スタック
├── xylograph-dtd/        # 内容モデル DFA, 妥当性検証（parser から分離して単体テスト可能に）
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
   - 代償: DOM API の「ノードを他文書へ移動」「ノードだけを保持し続ける」が素直に書けない → ハンドルは `(Arc<Document>, NodeId)` の薄いラッパにする。W3C IDL をそのまま出す（決定 3）以上、`Node` を単独で持ち回れることは必須要件。
2. **文字列**: 名前（要素名・属性名・名前空間 URI）はインターン。テキストは入力バッファからの借用（`Cow`）をパーサ層では許し、DOM 構築時に所有へ倒す。
3. **XDM とツリーの分離**: XPath 評価器は `trait Node` に対して動く。これにより DOM ツリー・RTF・将来のストリーミング実装を同じ評価器で扱える。名前空間ノードは XDM 側で合成する（DOM には持たせない）。
4. **XPath は「コンパイル」する**: AST をそのまま歩くのではなく、軸走査＋述語の閉包（あるいは簡易バイトコード）へ落とす。`xsl:key` とパターンマッチはここで最適化余地が大きい。
5. **パターンのインデックス化**: テンプレート規則は「最下位ステップの名前」でバケットに分け、候補を絞ってから優先度順に照合する。
6. **RTF の表現**: 独立した小さなアリーナ（`DocumentFragment` ではなく専用ルート）とし、型システム上 node-set と区別する。ただし `exsl:node-set()`（決定 5）でゼロコピーに node-set へ昇格できるよう、**内部表現は通常のツリーと同一**にして型タグだけで区別する。
7. **拡張関数の登録機構を先に作る**: EXSLT を最初から入れる（決定 5）なら、EXSLT 自身をその機構の最初の利用者にする。組み込みハードコードにしない。
8. **パーサ本体は I/O を持たない（Sans-I/O）**: 決定 7。下記。

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

**1b. Sans-I/O トークナイザ／パーサコア**
- 要素・属性・テキスト・CDATA・コメント・PI・文字参照・定義済み実体
- 名前空間解決、`xml:space` / `xml:lang`
- `feed` / `next` と `Progress`（`Event` / `NeedMoreInput` / `NeedEntity` / `Eof`）
- 完了条件: `SliceReader` で xmlconf の not-wf / valid（DTD 非依存分）を通す

**1c. ドライバとイベント API**
- カーソル API（`Event<'_>` を借用で返す）を一次、`OwnedEvent` の `Iterator` をラッパとして提供
- `Reader<R: Read>` 同期ドライバ、`AsyncReader<R: AsyncRead>`（feature `tokio`、既定 OFF）
- 同一の適合性テストを 3 つのドライバすべてで回す（**入力の刻み方を変えても結果が変わらないこと**をランダム分割で検証）
- **完了条件**: xmlconf を全ドライバで通す。ストリーミング API が公開されている

### Phase 2 — DTD 処理（2a）と妥当性検証（2b）

**2a. DTD 情報の取得**
- 内部・外部サブセット、パラメータ実体、条件セクション
- 既定属性値、属性型、未解析実体・記法
- `standalone` 制約、`EntityResolver` 相当、セキュリティ予算
- 完了条件: xmlconf の not-wf / DTD 依存 valid ケースを通す

**2b. 妥当性検証**（決定 1）
- 内容モデルの DFA コンパイルと照合、決定性制約の検査
- 属性の妥当性、ID/IDREF、ルート要素名
- `ErrorListener` 経由の recoverable エラー報告と継続
- 完了条件: **xmlconf の invalid 群を全件検出**し、valid 群で誤検出ゼロ。既知の逸脱は文書化

**2c. XML Base / xml:id**（決定 6）
- ノードごとの基底 URI 計算（起点は実体の system ID、`xml:base` で上書き）
- `xml:id` の ID 型扱い、NCName 検査、xml:id error の報告、一意性検査（2b の ID 機構を共用）
- `ParserConfig` の実行時フラグと feature `xml-base` / `xml-id`
- 完了条件: xml:id 1.0 テストスイート、XML Base のテストケースを通す

### Phase 3 — DOM とシリアライザ

- アリーナツリー、**W3C DOM Level 3 Core の全インタフェース**（決定 3）、`DocumentBuilder`
- DOM の live NodeList / NamedNodeMap、`DOMException` コード体系
- XML シリアライザ（エンコーディング、エスケープ、indent、名前空間修復）
- SAX 相当の push アダプタ、StAX Writer
- **完了条件**: パース → DOM → 直列化のラウンドトリップが情報を落とさない

### Phase 3.5 — XInclude（決定 6）

Phase 3 の DOM（アリーナ）と Phase 2c の基底 URI / ID の上に載る後処理層。

- XPointer フレームワーク、`element()` scheme、`xmlns()` scheme、短縮ポインタ
- `xi:include` の全属性、`xi:fallback`、`parse="text"` の符号化決定
- base URI fixup / language fixup、再帰処理とループ検出、深さ・取得数の上限
- `UriResolver` 経由の取得と既定無効の実行時フラグ、feature `xinclude`
- 妥当性検証との順序を定義（検証 → 展開）
- **完了条件**: XInclude 1.0 テストスイート（W3C）を通す。fixup の ON/OFF 双方でテスト

### Phase 4 — XPath 1.0

- XDM トレイトと DOM への実装（名前空間ノード合成、文書順）
- 字句・構文解析、コンパイラ、評価器、13 軸
- コア関数 27 個、型変換
- `javax.xml.xpath` 相当の API
- **完了条件**: XPath テストスイートを通す。CLI で `xylo xpath` が動く

### Phase 5 — XSLT 骨格

- スタイルシートのパースとコンパイル、`import`/`include`、優先順位
- パターンマッチと既定優先度、モード、組み込み規則
- `apply-templates` / `call-template` / `for-each` / `if` / `choose` / `value-of` / `variable` / `param` / RTF
- **拡張関数・拡張要素の登録機構**（EXSLT の受け皿。ここで作る）と `exsl:node-set()`
- リテラル結果要素、属性値テンプレート
- テキスト出力メソッドのみ
- **完了条件**: 単純なスタイルシートが動く。テストスイートの basic 群が通り始める

### Phase 6 — XSLT 完全化

- `copy` / `copy-of` / `element` / `attribute` / `attribute-set` / `text` / `comment` / `pi`
- `sort` / `key` / `number` / `message` / `decimal-format`
- `document()` / `generate-id()` / `current()` / `system-property()` / `unparsed-entity-uri()` / `format-number()`
- `strip-space` / `preserve-space` / `namespace-alias` / `exclude-result-prefixes`
- 前方互換処理、`fallback`、`element-available` / `function-available`
- `xsl:output` 全属性、HTML 出力メソッド、`disable-output-escaping`、非 UTF 出力エンコーディング
- ICU 照合による `xsl:sort`（決定 2）
- **完了条件**: XSLT 1.0 テストスイートの合格率を公表できる水準（目標 95%+、非合格は既知の逸脱として文書化）

### Phase 6.5 — EXSLT（決定 5）

- common（`node-set`, `object-type`, `exsl:document`）→ strings → math → sets → functions → dates-and-times → regular-expressions の順
- 各モジュールを feature フラグで個別に切れる構成にし、`function-available()` が feature 状態と一致することをテストする
- **完了条件**: 各モジュールの EXSLT 公式サンプルが通る。libxslt との差分テスト

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
| 1 | DTD 妥当性検証 | **ゴールに含める** | Phase 2 を 2a（DTD 情報）/ 2b（検証）に分割。`xylograph-dtd` を独立クレート化。完了条件に xmlconf invalid 群の全件検出を追加 |
| 2 | `xsl:sort` の照合 | **ICU（ICU4X）依存で可** | `icu_collator` を feature `icu`（既定 ON）で導入。`lang` / `case-order` を CLDR 準拠に。`lang()` 関数の BCP 47 処理にも流用。データサイズ対策としてロケール絞り込みビルドを用意 |
| 3 | API 設計 | **W3C 規定のインタフェースは踏襲、それ以外は Rust 的に再設計** | DOM Level 3 Core はメソッド名・例外コードまで規定どおり（命名のみ snake_case）。live NodeList も維持。パーサ設定・変換駆動・エラー通知・CLI は型付きビルダーと `Result` で再設計し、ファクトリ + 文字列 feature 方式は採らない |
| 4 | 非 UTF エンコーディング | **外部ライブラリに委譲** | 自前は UTF-8/16・ASCII・Latin-1 まで。以降は `encoding_rs`（feature `encodings`、既定 ON）。`Decoder` トレイトで抽象化し差し替え可能に。出力側は符号化不能文字を文字参照へフォールバック |
| 5 | EXSLT | **最初から入れる** | Phase 5 で拡張関数の登録機構を先に作り、EXSLT をその最初の利用者にする。RTF は `exsl:node-set()` でゼロコピー昇格できる内部表現にする。Phase 6.5 として common → strings → math → sets → functions → dates → regex の順に実装 |
| 6 | XInclude / XML Base / xml:id | **必須。feature + 実行時フラグで切替** | XML Base と xml:id は Phase 2c、XInclude は Phase 3.5（XPointer framework / `element()` / `xmlns()` を含む）。基底 URI の起点は実体の system ID なので **Phase 1 の実体スタックに system ID を持たせる**。XInclude の実行時既定は無効（JAXP 準拠）、XML Base / xml:id は既定有効 |
| 7 | パーサの I/O とイベント API | **Sans-I/O コア + 同期／非同期ドライバ。カーソル API が一次、所有イベント `Iterator` はその上のラッパ** | 下記「決定 7 の詳細」。`tokio` は feature（既定 OFF）に隔離 |

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
