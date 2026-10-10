# xenolith

[![CI](https://github.com/torao/xenolith/actions/workflows/ci.yml/badge.svg)](https://github.com/torao/xenolith/actions/workflows/ci.yml)

xenolith is an XML 1.0 library written in safe Rust. It reads XML from byte streams, validates documents against a
DTD, processes XInclude, builds a DOM tree, and writes the result back as XML text. It follows the W3C
recommendations it implements (see [Specifications](#specifications)), and its APIs follow their Java counterparts
where the names carry meaning.

**Status.** The core crate, [`xenolith`](crates/xenolith), is being reworked around its event vocabulary. The layers
built on top of it (the XPath data model, XPath 1.0, XSLT 1.0, EXSLT and the command-line tool) are parked outside
the workspace. Each one returns once the vocabulary it builds on has settled. [ROADMAP.md](ROADMAP.md) holds the plan,
the feature inventory and the design history.

Coming from Java? [MIGRATING-FROM-JAVA.md](crates/xenolith/MIGRATING-FROM-JAVA.md) maps the JAXP APIs onto their
counterparts here. Its XPath and XSLT sections describe the parked crates.

Working on the library itself? [DEVELOPER-GUIDE.md](DEVELOPER-GUIDE.md) explains what each crate owns, where to start
reading, the invariants a change must not break, and how to run the checks CI runs.

## What it does

- Read a document as a sequence of events. An application pulls the events one by one, as in StAX, or has them
  pushed to its consumers, as in SAX.
- Build a DOM tree from the events, and send a tree back out as events.
- Validate a document against the DTD it declares, or against a DTD kept separately as a schema.
- Replace `xi:include` elements with the content of the resources they reference.
- Select the part of a resource an XPointer identifies, while the events stream by.
- Write a tree or an event sequence as well-formed XML text.

## Quick start

`Reader` and `Writer` assemble the common pipelines. `Reader` reads strictly by default, and its `with_*` methods
add a resolver, XInclude, an input encoding or parser settings.

```rust
use xenolith::{Reader, Writer};

let doc = Reader::new().document("<order><item qty='2'>pen</item></order>".as_bytes())?;
let root = doc.document_element().unwrap();
assert_eq!(doc.text_content(root), "pen");

let out = Writer::new().with_xml_declaration(true).write(&doc, Vec::new())?;
assert_eq!(
  String::from_utf8(out).unwrap(),
  "<?xml version=\"1.0\" encoding=\"UTF-8\"?><order><item qty=\"2\">pen</item></order>"
);
```

## The event pipeline

The facade is built on an event pipeline, and an application can use the pipeline directly. An `EventProducer`
sends document events to the `EventConsumer`s registered on it. `StreamSource` drives the parser over any
`std::io::Read`. `DomSource` walks a tree. `WriterSource` turns `write_*` calls into events. On the consuming side,
`DomBuilder` builds a tree, `XmlWriter` writes XML text, and the validators check what passes.

- **No tree unless one is asked for.** Events borrow their names and text from the parser's buffer, so reading costs
  no allocation per event. A consumer that has what it needs can stop the run part way through the document.
- **One role per component.** The parser reads structure only. Lexical checks (`StrictXmlConstraints`), validity
  checks, tree building and output are separate consumers, combined as needed.
- **Composable.** A `Dispatcher` sends each event to several consumers in one pass. A *transformer* is both a
  consumer and a producer, and sits between the two. `XIncludeTransformer` is one: it replaces `xi:include` in the
  events it receives and passes the result downstream.

```rust
use xenolith::event::{EventCursor, EventProducer};
use xenolith::io::StreamSource;
use xenolith::io::write::XmlWriter;

let mut writer = XmlWriter::new(Vec::new());
StreamSource::new("<order><item qty='2'>pen</item></order>".as_bytes()).add_consumer(&mut writer).emit()?;
assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<order><item qty=\"2\">pen</item></order>");
```

Beneath `StreamSource`, `io::Parser` follows a sans-I/O design. The caller feeds it bytes, and it asks the caller for
any external entity rather than fetching one itself. `StreamSource` and the `async` feature's `AsyncReader` add the
actual I/O.

## Security

xenolith is designed to read untrusted documents. These are design decisions, not a guarantee against every threat.

- **Nothing is fetched unless the application says how.** External entities, external DTD subsets and `xi:include`
  resources are loaded only through the `UriResolver` the caller supplies. This is what keeps XXE and SSRF out by
  default.
- **Resource use is limited by default.** Token length, entity expansion (depth, count and characters), element
  nesting, XInclude depth and count, what an XPointer keeps pending while it tries its parts in order, and the
  nesting of XPath expressions all have limits. Removing them takes an explicit `Limits::unlimited`.
- **No panic on bad input.** An error while reading or parsing is an `Err`. Fuzzing checks this.
- **No `unsafe`.** The workspace forbids it.
- **Few dependencies.** By default the library depends on `encoding_rs` and `thiserror` only.

## Workspace

| Crate | In the workspace | Contents |
| --- | --- | --- |
| [`xenolith`](crates/xenolith) | yes | The library: errors and locations, character classes, interned names, RFC 3986 URIs (crate root); the event vocabulary, strict checks and the `Schema`/`Validator` contract (`event`); the parser, decoding, entity resolution and writing (`io`); the DTD model, reader and validator (`dtd`); the arena DOM (`dom`); XInclude 1.0 as a pipeline stage (`xinclude`); XPointer and the filter that selects by it (`xpointer`); and the `Reader`/`Writer` facade |
| [`xenolith-fuzz`](crates/xenolith-fuzz) | yes | The properties the fuzz targets check, and their seed corpus |
| [`xenolith-xdm`](crates/xenolith-xdm) | parked | The XPath 1.0 data model over the DOM |
| [`xenolith-xpath`](crates/xenolith-xpath) | parked | XPath 1.0 |
| [`xenolith-xslt`](crates/xenolith-xslt) | parked | XSLT 1.0 |
| [`xenolith-exslt`](crates/xenolith-exslt) | parked | EXSLT extension functions |
| [`xenolith-cli`](crates/xenolith-cli) | parked | The command-line tool, installed as `xenolith` |

A parked crate is not compiled, and it still uses the vocabulary from before the rework. The plan is to bring XPath
(with its data model) into the core crate, to keep XSLT (with EXSLT) as an extension crate, and to keep the
command-line tool. [`fuzz/`](fuzz) holds the libFuzzer targets. It stays outside the workspace because it needs a
nightly toolchain.

## Feature flags

| Feature | Default | Effect |
| --- | --- | --- |
| `encodings` | on | Encodings beyond the built-in UTF-8, UTF-16, US-ASCII and ISO-8859-1, through `encoding_rs`. Without it, any other encoding is an `Error::UnsupportedFeature` |
| `async` | off | `AsyncReader`, a runtime-independent driver over `futures_io::AsyncRead` |
| `tokio` | off | Adapters from tokio's `AsyncRead` to the `async` driver. Enables `async` |

XInclude needs no feature: it fetches only through the resolver the caller supplies. XML Base and `xml:id` are
parser settings (`ParserConfig`), on by default.

A build with every optional feature removed still works:

```bash
cargo test --workspace --exclude xenolith-fuzz --no-default-features
```

## Building

```bash
cargo test --workspace --all-features
```

Requires Rust 1.85 or later (edition 2024). Code is formatted with `cargo fmt` using [rustfmt.toml](rustfmt.toml):
2-space indentation and 120-column lines. CI rejects unformatted code.

CI runs these on every push, with `-D warnings`:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features
cargo test --workspace --all-features
cargo doc --workspace --no-deps --all-features
cargo build --workspace --all-features    # on the 1.85 toolchain
```

## Documentation

Every public item has a doc comment: `missing_docs` is a warning, and CI builds the docs with `-D warnings`. Items
for ordinary use also have runnable examples, which `cargo test` compiles and runs:

```bash
cargo test --workspace --all-features --doc
```

The examples in MIGRATING-FROM-JAVA.md are not compiled. The guide is not part of the crate's documentation.

## Specifications

The links point to dated versions rather than to "latest", so a reviewer reads the same text the code was written
against. Section numbers appear in the code beside the rules they implement.

| Document | Version | Implemented in |
| --- | --- | --- |
| [XML 1.0 (Fifth Edition)](https://www.w3.org/TR/2008/REC-xml-20081126/) | REC 2008-11-26 | `io`, `dtd`, `chars` |
| [Namespaces in XML 1.0 (Third Edition)](https://www.w3.org/TR/2009/REC-xml-names-20091208/) | REC 2009-12-08 | `io`, `name`, `event::strict` |
| [XML Base (Second Edition)](https://www.w3.org/TR/2009/REC-xmlbase-20090128/) | REC 2009-01-28 | `io`, `dom`, `xinclude` |
| [xml:id 1.0](https://www.w3.org/TR/2005/REC-xml-id-20050909/) | REC 2005-09-09 | `io`, `event::validate` |
| [XInclude 1.0 (Second Edition)](https://www.w3.org/TR/2006/REC-xinclude-20061115/) | REC 2006-11-15 | `xinclude` |
| [XPointer Framework](https://www.w3.org/TR/2003/REC-xptr-framework-20030325/), [`element()`](https://www.w3.org/TR/2003/REC-xptr-element-20030325/), [`xmlns()`](https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/) | REC 2003-03-25 | `xpointer`, `xinclude` |
| [DOM Level 3 Core](https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/) | REC 2004-04-07 | `dom` |
| [RFC 3986](https://www.rfc-editor.org/rfc/rfc3986) | STD 66, 2005-01 | `uri` |

The `xpointer()` scheme is not implemented. A pointer of several parts is evaluated part by part at once, and what
a part after the first selects is kept pending until it is known that no part before it selects anything. The parked
crates list the specifications they implement in their own documentation.

## Conformance

The W3C XML Conformance Test Suite is not vendored. CI fetches it on every push. To run it locally:

```bash
curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz && tar xf xmlts20130923.tar.gz
XMLCONF=xmlconf cargo test -p xenolith --test parser_conformance -- --nocapture
XMLCONF=xmlconf cargo test -p xenolith --all-features --test validate_conformance -- --nocapture
```

The first runs the parser against the well-formed and not-well-formed cases. The second runs the DTD validator
against the invalid cases. Each invalid case the validator cannot detect yet is listed in `KNOWN_DEVIATIONS` with the
reason. A case that needs machinery not built yet is counted as skipped and reported, never passed silently.

The XSLT conformance run (the OASIS/Xalan suite) and the differential test against Java's `javax.xml.xpath` belong to
the parked crates. Their CI job is switched off until those crates return.

## Fuzzing

Three libFuzzer targets cover the parser, the DOM builder with the writer, and the validator. A short run of each
happens on every push:

```bash
./fuzz/short-run.sh 60
```

The properties live in [`crates/xenolith-fuzz`](crates/xenolith-fuzz) rather than in the targets. That crate is a
workspace member, so `cargo test` replays the seed corpus through the same properties on stable Rust and on every
platform. Most properties say that arbitrary bytes must not cause a panic or a hang. One says more: what the writer
writes must parse back to a tree that writes the same text. `cargo fuzz` needs a nightly toolchain, and its runtime
does not load on Windows, so run it under WSL or on Linux. The XPath and XSLT targets return with their crates.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
