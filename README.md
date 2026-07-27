# xylograph

[![CI](https://github.com/torao/xylograph/actions/workflows/ci.yml/badge.svg)](https://github.com/torao/xylograph/actions/workflows/ci.yml)

A native Rust implementation of XML processing and XSLT 1.0, aiming for parity with Java's XML APIs (DOM, XPath, XSLT).

**Status: Phase 0.** The workspace and its shared primitives exist; nothing parses XML yet.
See [ROADMAP.md](ROADMAP.md) for the feature inventory, design decisions and phase plan.

## Crates

Depend on [`xylograph`](crates/xylograph) — one dependency that gathers the layers under one
name. The work is split into focused crates so that a caller who wants only the parser does not
compile the collation tables or the transformation engine, and the facade re-exports them:

```rust
use xylograph::parser::Reader; // the parser lives in its own crate, reached through the facade
use xylograph::{Error, QName}; // shared primitives are at the crate root
```

| Crate | Status | Contents |
| --- | --- | --- |
| [`xylograph`](crates/xylograph) | facade | the entry point; re-exports the layers below under one name |
| [`xylograph-core`](crates/xylograph-core) | Phase 0 | errors and locations, XML character classes, interned names, RFC 3986 URIs, character decoding |
| [`xylograph-parser`](crates/xylograph-parser) | Phase 3e | a namespace-aware XML pull parser with a full DTD (internal and external subsets, parameter entities), entity resolution via a resolver, attribute defaults, optional XML Base / `xml:id`, a SAX-style push adapter, and a sans-I/O core |
| [`xylograph-validate`](crates/xylograph-validate) | Phase 2c | a schema-agnostic validation framework (`Validator` / `Schema` / `ErrorListener`) with a DTD validator as its first implementation: content models, attribute and ID/IDREF constraints, root-element checking, and `xml:id` |
| [`xylograph-dom`](crates/xylograph-dom) | Phase 4a | an arena-based DOM tree (`Vec<NodeSlot>` + `NodeId`) with a W3C-shaped, Rust-idiomatic API: node kinds (attributes included), navigation, values, mutation with `DOMException`, live `NodeList` / `NamedNodeMap`, `getElementsByTagName(NS)`, `getElementById`, namespace checks, base URIs (XML Base), and `build` to make a tree from parsed XML |

| [`xylograph-serialize`](crates/xylograph-serialize) | Phase 3e | a serializer from a DOM subtree to well-formed XML text (escaping, optional XML declaration and indentation, namespace repair) and a StAX-style streaming `XmlWriter`; UTF-8 output |

| [`xylograph-xinclude`](crates/xylograph-xinclude) | Phase 3.5c | XInclude processing over a DOM: `xi:include` with `parse="xml"`/`"text"`, href resolution against the base URI, XPointer subresource selection (shorthand and `element()`), `xi:fallback`, recursion with loop detection and limits, and base URI / language fixup; resources are fetched through a caller-supplied `Loader` |

| [`xylograph-xdm`](crates/xylograph-xdm) | Phase 4d | the XPath 1.0 data model: a `Model` trait (the seven node kinds, the axis primitives, document order, string-values) and a DOM implementation that merges text and synthesizes namespace nodes without changing the tree |

| [`xylograph-xpath`](crates/xylograph-xpath) | Phase 4e | XPath 1.0, complete: a lexer that settles the language's context-dependent tokens, a recursive-descent parser, and an evaluator over the data model — all thirteen axes, node tests, predicates, the four value types and their conversions, and the whole core function library, behind a compile-once `XPath` |

| [`xylograph-xslt`](crates/xylograph-xslt) | Phase 6d | XSLT 1.0, in progress: match patterns, stylesheet compilation (`xsl:import` / `xsl:include`, import precedence, conflict resolution), and an engine that runs `apply-templates`, `call-template`, `for-each`, `if`, `choose`, `value-of`, variables and parameters, the built-in rules, literal result elements and attribute value templates, plus the result-tree instructions `element`, `attribute`, `comment`, `processing-instruction`, `copy`, `copy-of` and `message` with attribute sets, `xsl:key` with `key()`, `xsl:sort` with language-aware collation, `xsl:number`, `xsl:decimal-format` with `format-number()`, `document()` over a multi-document node space, result tree fragments as trees, and XSLT's own functions `current()`, `generate-id()`, `system-property()`, `element-available()` and `function-available()`, and `xsl:output` carried out for the XML, HTML and text methods with `disable-output-escaping` |

Crates for EXSLT and the CLI arrive in later phases;
each is re-exported through the facade as it lands. See the roadmap.

## Building

```bash
cargo test --workspace --all-features
```

Requires Rust 1.85 or later (edition 2024).

Code is formatted with `cargo fmt` using the settings in [rustfmt.toml](rustfmt.toml):
2-space indentation, 120-column lines. CI rejects anything unformatted.

## Documentation

Every public item carries a doc comment — `missing_docs` is a warning, and CI builds the docs
with `-D warnings`. Anything that is part of ordinary use additionally carries a runnable
`# Examples` block, so the examples are compiled and executed by `cargo test`:

```bash
cargo test --workspace --all-features --doc
```

### Feature flags

Optional capabilities are compiled in by default and are switched on or off at run time. A
build with everything removed still works, with reduced functionality:

```bash
cargo test --workspace --no-default-features
```

| Feature | Default | Effect |
|---|---|---|
| `encodings` | on | Encodings beyond UTF-8, UTF-16, US-ASCII and ISO-8859-1, via `encoding_rs`. Without it those report an error naming the feature |
| `parse` | on | `dom::build`, which turns parsed XML into a DOM tree |
| `icu` | on | Language-aware collation for `xsl:sort`, from CLDR through ICU4X. Without it a text sort compares by Unicode code point. XSLT 1.0 §10 leaves the collating sequence to the processor, so this changes the *answer*, not just the speed — see the behaviour report |
| `xinclude` | off | `xinclude`, which expands `xi:include` over a DOM. Off by default: it fetches resources |
| `tokio` | off | `AsyncReader`, over `tokio`'s `AsyncRead`. Only `io-util` is pulled in; the runtime stays the caller's choice |
| `xml-base` | off | Per-node base URI computation from `xml:base` and the entity's system id (XML Base); read it with `Parser::base_uri` |
| `xml-id` | off | `xml:id` as an ID-typed attribute, with tokenized normalization; checked for NCName validity and uniqueness in the same ID space as declared IDs |

## Specifications

Each crate names the documents it was written from, in its own documentation. The links are to
**dated** versions rather than to "latest", so that a reviewer can read the same text the code
was written against — `/TR/xml/` moves, `/TR/2008/REC-xml-20081126/` does not.

| Document | Version | Implemented in |
|---|---|---|
| [XML 1.0 (Fifth Edition)](https://www.w3.org/TR/2008/REC-xml-20081126/) | REC 2008-11-26 | `-core`, `-parser`, `-validate`, `-serialize` |
| [Namespaces in XML 1.0 (Third Edition)](https://www.w3.org/TR/2009/REC-xml-names-20091208/) | REC 2009-12-08 | `-parser`, `-serialize`, `-xdm`, `-xpath` |
| [XPath 1.0](https://www.w3.org/TR/1999/REC-xpath-19991116/) | REC 1999-11-16 | `-xdm`, `-xpath` |
| [DOM Level 3 Core](https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/) | REC 2004-04-07 | `-dom` |
| [XInclude 1.0 (Second Edition)](https://www.w3.org/TR/2006/REC-xinclude-20061115/) | REC 2006-11-15 | `-xinclude` |
| [XPointer Framework](https://www.w3.org/TR/2003/REC-xptr-framework-20030325/), [`element()`](https://www.w3.org/TR/2003/REC-xptr-element-20030325/), [`xmlns()`](https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/) | REC 2003-03-25 | `-xinclude` |
| [XML Base (Second Edition)](https://www.w3.org/TR/2009/REC-xmlbase-20090128/) | REC 2009-01-28 | `-parser`, `-dom`, `-xinclude` |
| [xml:id 1.0](https://www.w3.org/TR/2005/REC-xml-id-20050909/) | REC 2005-09-09 | `-parser`, `-validate` |
| [XSLT 1.0](https://www.w3.org/TR/1999/REC-xslt-19991116) | REC 1999-11-16 | `-xslt` |
| [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986) | STD 66, 2005-01 | `-core` |

Section numbers appear in the code beside the rules they implement, so a claim like "§4.4 says a
half rounds towards positive infinity" can be checked against the paragraph it cites. EXSLT
arrives in a later phase; see the roadmap.

## Conformance

The W3C XML Conformance Test Suite is not vendored. To run against it:

```bash
curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz && tar xf xmlts20130923.tar.gz && XMLCONF=xmlconf cargo test --workspace --test conformance -- --nocapture
```

This exercises both halves: the parser against the well-formed and not-well-formed cases, and
`xylograph-validate` against the invalid cases (89 of 97 detected; the remaining 8 are recorded
as documented deviations). CI fetches the suite on every push. Cases needing machinery a phase
does not yet have are counted as skipped and reported, never silently passed.

XPath has no official W3C suite of its own — the XML Query Test Suite covers 2.0, and the corpus
that exercises XPath 1.0 thoroughly is the OASIS/Xalan XSLT suite. The harness for that is in
place and takes the suite the same way:

```bash
XSLTCONF=xslt-conformance cargo test -p xylograph-xslt --test conformance -- --nocapture
```

It prints how many cases were judged, passed, failed and skipped, and groups the failures by
kind. **The pass rate has not been measured yet** — the suite is not vendored and has not been
run here, so no figure is claimed. Setting `XSLTCONF_MAX_FAILURES` makes the run fail above a
budget; until someone has looked at the report there is no honest number to put there. The
harness itself is checked on every run against a small suite built by the test, so a harness that
had stopped finding cases would not look like a clean skip.

A differential comparison against **Java**, whose behaviour this library sets out to match, is
built on the same expression corpus once the library is complete.

Property-based tests (`cargo test -p xylograph-xpath --test properties`) cover what a
hand-written case cannot: that the lexer never panics on arbitrary text, that number formatting
and parsing are inverses, and that printing an expression tree yields something that parses back
to the same tree.

## Where the specifications do not say

XML and XPath leave a good deal open, and the three kinds of "open" are not the same: what the
specification leaves **undefined** (another implementation may differ, and a document relying on
it is not portable), what it allows a range of and **this library picks** (stable across
platforms), and what depends on the **build or platform** (which features were compiled in).
They are kept apart in the documentation, and a test prints the whole list — observed by running
the code, not copied from prose, so it cannot drift:

```bash
cargo test -p xylograph --all-features --test behaviour -- --nocapture
```

CI prints it on every run, beside the conformance figures. A behaviour the specification *does*
pin down never appears there; it belongs in a test that asserts it.

## Coming from Java

The APIs follow their Java counterparts where the names carry meaning, so what you know there
transfers: `org.w3c.dom` in [`xylograph-dom`](crates/xylograph-dom), and `javax.xml.xpath` in
[`xylograph-xpath`](crates/xylograph-xpath) — `XPath` is the environment, `XPathExpression` the
compiled expression, `Namespaces` the `NamespaceContext`, `Variables` the
`XPathVariableResolver`. Each crate's documentation carries the mapping in full, including where
it deliberately differs.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
