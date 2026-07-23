# xylograph

[![CI](https://github.com/torao/xylograph/actions/workflows/ci.yml/badge.svg)](https://github.com/torao/xylograph/actions/workflows/ci.yml)

A native Rust implementation of XML processing and XSLT 1.0, aiming for parity with Java's XML APIs (DOM, XPath, XSLT).

**Status: Phase 0.** The workspace and its shared primitives exist; nothing parses XML yet.
See [ROADMAP.md](ROADMAP.md) for the feature inventory, design decisions and phase plan.

## Crates

| Crate | Status | Contents |
| --- | --- | --- |
| [`xylograph-core`](crates/xylograph-core) | Phase 0 | errors and locations, XML character classes, interned names, RFC 3986 URIs, character decoding |
| [`xylograph-parser`](crates/xylograph-parser) | Phase 2a | a namespace-aware XML pull parser with a full DTD (internal and external subsets, parameter entities), entity resolution via a resolver, attribute defaults, and a sans-I/O core |

Crates for the DTD, DOM, XPath, XInclude, serializer, XSLT, EXSLT and the CLI arrive in later
phases; see the roadmap.

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
| `tokio` | off | `AsyncReader`, over `tokio`'s `AsyncRead`. Only `io-util` is pulled in; the runtime stays the caller's choice |

## Conformance

The W3C XML Conformance Test Suite is not vendored. To run against it:

```bash
curl -O https://www.w3.org/XML/Test/xmlts20130923.tar.gz && tar xf xmlts20130923.tar.gz && XMLCONF=xmlconf cargo test -p xylograph-parser --test conformance -- --nocapture
```

CI fetches it on every push. Cases needing machinery a phase does not yet have are counted as
skipped and reported, never silently passed.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.
