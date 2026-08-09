//! `http://exslt.org/common` — asking what a value is.
//!
//! # How `node-set()` gets its tree
//!
//! The best-known function of this module converts a result tree fragment into a node-set, and
//! it is the one here that cannot be an ordinary extension function on its own. A fragment lives
//! in the engine's result document, and no expression can carry one — XSLT 1.0 §11.1 sees to
//! that — so by the time any function is called, its argument has already become a string.
//!
//! So the engine does the lifting. Seeing `exsl:node-set($x)` in an expression, it copies that
//! fragment into a document of its own, puts it in the model's node space, and binds `$x` to the
//! resulting node-set **for that expression alone**. What arrives here is therefore already a
//! node-set, and this function is the identity on one — which is also what EXSLT says it is when
//! given a node-set.
//!
//! A transformation needs somewhere to put such a tree: `xylogue_xslt::TreeSpace`, or a
//! `LoadedDocuments`, sharing the model's `Documents` handle. Without one, calling this is an
//! error saying so, rather than an answer built from the wrong thing.
//!
//! # Examples
//!
//! ```
//! use std::rc::Rc;
//! use xylogue_dom::build;
//! use xylogue_xdm::{DomModel, Documents};
//! use xylogue_xpath::Functions;
//! use xylogue_xslt::{Stylesheet, Transform, TreeSpace};
//!
//! let stylesheet = Stylesheet::compile(
//!   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
//!                       xmlns:exsl="http://exslt.org/common">
//!         <xsl:template match="/">
//!           <xsl:variable name="frag"><i>one</i><i>two</i></xsl:variable>
//!           <xsl:value-of select="count(exsl:node-set($frag)/i)"/>
//!         </xsl:template>
//!       </xsl:stylesheet>"#,
//!   "file:///s.xsl",
//! )?;
//!
//! let source = build::parse("<a/>".as_bytes())?;
//! // One handle, shared: the model reads what the transformation puts in.
//! let documents = Documents::new();
//! let model = DomModel::with_documents(&source, &documents);
//! let space = Rc::new(TreeSpace::new(&documents));
//! let functions = xylogue_exslt::register(Functions::new());
//!
//! let result = Transform::new()
//!   .run_with_documents(&stylesheet, &model, model.root_node(), functions, space)?;
//! assert_eq!(result.text().trim(), "2");
//! # Ok::<(), xylogue_core::Error>(())
//! ```
//!
//! # Specifications
//!
//! - [`exslt:common`](http://exslt.org/exsl/index.html)

use xylogue_xdm::Model;
use xylogue_xpath::{Context, Functions, Value};

use crate::support::arity;

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/common";

/// Adds this module's functions.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  functions
    .with(NAMESPACE, "object-type", |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
      arity("exsl:object-type", &arguments, 1, Some(1))?;
      Ok(Value::String(object_type(&arguments[0]).to_owned()))
    })
    .with(NAMESPACE, "node-set", |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
      arity("exsl:node-set", &arguments, 1, Some(1))?;
      match arguments.into_iter().next() {
        // Already a tree by the time it arrives — see this module's documentation.
        Some(nodes @ Value::NodeSet(_)) => Ok(nodes),
        // Anything else is a value rather than a tree, and EXSLT makes a node-set of it by
        // giving a fragment holding one text node. There is no such fragment to hand back
        // here, so this says what it cannot do instead of inventing one.
        Some(other) => {
          let message = format!(
            "exsl:node-set() converts a result tree fragment, and was given {}; \
             only a variable holding one can be converted",
            crate::support::describe(&other)
          );
          Err(xylogue_core::Error::xslt(message))
        }
        None => unreachable!("the arity was just checked"),
      }
    })
}

/// What EXSLT calls the type of a value.
///
/// The four XPath 1.0 types have the names EXSLT gives them. A result tree fragment would be
/// `RTF`, and cannot arrive here: an expression has no way to carry one — which is the same
/// reason `node-set()` is not here.
fn object_type<N>(value: &Value<N>) -> &'static str {
  match value {
    Value::NodeSet(_) => "node-set",
    Value::Boolean(_) => "boolean",
    Value::Number(_) => "number",
    Value::String(_) => "string",
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_xpath_type_has_the_name_exslt_gives_it() {
    assert_eq!(object_type::<()>(&Value::Boolean(true)), "boolean");
    assert_eq!(object_type::<()>(&Value::Number(1.0)), "number");
    assert_eq!(object_type::<()>(&Value::String(String::new())), "string");
    assert_eq!(object_type::<()>(&Value::NodeSet(Vec::new())), "node-set");
  }
}
