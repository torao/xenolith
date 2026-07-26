//! XSLT 1.0 for xylograph.
//!
//! This crate is being built up a piece at a time; so far it holds [`Pattern`], the test an
//! `xsl:template`'s `match` attribute makes on a node. The stylesheet model and the
//! transformation engine follow — see `ROADMAP.md`, Phase 5.
//!
//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XSLT 1.0] — W3C Recommendation 16 November 1999. [Patterns (§5.2)] and the [conflict
//!   resolution and default priorities (§5.5)] this crate implements so far.
//! - [XPath 1.0] — W3C Recommendation 16 November 1999, whose paths a pattern is a subset of and
//!   whose expressions a predicate is.
//!
//! [XSLT 1.0]: https://www.w3.org/TR/1999/REC-xslt-19991116
//! [Patterns (§5.2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#patterns
//! [conflict resolution and default priorities (§5.5)]: https://www.w3.org/TR/1999/REC-xslt-19991116#conflict
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/

mod pattern;

pub use pattern::{Alternative, Pattern};
