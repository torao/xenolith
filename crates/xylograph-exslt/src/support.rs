//! What the modules share: argument checking, and the conversions EXSLT leans on.
//!
//! Each helper is compiled only for the modules that use it. A build with no module at all is a
//! legitimate one — every feature is optional — and an unused helper there would be dead code
//! under the workspace's `-D warnings`.

#[cfg(any(feature = "common", feature = "math", feature = "sets"))]
use xylograph_core::error::{Error, ErrorKind, Result};
#[cfg(any(feature = "math", feature = "sets"))]
use xylograph_xdm::Model;
#[cfg(any(feature = "math", feature = "sets"))]
use xylograph_xpath::Context;
#[cfg(any(feature = "common", feature = "math", feature = "sets"))]
use xylograph_xpath::Value;

/// Checks how many arguments a function was given.
#[cfg(any(feature = "common", feature = "math", feature = "sets"))]
pub(crate) fn arity<N>(name: &str, arguments: &[Value<N>], least: usize, most: Option<usize>) -> Result<()> {
  let found = arguments.len();
  if found < least || most.is_some_and(|most| found > most) {
    let expected = match most {
      Some(most) if most == least => format!("{least} argument(s)"),
      Some(most) => format!("between {least} and {most} arguments"),
      None => format!("at least {least} argument(s)"),
    };
    let message = format!("the function \"{name}()\" needs {expected}, but was given {found}");
    return Err(Error::new(ErrorKind::XPath, message));
  }
  Ok(())
}

/// Reads an argument that has to be a node-set.
///
/// EXSLT's set and math modules are about node-sets, and a string where one was meant is a
/// mistake worth naming rather than an empty answer.
#[cfg(any(feature = "math", feature = "sets"))]
pub(crate) fn nodes<M: Model>(name: &str, value: &Value<M::Node>) -> Result<Vec<M::Node>> {
  match value {
    Value::NodeSet(nodes) => Ok(nodes.clone()),
    other => {
      let message = format!("{name}() takes a node-set, but was given {}", describe(other));
      Err(Error::new(ErrorKind::XPath, message))
    }
  }
}

/// Names a value's type for a message.
#[cfg(any(feature = "common", feature = "math", feature = "sets"))]
pub(crate) fn describe<N>(value: &Value<N>) -> &'static str {
  match value {
    Value::NodeSet(_) => "a node-set",
    Value::Boolean(_) => "a boolean",
    Value::Number(_) => "a number",
    Value::String(_) => "a string",
  }
}

/// Puts a node-set into document order with each node once, as a node-set must be.
#[cfg(any(feature = "math", feature = "sets"))]
pub(crate) fn in_document_order<M: Model>(mut nodes: Vec<M::Node>, context: &Context<'_, M>) -> Vec<M::Node> {
  nodes.sort_by(|a, b| context.model.document_order(*a, *b));
  nodes.dedup();
  nodes
}
