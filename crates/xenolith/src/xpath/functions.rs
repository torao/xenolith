//! The core function library (XPath 1.0 §4).
//!
//! This module dispatches all twenty-seven XPath 1.0 core functions by name. Core functions have no namespace, so a
//! prefixed name refers to an extension function. This implementation does not support extension functions, so it
//! reports a bound prefixed call as unavailable and an unbound prefix as an error.
//!
//! When a core function's optional argument is omitted, the context node supplies the default: node-set functions use
//! it as a singleton node-set, while string and number functions convert its string-value as required. When a function
//! needs one string from a node-set, [`Object::string`](crate::xpath::object::Object::string) uses the string-value of
//! its first node in document order. Functions such as `id()` and `sum()` that process every node handle each member
//! individually.
//!
//! String functions such as `string-length`, `substring`, and `translate` operate on Unicode scalar values, matching
//! XPath 1.0's XML character model. Java's `String.length()` counts UTF-16 code units, so supplementary characters
//! have a different length there; for example, `string-length('😀')` returns 1 here, while Java returns 2.

#[cfg(test)]
mod test;

use crate::chars::is_whitespace;
use crate::error::{Error, Result};
use crate::name::XML_NS_URI;
use crate::xpath::context::{Context, namespace_of};
use crate::xpath::model::{Model, make_node_set};
use crate::xpath::object::{Object, string_to_number};

/// Each core function by name, with the least and most arguments it takes (`None` for no most).
pub(crate) const SIGNATURES: &[(&str, usize, Option<usize>)] = &[
  // Node-set functions (§4.1).
  ("last", 0, Some(0)),
  ("position", 0, Some(0)),
  ("count", 1, Some(1)),
  ("id", 1, Some(1)),
  ("local-name", 0, Some(1)),
  ("namespace-uri", 0, Some(1)),
  ("name", 0, Some(1)),
  // String functions (§4.2).
  ("string", 0, Some(1)),
  ("concat", 2, None),
  ("starts-with", 2, Some(2)),
  ("contains", 2, Some(2)),
  ("substring-before", 2, Some(2)),
  ("substring-after", 2, Some(2)),
  ("substring", 2, Some(3)),
  ("string-length", 0, Some(1)),
  ("normalize-space", 0, Some(1)),
  ("translate", 3, Some(3)),
  // Boolean functions (§4.3).
  ("boolean", 1, Some(1)),
  ("not", 1, Some(1)),
  ("true", 0, Some(0)),
  ("false", 0, Some(0)),
  ("lang", 1, Some(1)),
  // Number functions (§4.4).
  ("number", 0, Some(1)),
  ("sum", 1, Some(1)),
  ("floor", 1, Some(1)),
  ("ceiling", 1, Some(1)),
  ("round", 1, Some(1)),
];

/// Checks that a call names a function this implementation provides and gives it a number of arguments it takes,
/// returning the reason when it does not.
///
/// The parser can validate a function's name and argument count before evaluation. This lets parsing report unknown
/// functions and invalid argument counts without evaluating arguments that could fail themselves, such as an unbound
/// variable. [`call`] uses the same signature table as a defensive check, so parsing and evaluation apply the same
/// rules.
pub(crate) fn check(prefix: Option<&str>, local: &str, given: usize) -> std::result::Result<(), String> {
  // A prefixed name is an extension function; the core library is reached only by an unprefixed one.
  if let Some(prefix) = prefix {
    return Err(match namespace_of(prefix) {
      Some(namespace) => format!("no extension function {{{namespace}}}{local} is available"),
      None => format!("the prefix \"{prefix}\" is not bound"),
    });
  }
  let Some(&(_, min, max)) = SIGNATURES.iter().find(|(name, ..)| *name == local) else {
    return Err(format!("no function named \"{local}\" is available"));
  };
  if given >= min && max.is_none_or(|max| given <= max) {
    return Ok(());
  }
  let expected = match max {
    Some(max) if max == min => format!("{min} argument{}", plural(min)),
    Some(max) => format!("{min} or {max} arguments"),
    None => format!("at least {min} argument{}", plural(min)),
  };
  Err(format!("the function \"{local}()\" takes {expected}, but was given {given}"))
}

