//! EXSLT extension functions for xylograph's XSLT 1.0 engine.
//!
//! XSLT 1.0 is a small language on purpose, and EXSLT is what the community agreed to add to it:
//! arithmetic beyond the four operators, set operations beyond union, string handling beyond
//! `substring-before`. Each module has a namespace of its own, and a stylesheet reaches one by
//! binding a prefix to it and calling the functions by that prefix.
//!
//! Nothing here is built into the engine. These are ordinary extension functions, registered the
//! way any caller's would be — EXSLT was the reason that registry was designed (see `ROADMAP.md`,
//! decision 5), and being its first user is the check that it was designed right.
//!
//! # Examples
//!
//! Registering them is the whole of the wiring; a stylesheet then reaches a module by binding a
//! prefix to its namespace. (Each module's own documentation has an example that calls it — they
//! live there because a module only exists when its feature does, and so does its example.)
//!
//! ```
//! use xylograph_dom::build;
//! use xylograph_xdm::DomModel;
//! use xylograph_xpath::Functions;
//! use xylograph_xslt::{Stylesheet, Transform};
//!
//! let stylesheet = Stylesheet::compile(
//!   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
//!         <xsl:template match="/">ran</xsl:template>
//!       </xsl:stylesheet>"#,
//!   "file:///s.xsl",
//! )?;
//!
//! let doc = build::parse("<a/>".as_bytes())?;
//! let model = DomModel::new(&doc);
//! // Whatever the caller already has, with every EXSLT module this build was made with.
//! let functions = xylograph_exslt::register(Functions::new());
//!
//! let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions)?;
//! assert_eq!(result.text(), "ran");
//! # Ok::<(), xylograph_core::Error>(())
//! ```
//!
//! # Which modules are here
//!
//! One feature per module, so a build takes only what it uses. `function-available()` answers
//! from the registry rather than from a list, so it agrees with the features the build was made
//! with without anything having to keep the two in step.
//!
//! | Feature | Namespace | What it adds |
//! |---|---|---|
//! | `common` | `http://exslt.org/common` | [`object-type()`](common); `node-set()` needs the engine to adopt a fragment, and arrives with it |
//! | `math` | `http://exslt.org/math` | [minimum, maximum, powers, logarithms and trigonometry](math) |
//! | `sets` | `http://exslt.org/sets` | [difference, intersection, and the rest](sets) |
//! | `strings` | `http://exslt.org/str` | [splitting, padding, aligning and URI escaping](strings) |
//!
//! `functions`, `dates-and-times` and `regular-expressions` arrive in later sub-phases; see
//! `ROADMAP.md`.
//!
//! # Specifications
//!
//! EXSLT is a community specification rather than a W3C Recommendation, and it has no dated
//! versions. The pages below are what each module is implemented from:
//!
//! - [EXSLT] — the index of the modules
//! - [`exslt:common`], [`exslt:math`], [`exslt:sets`]
//!
//! Where a page is silent or self-contradictory, what this implementation does is written down at
//! the function, and the `xylograph` crate's behaviour report prints it.
//!
//! [EXSLT]: http://exslt.org/
//! [`exslt:common`]: http://exslt.org/exsl/index.html
//! [`exslt:math`]: http://exslt.org/math/index.html
//! [`exslt:sets`]: http://exslt.org/set/index.html

#[cfg(feature = "common")]
pub mod common;
#[cfg(feature = "math")]
pub mod math;
#[cfg(feature = "sets")]
pub mod sets;
#[cfg(feature = "strings")]
pub mod strings;

mod support;

use std::rc::Rc;

use xylograph_xdm::Model;
use xylograph_xpath::Functions;
use xylograph_xslt::{DocumentSource, NoDocuments};

/// Adds every EXSLT module this build has to a set of functions.
///
/// `str:tokenize` and `str:split` answer with nodes and so need somewhere to put them; with this
/// they report that rather than answering. Use [`register_with`] to give them somewhere.
///
/// # Examples
///
/// See the crate documentation.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  register_with(functions, &(Rc::new(NoDocuments) as Rc<dyn DocumentSource<M::Node>>))
}

/// Adds every EXSLT module this build has, with somewhere for the functions that build trees.
///
/// `trees` must share the model's `Documents` handle, or the nodes handed back name a document
/// that model cannot read — `xylograph_xslt::TreeSpace` is the usual one.
///
/// # Examples
///
/// One handle, shared three ways: the model reads it, the transformation puts documents in it,
/// and the functions build trees in it. (The [`strings`] module's own documentation has the
/// example that calls one of the two functions this matters for.)
///
/// ```
/// use std::rc::Rc;
/// use xylograph_dom::build;
/// use xylograph_xdm::{DomModel, Documents, DomNode};
/// use xylograph_xpath::Functions;
/// use xylograph_xslt::{DocumentSource, TreeSpace};
///
/// let source = build::parse("<a/>".as_bytes())?;
/// let documents = Documents::new();
/// let model = DomModel::with_documents(&source, &documents);
/// let space: Rc<dyn DocumentSource<DomNode>> = Rc::new(TreeSpace::new(&documents));
///
/// // The set is tied to the model it will run against, which is what names the node type here.
/// let functions: Functions<DomModel<'_>> = xylograph_exslt::register_with(Functions::new(), &space);
/// # let _ = (model, functions);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[must_use]
pub fn register_with<M: Model>(functions: Functions<M>, trees: &Rc<dyn DocumentSource<M::Node>>) -> Functions<M> {
  let functions = functions;
  let _ = trees;
  #[cfg(feature = "common")]
  let functions = common::register(functions);
  #[cfg(feature = "math")]
  let functions = math::register(functions);
  #[cfg(feature = "sets")]
  let functions = sets::register(functions);
  #[cfg(feature = "strings")]
  let functions = strings::register(functions, trees);
  functions
}

/// The namespaces of the modules this build has, for a caller that wants to report them.
///
/// # Examples
///
/// ```
/// // Every module this build was made with, and nothing it was not.
/// let modules = xylograph_exslt::modules();
/// assert_eq!(modules.contains(&"http://exslt.org/math"), cfg!(feature = "math"));
/// ```
#[must_use]
pub fn modules() -> Vec<&'static str> {
  let mut modules = Vec::new();
  if cfg!(feature = "common") {
    modules.push(common::NAMESPACE);
  }
  if cfg!(feature = "math") {
    modules.push(math::NAMESPACE);
  }
  if cfg!(feature = "sets") {
    modules.push(sets::NAMESPACE);
  }
  if cfg!(feature = "strings") {
    modules.push(strings::NAMESPACE);
  }
  modules
}

// The namespaces are named even where the module was not built, so `modules()` above can list
// what a build has without the names themselves depending on the features.
#[cfg(not(feature = "common"))]
mod common {
  pub(crate) const NAMESPACE: &str = "http://exslt.org/common";
}
#[cfg(not(feature = "math"))]
mod math {
  pub(crate) const NAMESPACE: &str = "http://exslt.org/math";
}
#[cfg(not(feature = "sets"))]
mod sets {
  pub(crate) const NAMESPACE: &str = "http://exslt.org/set";
}
#[cfg(not(feature = "strings"))]
mod strings {
  pub(crate) const NAMESPACE: &str = "http://exslt.org/str";
}
