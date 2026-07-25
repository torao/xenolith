//! Calling the core functions.
//!
//! Only the handful a predicate cannot do without are here; the rest of the core library — the
//! string, number and remaining node-set functions — arrives with the next phase. A call to one
//! of those, or to any name in a namespace, is reported as unavailable rather than guessed at.

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_xdm::Model;

use crate::context::Context;
use crate::value::Value;

/// Calls a core function by name, with its arguments already evaluated.
pub(crate) fn call<M: Model>(
  prefix: Option<&str>,
  local: &str,
  arguments: Vec<Value<M::Node>>,
  context: &Context<'_, M>,
) -> Result<Value<M::Node>> {
  // The core functions are in no namespace; a prefixed name is an extension function, and
  // registering those is a later phase.
  if let Some(prefix) = prefix {
    return Err(unavailable(&format!("{prefix}:{local}")));
  }
  match local {
    "position" => {
      expect_arity(local, &arguments, 0)?;
      Ok(Value::Number(context.position as f64))
    }
    "last" => {
      expect_arity(local, &arguments, 0)?;
      Ok(Value::Number(context.size as f64))
    }
    "count" => {
      expect_arity(local, &arguments, 1)?;
      match &arguments[0] {
        Value::NodeSet(nodes) => Ok(Value::Number(nodes.len() as f64)),
        other => Err(argument_type("count", other.type_name(), "a node-set")),
      }
    }
    "not" => {
      expect_arity(local, &arguments, 1)?;
      Ok(Value::Boolean(!arguments[0].boolean()))
    }
    "true" => {
      expect_arity(local, &arguments, 0)?;
      Ok(Value::Boolean(true))
    }
    "false" => {
      expect_arity(local, &arguments, 0)?;
      Ok(Value::Boolean(false))
    }
    _ => Err(unavailable(local)),
  }
}

/// Checks that a call was given the number of arguments the function takes.
fn expect_arity<N>(name: &str, arguments: &[Value<N>], expected: usize) -> Result<()> {
  if arguments.len() == expected {
    return Ok(());
  }
  let message = format!(
    "the function \"{name}()\" takes {expected} argument{}, but was given {}",
    if expected == 1 { "" } else { "s" },
    arguments.len()
  );
  Err(Error::new(ErrorKind::XPath, message))
}

fn argument_type(name: &str, found: &str, expected: &str) -> Error {
  let message = format!("the function \"{name}()\" needs {expected}, but was given {found}");
  Error::new(ErrorKind::XPath, message)
}

fn unavailable(name: &str) -> Error {
  Error::new(ErrorKind::XPath, format!("no function named \"{name}\" is available"))
}
