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

| [`xylograph-xpath`](crates/xylograph-xpath) | Phase 4d | XPath 1.0: a lexer that settles the language's context-dependent tokens, a recursive-descent parser, and an evaluator over the data model — all thirteen axes, node tests, predicates, the four value types and their conversions, and the whole core function library |

Crates for the XPath evaluator, XSLT, EXSLT and the CLI arrive in later phases;
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
| `xinclude` | off | `xinclude`, which expands `xi:include` over a DOM. Off by default: it fetches resources |
| `tokio` | off | `AsyncReader`, over `tokio`'s `AsyncRead`. Only `io-util` is pulled in; the runtime stays the caller's choice |
| `xml-base` | off | Per-node base URI computation from `xml:base` and the entity's system id (XML Base); read it with `Parser::base_uri` |
| `xml-id` | off | `xml:id` as an ID-typed attribute, with tokenized normalization; checked for NCName validity and uniqueness in the same ID space as declared IDs |

## Conformance

The W3C XML Conformance Test Suite is not vendored. To run against it:

```bash
curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz && tar xf xmlts20130923.tar.gz && XMLCONF=xmlconf cargo test --workspace --test conformance -- --nocapture
```

This exercises both halves: the parser against the well-formed and not-well-formed cases, and
`xylograph-validate` against the invalid cases (89 of 97 detected; the remaining 8 are recorded
as documented deviations). CI fetches the suite on every push. Cases needing machinery a phase
does not yet have are counted as skipped and reported, never silently passed.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