/// Calls a core function by its unprefixed name after evaluating its arguments.
pub(crate) fn call<M: Model>(
  prefix: Option<&str>,
  local: &str,
  arguments: Vec<Object<M::Node>>,
  context: &Context<'_, M>,
) -> Result<Object<M::Node>> {
  // The parser has checked the call already; checking again keeps the arguments below from being indexed past their
  // end, whatever tree is evaluated.
  check(prefix, local, arguments.len()).map_err(Error::xpath)?;
  let model = context.model;
  match local {
    // --- Node-set functions (§4.1) ----------------------------------------------------------
    "last" => Ok(Object::Number(context.size as f64)),
    "position" => Ok(Object::Number(context.position as f64)),
    "count" => Ok(Object::Number(nodes_of(local, &arguments[0])?.len() as f64)),
    "id" => Ok(Object::NodeSet(id(&arguments[0], context))),
    "local-name" => {
      let name = first_node(local, &arguments, context)?.and_then(|node| model.expanded_name(node));
      Ok(Object::String(name.map(|name| name.local).unwrap_or_default()))
    }
    "namespace-uri" => {
      let name = first_node(local, &arguments, context)?.and_then(|node| model.expanded_name(node));
      Ok(Object::String(name.and_then(|name| name.namespace).unwrap_or_default()))
    }
    "name" => {
      let name = first_node(local, &arguments, context)?.and_then(|node| model.qualified_name(node));
      Ok(Object::String(name.unwrap_or_default()))
    }

    // --- String functions (§4.2) ------------------------------------------------------------
    "string" => Ok(Object::String(string_argument(&arguments, context))),
    "concat" => Ok(Object::String(arguments.iter().map(|argument| argument.string(model)).collect())),
    "starts-with" => {
      let (text, prefix) = (arguments[0].string(model), arguments[1].string(model));
      Ok(Object::Boolean(text.starts_with(&prefix)))
    }
    "contains" => {
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      Ok(Object::Boolean(text.contains(&needle)))
    }
    "substring-before" => {
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      let before = text.find(&needle).map(|at| text[..at].to_owned());
      Ok(Object::String(before.unwrap_or_default()))
    }
    "substring-after" => {
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      let after = text.find(&needle).map(|at| text[at + needle.len()..].to_owned());
      Ok(Object::String(after.unwrap_or_default()))
    }
    "substring" => {
      let text = arguments[0].string(model);
      let start = arguments[1].number(model);
      let length = arguments.get(2).map(|argument| argument.number(model));
      Ok(Object::String(substring(&text, start, length)))
    }
    "string-length" => Ok(Object::Number(string_argument(&arguments, context).chars().count() as f64)),
    "normalize-space" => Ok(Object::String(normalize_space(&string_argument(&arguments, context)))),
    "translate" => {
      let text = arguments[0].string(model);
      let (from, to) = (arguments[1].string(model), arguments[2].string(model));
      Ok(Object::String(translate(&text, &from, &to)))
    }

    // --- Boolean functions (§4.3) -----------------------------------------------------------
    "boolean" => Ok(Object::Boolean(arguments[0].boolean())),
    "not" => Ok(Object::Boolean(!arguments[0].boolean())),
    "true" => Ok(Object::Boolean(true)),
    "false" => Ok(Object::Boolean(false)),
    "lang" => Ok(Object::Boolean(lang(&arguments[0].string(model), context))),

    // --- Number functions (§4.4) ------------------------------------------------------------
    "number" => {
      let value = match arguments.first() {
        Some(argument) => argument.number(model),
        None => string_to_number(&model.string_value(context.node)),
      };
      Ok(Object::Number(value))
    }
    "sum" => {
      let total = nodes_of(local, &arguments[0])?.iter().map(|node| string_to_number(&model.string_value(*node))).sum();
      Ok(Object::Number(total))
    }
    "floor" => Ok(Object::Number(arguments[0].number(model).floor())),
    "ceiling" => Ok(Object::Number(arguments[0].number(model).ceil())),
    "round" => Ok(Object::Number(round(arguments[0].number(model)))),

    _ => unreachable!("check accepts only the functions in SIGNATURES, and every one is dispatched here"),
  }
}

// --- The functions that need more than a line ---------------------------------------------------

/// Returns the elements identified by the IDs in an `id()` argument.
///
/// For a node-set argument, this function reads the string-value of each node. For any other argument, it reads the
/// argument's string conversion. It splits those strings on whitespace and looks up each resulting ID.
fn id<M: Model>(argument: &Object<M::Node>, context: &Context<'_, M>) -> Vec<M::Node> {
  let lists = match argument {
    Object::NodeSet(nodes) => nodes.iter().map(|node| context.model.string_value(*node)).collect(),
    other => vec![other.string(context.model)],
  };
  let mut found = Vec::new();
  for list in &lists {
    for id in list.split(is_whitespace).filter(|id| !id.is_empty()) {
      if let Some(node) = context.model.element_by_id(id) {
        found.push(node);
      }
    }
  }
  make_node_set(context.model, &mut found);
  found
}

/// Returns the characters whose one-based positions fall within the range requested by `substring()`.
///
/// XPath rounds `start` and the optional `length`. This function keeps positions at or above the rounded start and,
/// when `length` is present, below the sum of the rounded start and length. IEEE 754 comparisons make any `NaN` bound
/// select no characters.
fn substring(text: &str, start: f64, length: Option<f64>) -> String {
  let from = round(start);
  let until = length.map(|length| from + round(length));
  text
    .chars()
    .enumerate()
    .filter(|(index, _)| {
      let position = (index + 1) as f64;
      position >= from && until.is_none_or(|until| position < until)
    })
    .map(|(_, character)| character)
    .collect()
}

