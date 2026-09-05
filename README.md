# xenolith

[![CI](https://github.com/torao/xenolith/actions/workflows/ci.yml/badge.svg)](https://github.com/torao/xenolith/actions/workflows/ci.yml)

A native Rust implementation of XML processing and XSLT 1.0, aiming for parity with Java's XML APIs (DOM, XPath, XSLT).

**Status: Phase 7.** The parser, DOM, serializer, XInclude, XPath 1.0, XSLT 1.0 and EXSLT are in
place, behind a `javax.xml.transform`-shaped facade and a command-line tool. What is measured,
what is deliberately left out and what is still to come are in [ROADMAP.md](ROADMAP.md), along
with the feature inventory and the design decisions.

Coming from Java? [MIGRATING-FROM-JAVA.md](crates/xenolith/MIGRATING-FROM-JAVA.md) maps each
JAXP API onto its counterpart here, and says where it deliberately differs.

Working on the library itself? [DEVELOPER-GUIDE.md](DEVELOPER-GUIDE.md) is the orientation: what
each crate owns, where to start reading, the invariants a change must not break, and how to run
every check CI runs.

## Crates

Depend on [`xenolith`](crates/xenolith) — one dependency that gathers the layers under one
name. The work is split into focused crates so that a caller who wants only the parser does not
compile the collation tables or the transformation engine, and the facade re-exports them:

```rust
use xenolith::parser::Reader; // the parser lives in its own crate, reached through the facade
use xenolith::{Error, QName}; // shared primitives are at the crate root
```

| Crate | Status | Contents |
| --- | --- | --- |
| [`xenolith`](crates/xenolith) | facade | the entry point; re-exports the layers below under one name |
| [`xenolith-core`](crates/xenolith-core) | Phase 0 | errors and locations, XML character classes, interned names, RFC 3986 URIs, character decoding |
| [`xenolith-parser`](crates/xenolith-parser) | Phase 3e | a namespace-aware XML pull parser with a full DTD (internal and external subsets, parameter entities), entity resolution via a resolver, attribute defaults, optional XML Base / `xml:id`, a SAX-style push adapter, and a sans-I/O core |
| [`xenolith-validate`](crates/xenolith-validate) | Phase 2c | a schema-agnostic validation framework (`Validator` / `Schema` / `ErrorListener`) with a DTD validator as its first implementation: content models, attribute and ID/IDREF constraints, root-element checking, and `xml:id` |
| [`xenolith-dom`](crates/xenolith-dom) | Phase 4a | an arena-based DOM tree (`Vec<NodeSlot>` + `NodeId`) with a W3C-shaped, Rust-idiomatic API: node kinds (attributes included), navigation, values, mutation with `DOMException`, live `NodeList` / `NamedNodeMap`, `getElementsByTagName(NS)`, `getElementById`, namespace checks, base URIs (XML Base), and `build` to make a tree from parsed XML |
| [`xenolith-serialize`](crates/xenolith-serialize) | Phase 3e | a serializer from a DOM subtree to well-formed XML text (escaping, optional XML declaration and indentation, namespace repair) and a StAX-style streaming `XmlWriter`; UTF-8 output |
| [`xenolith-xinclude`](crates/xenolith-xinclude) | Phase 3.5c | XInclude processing over a DOM: `xi:include` with `parse="xml"`/`"text"`, href resolution against the base URI, XPointer subresource selection (shorthand and `element()`), `xi:fallback`, recursion with loop detection and limits, and base URI / language fixup; resources are fetched through a caller-supplied `Loader` |
| [`xenolith-xdm`](crates/xenolith-xdm) | Phase 4d | the XPath 1.0 data model: a `Model` trait (the seven node kinds, the axis primitives, document order, string-values) and a DOM implementation that merges text and synthesizes namespace nodes without changing the tree |
| [`xenolith-xpath`](crates/xenolith-xpath) | Phase 4e | XPath 1.0, complete: a lexer that settles the language's context-dependent tokens, a recursive-descent parser, and an evaluator over the data model — all thirteen axes, node tests, predicates, the four value types and their conversions, and the whole core function library, behind a compile-once `XPath` |
| [`xenolith-xslt`](crates/xenolith-xslt) | Phase 6e | XSLT 1.0 at 93.4% of the conformance suite, all but extension elements and `func:function`: match patterns, stylesheet compilation (`xsl:import` / `xsl:include`, import precedence, conflict resolution), and an engine that runs `apply-templates`, `call-template`, `for-each`, `if`, `choose`, `value-of`, variables and parameters, the built-in rules, literal result elements and attribute value templates, plus the result-tree instructions `element`, `attribute`, `comment`, `processing-instruction`, `copy`, `copy-of` and `message` with attribute sets, `xsl:key` with `key()`, `xsl:sort` with language-aware collation, `xsl:number`, `xsl:decimal-format` with `format-number()`, `document()` over a multi-document node space, result tree fragments as trees, `xsl:apply-imports`, and XSLT's own functions `current()`, `generate-id()`, `system-property()`, `element-available()` and `function-available()`, and `xsl:output` carried out for the XML, HTML and text methods with `disable-output-escaping`, namespace declarations written for the names a result carries, and §16's default method for a result whose root is `html` |
| [`xenolith-exslt`](crates/xenolith-exslt) | Phase 6.5g | EXSLT extension functions, one feature per module: `math`, `sets`, `strings`, `dates` (reading an ISO 8601 date or time apart), `regexp` (a linear-time matcher: no backreferences, and no pattern that can be made to run for ever) and `common` (`object-type()`, `node-set()`). Nothing is built into the engine — these are registered the way any caller's functions are. `exsl:document`, which writes a result other than the principal one, is carried out by the engine through a `ResultSink` the caller supplies; without one it is refused rather than writing to a path the stylesheet chose. `functions`, where a stylesheet declares a function of its own, is still to come |
| [`xenolith-cli`](crates/xenolith-cli) | Phase 7b | the command-line tool, installed as `xenolith`: `transform`, `xpath`, `validate` and `format`. A binary, not a library — nothing you depend on pulls in an argument parser |

Every library crate is re-exported through the facade. `xenolith-cli` is the exception: it is a
binary, installed rather than depended on, and what it puts on the path is called `xenolith`.

## The command line

```bash
cargo install --path crates/xenolith-cli
```

Every subcommand reads a named file or, with none, standard input, and writes to standard output
unless told otherwise — so they compose with the rest of a pipeline:

```bash
xenolith transform --param year=2026 report.xsl data.xml
xenolith xpath --namespace h=http://www.w3.org/1999/xhtml '//h:a/@href' page.xhtml
xenolith validate data.xml
xenolith format --indent 4 data.xml
```

It exits `0` when it did what was asked, `1` when the document answered no — invalid, or nothing
selected under `--fail-on-empty` — and `2` when the request could not be carried out at all: a
file that is not there, XML that is not well-formed, a stylesheet that is not one. A script can
tell "the answer is no" from "I could not ask". Diagnostics and what `xsl:message` said go to
standard error, leaving the result alone on standard output.

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

The Java migration guide is included into the facade's documentation rather than only linked, so
its examples are compiled and run by that same command. A guide that had drifted from the API
fails the build instead of misleading a reader.

### Feature flags

Optional capabilities are compiled in by default and are switched on or off at run time. A
build with everything removed still works, with reduced functionality:

```bash
cargo test --workspace --no-default-features
```

| Feature | Default | Effect |
|---|---|---|
| `encodings` | on | Encodings beyond UTF-8, UTF-16, US-ASCII and ISO-8859-1, via `encoding_rs`. Without it those report an error giving the feature |
| `parse` | on | `dom::build`, which turns parsed XML into a DOM tree |
| `exslt` | on | `exslt`, the EXSLT extension functions. Each module has a feature of its own on that crate, and `function-available()` answers from the registry, so it agrees with the build without anything keeping the two in step |
| `icu` | on | Language-aware collation for `xsl:sort`, from CLDR through ICU4X. Without it a text sort compares by Unicode code point. XSLT 1.0 §10 leaves the collating sequence to the processor, so this changes the *answer*, not just the speed — see the behaviour report |
| `xinclude` | off | `xinclude`, which expands `xi:include` over a DOM. Off by default: it fetches resources |
| `tokio` | off | `AsyncReader`, over `tokio`'s `AsyncRead`. Only `io-util` is pulled in; the runtime stays the caller's choice |
| `xml-base` | off | Per-node base URI computation from `xml:base` and the entity's system id (XML Base); read it with `Parser::base_uri` |
| `xml-id` | off | `xml:id` as an ID-typed attribute, with tokenized normalization; checked for NCName validity and uniqueness in the same ID space as declared IDs |

## Specifications

Each crate lists the documents it was written from, in its own documentation. The links are to
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
| [EXSLT](http://exslt.org/) | community spec, undated | `-exslt` |
| [XML Schema Part 2: Datatypes](https://www.w3.org/TR/2004/REC-xmlschema-2-20041028/) | REC 2004-10-28 | `-exslt` (`dates`, for the lexical forms and the calendar) |
| [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986) | STD 66, 2005-01 | `-core` |

Section numbers appear in the code beside the rules they implement, so a claim like "§4.4 says a
half rounds towards positive infinity" can be checked against the paragraph it cites.

## Conformance

The W3C XML Conformance Test Suite is not vendored. To run against it:

```bash
curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz && tar xf xmlts20130923.tar.gz && XMLCONF=xmlconf cargo test --workspace --test conformance -- --nocapture
```

This exercises both halves: the parser against the well-formed and not-well-formed cases, and
`xenolith-validate` against the invalid cases (89 of 97 detected; the remaining 8 are recorded
as documented deviations). CI fetches the suite on every push. Cases needing machinery a phase
does not yet have are counted as skipped and reported, never silently passed.

XPath has no official W3C suite of its own — the XML Query Test Suite covers 2.0, and the corpus
that exercises XPath 1.0 thoroughly is the OASIS/Xalan XSLT suite. OASIS no longer distributes
it; the copy that is maintained is Apache's, which imported the cases into
[`apache/xalan-test`](https://github.com/apache/xalan-test):

```bash
git clone --depth 1 https://github.com/apache/xalan-test.git xslt-conformance
XSLTCONF=xslt-conformance cargo test -p xenolith-xslt --test conformance -- --nocapture
```

**1542 of 1651 runnable cases pass: 93.4%** (measured 31 July 2026). A further 39 are not judged
— their expected result is HTML, which is neither XML to compare as a tree nor exact text to
compare as a string, so counting them either way would be dishonest. The suite's 315
error-expecting cases are reported separately, at 61.9%: what is missing there is detecting
static errors, not carrying out XSLT.

The remaining 109 failures are listed by kind in the report and grouped in
[ROADMAP.md](ROADMAP.md) §6e, along with the six real bugs the first measurement found — the
largest of which was that a result tree's namespace declarations were never written, so any
result carrying a namespace was not well-formed XML. Measuring moved the figure from 77.9% to
93.4%.

The comparison erases what the specifications say carries no meaning — the order of an element's
attributes, which prefix stands for a namespace, and how much an indenting processor indents by —
and nothing else; tests pin down both directions. Setting `XSLTCONF_MAX_FAILURES` makes the run
fail above a budget. The harness itself is checked on every run against a small suite built by
the test, so a harness that had stopped finding cases would not look like a clean skip.

## Differential testing against Java

The tests in this repository were written alongside the code they test, so they can only assert
what their author understood the specification to say. This one asks a second implementation the
same questions and requires the same answers — **Java**, whose behaviour this library sets out to
match:

```bash
XYLOGUE_JAVA=java cargo test -p xenolith-xpath --test differential -- --nocapture
```

82 expressions over two documents, evaluated by `javax.xml.xpath` and by this crate, compared as
strings. It runs on every push. Nothing has to be built first: the reference program is run in
the JDK's single-file source mode.

It found a JDK defect on its first run. `name(//processing-instruction())` answers with the
document element's name there, where §4.1 and §5.7 make it the processing instruction's target —
and the JDK contradicts itself, since `count()` and `string()` over that same node-set are right,
as is `name()` by any other route to it. That is recorded as a known difference with the
reasoning, and the test still evaluates it: an entry that stopped being a difference would fail,
rather than quietly protecting a new bug.

Property-based tests (`cargo test -p xenolith-xpath --test properties`) cover what a
hand-written case cannot: that the lexer never panics on arbitrary text, that number formatting
and parsing are inverses, and that printing an expression tree yields something that parses back
to the same tree.

## Performance

```bash
cargo bench -p xenolith
```

Measured 31 July 2026 on a release build, over generated documents of 64 and 1024 records:

| | throughput |
|---|---|
| parsing, events only | ~52 MiB/s |
| parsing into a DOM | ~28 MiB/s |
| serializing a DOM | ~130 MiB/s |
| XPath, `//book[@year > 2000]/title/text()` | ~46 MiB/s |
| XSLT, transformation alone | ~12.8 MiB/s |
| XSLT, parse → transform → write | ~7.1 MiB/s |

Compiling an expression costs 2.1 µs and a stylesheet 68 µs, so both are worth keeping rather
than redoing per document. Everything scales linearly with input size.

The benchmarks generate their documents rather than vendoring them, so the numbers can be
reproduced from a checkout, and `cargo test --benches` runs each one once — what is measured
rarely still has to keep working.

They earned their keep immediately: a benchmark that varies the *number of template rules*
showed that choosing a rule cost the whole rule set for every node, so a 512-rule stylesheet took
ten times as long over one document as a five-rule one. Real stylesheets are that size and
larger. Rules are now indexed by what their last step can match, which made that case 91% faster
and removed the dependence on rule count altogether — and a test pins the index to the answer an
exhaustive scan gives, since an index may only ever be a faster route to the same rule.

## Fuzzing

Five libFuzzer targets, over the parser, the DOM builder and serializer, the validator, XPath and
XSLT. A short run of each happens on every push; a longer one is a matter of giving a target more
time:

```bash
./fuzz/short-run.sh 60
```

The properties they check live in [`crates/xenolith-fuzz`](crates/xenolith-fuzz) rather than
inside the targets, because a fuzzer's finding is only as good as the property it was checking.
That crate is an ordinary workspace member, so `cargo test` replays the seed corpus through the
same properties on stable Rust and on every platform — a property that had rotted fails the build
instead of quietly finding nothing.

Most are "arbitrary bytes must not make it panic or hang", but three say more: what the serializer
writes must parse back to a tree that writes the same text; printing a parsed expression must
yield text that parses to the same tree; and a stylesheet that compiles must either run or fail,
with whatever it produces writable. `cargo fuzz` needs a nightly toolchain, and its runtime does
not load on Windows — run it under WSL or on Linux.

## Where the specifications do not say

XML and XPath leave a good deal open, and the three kinds of "open" are not the same: what the
specification leaves **undefined** (another implementation may differ, and a document relying on
it is not portable), what it allows a range of and **this library picks** (stable across
platforms), and what depends on the **build or platform** (which features were compiled in).
They are kept apart in the documentation, and a test prints the whole list — observed by running
the code, not copied from prose, so it cannot drift:

```bash
cargo test -p xenolith --all-features --test behaviour -- --nocapture
```

CI prints it on every run, beside the conformance figures. A behaviour the specification *does*
pin down never appears there; it belongs in a test that asserts it.

## Coming from Java

**[MIGRATING-FROM-JAVA.md](crates/xenolith/MIGRATING-FROM-JAVA.md)** is the guide: which crate
each JAXP API landed in, the same task written both ways, and what is deliberately different.
Every Rust example in it is compiled and run by `cargo test --doc`, so a guide that had drifted
from the API would fail the build rather than mislead a reader.

In short: the APIs follow their Java counterparts where the names carry meaning, so what you know
transfers. `org.w3c.dom` is in [`xenolith-dom`](crates/xenolith-dom), `javax.xml.xpath` in
[`xenolith-xpath`](crates/xenolith-xpath) — `XPath` is the environment, `XPathExpression` the
compiled expression, `Namespaces` the `NamespaceContext`, `Variables` the
`XPathVariableResolver` — and `javax.xml.transform` in `xenolith::transform`:

```rust
use xenolith::transform::{Source, Transformer};

let transformer = Transformer::compile(Source::bytes(stylesheet))?
  .with_parameter("greeting", "Good day");
let result = transformer.transform(Source::bytes(document))?;
println!("{}", result.text());
# Ok::<(), xenolith::Error>(())
```

Each crate's documentation carries its own share of the mapping too. The differences are the ones
worth reading first: a builder instead of setters, a `Result` instead of an exception, no
`ErrorListener` to install — what `xsl:message` said comes back beside the result and anything
fatal is the error of the call — and nothing fetched from outside unless you say how, so XXE is
not a setting you have to remember to turn off.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
