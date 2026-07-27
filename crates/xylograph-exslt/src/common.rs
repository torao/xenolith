//! `http://exslt.org/common` — asking what a value is.
//!
//! # `node-set()` is not here yet
//!
//! The best-known function of this module converts a result tree fragment into a node-set, and
//! it is the one thing here that the engine has to help with: a fragment lives in the engine's
//! own result document, and turning it into nodes means putting it into the *model's* node
//! space. The place for it exists — Phase 6c built the shared handle a document joins a model
//! through — but the engine does not yet hand a fragment over, and a `node-set()` that answered
//! with the fragment's string would be quietly wrong rather than absent. So it is absent, and
//! `function-available('exsl:node-set')` says so.
//!
//! # Specifications
//!
//! - [`exslt:common`](http://exslt.org/exsl/index.html)

use xylograph_xdm::Model;
use xylograph_xpath::{Context, Functions, Value};

use crate::support::arity;

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/common";

/// Adds this module's functions.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  functions.with(NAMESPACE, "object-type", |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
    arity("exsl:object-type", &arguments, 1, Some(1))?;
    Ok(Value::String(object_type(&arguments[0]).to_owned()))
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
