//! XSLT 1.0 for xylograph.
//!
//! This crate is being built up a piece at a time. So far it holds [`Pattern`], the test an
//! `xsl:template`'s `match` attribute makes on a node, and [`Stylesheet`], which reads a
//! stylesheet document into template rules and settles which of them applies to a node. The
//! engine that runs their bodies follows — see `ROADMAP.md`, Phase 5.
//!
//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XSLT 1.0] — W3C Recommendation 16 November 1999. [Patterns (§5.2)], the [conflict
//!   resolution and default priorities (§5.5)], and the [stylesheet structure (§2)] with the
//!   [import precedence of §2.6.2].
//! - [XPath 1.0] — W3C Recommendation 16 November 1999, whose paths a pattern is a subset of and
//!   whose expressions a predicate is.
//!
//! [XSLT 1.0]: https://www.w3.org/TR/1999/REC-xslt-19991116
//! [Patterns (§5.2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#patterns
//! [conflict resolution and default priorities (§5.5)]: https://www.w3.org/TR/1999/REC-xslt-19991116#conflict
//! [stylesheet structure (§2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#stylesheet-structure
//! [import precedence of §2.6.2]: https://www.w3.org/TR/1999/REC-xslt-19991116#import
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/

mod loader;
mod pattern;
mod stylesheet;

pub use loader::{Loader, NoLoader};
pub use pattern::{Alternative, Pattern};
pub use stylesheet::{Stylesheet, Template, Variable, XSLT_NAMESPACE};
