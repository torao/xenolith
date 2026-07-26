//! XSLT 1.0 for xylograph.
//!
//! [`Stylesheet`] reads a stylesheet document into its template rules, [`Pattern`] is the test an
//! `xsl:template`'s `match` makes on a node, and [`transform`] runs the rules over a source tree
//! to build a result.
//!
//! This is XSLT's skeleton, not the whole of it: what runs is listed on [`transform`], and an
//! instruction outside that list is reported rather than skipped. The rest — `xsl:copy`,
//! `xsl:element`, `xsl:sort`, `xsl:key`, `xsl:number` and their neighbours — arrives in Phase 6;
//! see `ROADMAP.md`.
//!
//! Extension functions are registered with [`Functions`](xylograph_xpath::Functions) and handed
//! to [`Transform::run_with`]; EXSLT will be the first thing built on that.
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

mod avt;
mod engine;
mod loader;
mod pattern;
mod stylesheet;

pub use engine::{DEFAULT_MAX_DEPTH, ResultTree, Transform, transform};
pub use loader::{Loader, NoLoader};
pub use pattern::{Alternative, Pattern};
pub use stylesheet::{OutputMethod, Stylesheet, Template, Variable, XSLT_NAMESPACE};
