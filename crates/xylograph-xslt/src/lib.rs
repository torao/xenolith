//! XSLT 1.0 for xylograph.
//!
//! [`Stylesheet`] reads a stylesheet document into its template rules, [`Pattern`] is the test an
//! `xsl:template`'s `match` makes on a node, and [`transform`] runs the rules over a source tree
//! to build a result.
//!
//! This is not yet the whole of XSLT: what runs is listed on [`transform`], and an instruction
//! outside that list is reported rather than skipped. The walk and the instructions that build
//! result nodes are in place — including `xsl:element`, `xsl:attribute`, `xsl:copy`,
//! `xsl:copy-of` and [`AttributeSet`]s; `xsl:sort`, `xsl:key`, `xsl:number` and the output
//! controls are still to come. A stylesheet can ask what is here before relying on it, with
//! `element-available()` and `function-available()`. See `ROADMAP.md`.
//!
//! Extension functions are registered with [`Functions`](xylograph_xpath::Functions) and handed
//! to [`Transform::run_with`]; EXSLT will be the first thing built on that. XSLT's own functions
//! — `current()`, `generate-id()`, `system-property()` and the two above — are added to that
//! same set, in the empty namespace that XPath leaves to a host language.
//!
//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XSLT 1.0] — W3C Recommendation 16 November 1999. [Patterns (§5.2)], the [conflict
//!   resolution and default priorities (§5.5)], the [stylesheet structure (§2)] with the
//!   [import precedence of §2.6.2], [creating the result tree (§7)] — which is where
//!   `xsl:element`, `xsl:attribute`, `xsl:copy` and the [attribute sets of §7.1.4] are defined —
//!   the [additional functions of §12.4], and [what a stylesheet may ask about the processor
//!   (§15)].
//! - [XPath 1.0] — W3C Recommendation 16 November 1999, whose paths a pattern is a subset of and
//!   whose expressions a predicate is.
//!
//! [XSLT 1.0]: https://www.w3.org/TR/1999/REC-xslt-19991116
//! [Patterns (§5.2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#patterns
//! [conflict resolution and default priorities (§5.5)]: https://www.w3.org/TR/1999/REC-xslt-19991116#conflict
//! [stylesheet structure (§2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#stylesheet-structure
//! [import precedence of §2.6.2]: https://www.w3.org/TR/1999/REC-xslt-19991116#import
//! [creating the result tree (§7)]: https://www.w3.org/TR/1999/REC-xslt-19991116#section-Creating-the-Result-Tree
//! [attribute sets of §7.1.4]: https://www.w3.org/TR/1999/REC-xslt-19991116#attribute-sets
//! [additional functions of §12.4]: https://www.w3.org/TR/1999/REC-xslt-19991116#add-func
//! [what a stylesheet may ask about the processor (§15)]: https://www.w3.org/TR/1999/REC-xslt-19991116#fallback
//! [XPath 1.0]: https://www.w3.org/TR/1999/REC-xpath-19991116/

mod avt;
mod engine;
mod functions;
mod loader;
mod pattern;
mod stylesheet;

pub use engine::{DEFAULT_MAX_DEPTH, ResultTree, Transform, transform};
pub use loader::{Loader, NoLoader};
pub use pattern::{Alternative, Pattern};
pub use stylesheet::{AttributeSet, OutputMethod, Stylesheet, Template, Variable, XSLT_NAMESPACE};