/// Removes leading and trailing XML whitespace and replaces each internal run with one space, as
/// `normalize-space()` requires.
fn normalize_space(text: &str) -> String {
  text.split(is_whitespace).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Replaces each character in `text` with the character at the same position in `to`, or removes it if `to` has no
/// character at that position, as `translate()` requires.
fn translate(text: &str, from: &str, to: &str) -> String {
  let from: Vec<char> = from.chars().collect();
  let to: Vec<char> = to.chars().collect();
  text
    .chars()
    .filter_map(|character| match from.iter().position(|candidate| *candidate == character) {
      // The first occurrence in `from` is the one that counts.
      Some(index) => to.get(index).copied(),
      None => Some(character),
    })
    .collect()
}

/// Rounds `value` to the nearest integer, sending halfway values toward positive infinity, as XPath `round()`
/// requires.
///
/// Not [`f64::round`], which sends a half away from zero: XPath wants `round(-1.5)` to be `-1`.
fn round(value: f64) -> f64 {
  // Integral values, including magnitudes where f64 cannot represent a fractional part, are already rounded.
  if !value.is_finite() || value.fract() == 0.0 {
    return value;
  }
  // Not `(value + 0.5).floor()`: that addition rounds 0.49999999999999994 up to 1.0, incorrectly rounding it to 1.
  // Comparing the fractional part with 0.5 avoids adding to the original value.
  let floor = value.floor();
  let rounded = if value - floor >= 0.5 { floor + 1.0 } else { floor };
  // §4.4 is explicit that rounding a value between -0.5 and zero gives *negative* zero, which `floor` does not: it
  // returns positive zero. The two print alike, so the difference only shows through division — `1 div round(-0.5)`
  // is -Infinity, not Infinity.
  if rounded == 0.0 && value.is_sign_negative() { -0.0 } else { rounded }
}

/// Returns whether the nearest `xml:lang` value on the context node or an ancestor matches `wanted` or a sublanguage.
fn lang<M: Model>(wanted: &str, context: &Context<'_, M>) -> bool {
  let mut current = Some(context.node);
  while let Some(node) = current {
    for attribute in context.model.attributes(node) {
      let Some(name) = context.model.expanded_name(attribute) else { continue };
      if name.namespace.as_deref() == Some(XML_NS_URI) && name.local == "lang" {
        // The nearest xml:lang settles it, whether or not it matches.
        return sublanguage_of(&context.model.string_value(attribute), wanted);
      }
    }
    current = context.model.parent(node);
  }
  false
}

/// Returns whether `value` equals `wanted` or begins with `wanted` followed by a hyphen, ignoring ASCII case.
fn sublanguage_of(value: &str, wanted: &str) -> bool {
  let value = value.to_ascii_lowercase();
  let wanted = wanted.to_ascii_lowercase();
  value == wanted || value.strip_prefix(&wanted).is_some_and(|rest| rest.starts_with('-'))
}

// --- Arguments ----------------------------------------------------------------------------------

/// Returns a function's string argument, using the context node's string-value when the argument is omitted.
fn string_argument<M: Model>(arguments: &[Object<M::Node>], context: &Context<'_, M>) -> String {
  match arguments.first() {
    Some(argument) => argument.string(context.model),
    None => context.model.string_value(context.node),
  }
}

/// Returns the first node in document order from a node-set argument, or the context node when no argument is given.
/// Returns `None` when the node-set is empty and an error when the argument is not a node-set.
fn first_node<M: Model>(
  name: &str,
  arguments: &[Object<M::Node>],
  context: &Context<'_, M>,
) -> Result<Option<M::Node>> {
  match arguments.first() {
    None => Ok(Some(context.node)),
    // A node-set is kept in document order, so its first node is the first in document order.
    Some(Object::NodeSet(nodes)) => Ok(nodes.first().copied()),
    Some(other) => Err(argument_type(name, other.type_name(), "a node-set")),
  }
}

/// The nodes of an argument that has to be a node-set.
fn nodes_of<'a, N>(name: &str, argument: &'a Object<N>) -> Result<&'a [N]> {
  match argument {
    Object::NodeSet(nodes) => Ok(nodes),
    other => Err(argument_type(name, other.type_name(), "a node-set")),
  }
}

const fn plural(count: usize) -> &'static str {
  if count == 1 { "" } else { "s" }
}

fn argument_type(name: &str, found: &str, expected: &str) -> Error {
  let message = format!("the function \"{name}()\" needs {expected}, but was given {found}");
  Error::xpath(message)
}
