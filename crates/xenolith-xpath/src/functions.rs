//! The core function library (XPath 1.0 §4).
//!
//! All twenty-seven functions, dispatched by name. They are in no namespace, so a prefixed name
//! is an extension function — registering those is a later phase, and until then such a call is
//! reported as unavailable rather than guessed at.
//!
//! Several functions take an optional argument that defaults to the context node, and several
//! read their argument as the string-value of a node-set's first node; those conversions are the
//! ones in [`value`](crate::value), so the behaviour is the same wherever they are applied.

use xenolith_core::chars::is_whitespace;
use xenolith_core::error::{Error, Result};
use xenolith_core::name::XML_NS_URI;
use xenolith_xdm::Model;

use crate::context::{Context, normalize};
use crate::value::{Value, string_to_number};

/// Calls a core function by name, with its arguments already evaluated.
pub(crate) fn call<M: Model>(
  prefix: Option<&str>,
  local: &str,
  arguments: Vec<Value<M::Node>>,
  context: &Context<'_, M>,
) -> Result<Value<M::Node>> {
  // A prefixed name is an extension function, which the caller supplies; the core library is
  // reached only by an unprefixed one.
  if let Some(prefix) = prefix {
    return extension(prefix, local, arguments, context);
  }
  let model = context.model;
  match local {
    // --- Node-set functions (§4.1) ----------------------------------------------------------
    "last" => {
      arity(local, &arguments, 0, Some(0))?;
      Ok(Value::Number(context.size as f64))
    }
    "position" => {
      arity(local, &arguments, 0, Some(0))?;
      Ok(Value::Number(context.position as f64))
    }
    "count" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Number(nodes_of(local, &arguments[0])?.len() as f64))
    }
    "id" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::NodeSet(id(&arguments[0], context)))
    }
    "local-name" => {
      arity(local, &arguments, 0, Some(1))?;
      let name = first_node(local, &arguments, context)?.and_then(|node| model.expanded_name(node));
      Ok(Value::String(name.map(|name| name.local).unwrap_or_default()))
    }
    "namespace-uri" => {
      arity(local, &arguments, 0, Some(1))?;
      let name = first_node(local, &arguments, context)?.and_then(|node| model.expanded_name(node));
      Ok(Value::String(name.and_then(|name| name.namespace).unwrap_or_default()))
    }
    "name" => {
      arity(local, &arguments, 0, Some(1))?;
      let name = first_node(local, &arguments, context)?.and_then(|node| model.qualified_name(node));
      Ok(Value::String(name.unwrap_or_default()))
    }

    // --- String functions (§4.2) ------------------------------------------------------------
    "string" => {
      arity(local, &arguments, 0, Some(1))?;
      Ok(Value::String(string_argument(&arguments, context)))
    }
    "concat" => {
      arity(local, &arguments, 2, None)?;
      Ok(Value::String(arguments.iter().map(|argument| argument.string(model)).collect()))
    }
    "starts-with" => {
      arity(local, &arguments, 2, Some(2))?;
      let (text, prefix) = (arguments[0].string(model), arguments[1].string(model));
      Ok(Value::Boolean(text.starts_with(&prefix)))
    }
    "contains" => {
      arity(local, &arguments, 2, Some(2))?;
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      Ok(Value::Boolean(text.contains(&needle)))
    }
    "substring-before" => {
      arity(local, &arguments, 2, Some(2))?;
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      let before = text.find(&needle).map(|at| text[..at].to_owned());
      Ok(Value::String(before.unwrap_or_default()))
    }
    "substring-after" => {
      arity(local, &arguments, 2, Some(2))?;
      let (text, needle) = (arguments[0].string(model), arguments[1].string(model));
      let after = text.find(&needle).map(|at| text[at + needle.len()..].to_owned());
      Ok(Value::String(after.unwrap_or_default()))
    }
    "substring" => {
      arity(local, &arguments, 2, Some(3))?;
      let text = arguments[0].string(model);
      let start = arguments[1].number(model);
      let length = arguments.get(2).map(|argument| argument.number(model));
      Ok(Value::String(substring(&text, start, length)))
    }
    "string-length" => {
      arity(local, &arguments, 0, Some(1))?;
      Ok(Value::Number(string_argument(&arguments, context).chars().count() as f64))
    }
    "normalize-space" => {
      arity(local, &arguments, 0, Some(1))?;
      Ok(Value::String(normalize_space(&string_argument(&arguments, context))))
    }
    "translate" => {
      arity(local, &arguments, 3, Some(3))?;
      let text = arguments[0].string(model);
      let (from, to) = (arguments[1].string(model), arguments[2].string(model));
      Ok(Value::String(translate(&text, &from, &to)))
    }

    // --- Boolean functions (§4.3) -----------------------------------------------------------
    "boolean" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Boolean(arguments[0].boolean()))
    }
    "not" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Boolean(!arguments[0].boolean()))
    }
    "true" => {
      arity(local, &arguments, 0, Some(0))?;
      Ok(Value::Boolean(true))
    }
    "false" => {
      arity(local, &arguments, 0, Some(0))?;
      Ok(Value::Boolean(false))
    }
    "lang" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Boolean(lang(&arguments[0].string(model), context)))
    }

    // --- Number functions (§4.4) ------------------------------------------------------------
    "number" => {
      arity(local, &arguments, 0, Some(1))?;
      let value = match arguments.first() {
        Some(argument) => argument.number(model),
        None => string_to_number(&model.string_value(context.node)),
      };
      Ok(Value::Number(value))
    }
    "sum" => {
      arity(local, &arguments, 1, Some(1))?;
      let total = nodes_of(local, &arguments[0])?.iter().map(|node| string_to_number(&model.string_value(*node))).sum();
      Ok(Value::Number(total))
    }
    "floor" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(model).floor()))
    }
    "ceiling" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(model).ceil()))
    }
    "round" => {
      arity(local, &arguments, 1, Some(1))?;
      Ok(Value::Number(round(arguments[0].number(model))))
    }

    // Not one of the core functions. A language hosting XPath adds functions of its own with no
    // prefix — XSLT's `current()`, `key()`, `format-number()` — so the registry is asked under
    // the empty namespace before giving up.
    _ => match context.functions.and_then(|functions| functions.get("", local)) {
      Some(function) => function.call(arguments, context),
      None => Err(unavailable(local)),
    },
  }
}

/// Whether `name` is one of the core functions (XPath 1.0 §4).
///
/// The unprefixed names the language itself defines, which is what XSLT's `function-available()`
/// has to know before it looks anywhere else.
///
/// # Examples
///
/// ```
/// assert!(xenolith_xpath::is_core_function("substring-before"));
/// assert!(!xenolith_xpath::is_core_function("key"), "key() is XSLT's, not XPath's");
/// ```
#[must_use]
pub fn is_core_function(name: &str) -> bool {
  CORE_FUNCTIONS.contains(&name)
}

/// The twenty-seven names of XPath 1.0 §4, in the order the specification introduces them.
const CORE_FUNCTIONS: &[&str] = &[
  // Node-set functions (§4.1).
  "last",
  "position",
  "count",
  "id",
  "local-name",
  "namespace-uri",
  "name",
  // String functions (§4.2).
  "string",
  "concat",
  "starts-with",
  "contains",
  "substring-before",
  "substring-after",
  "substring",
  "string-length",
  "normalize-space",
  "translate",
  // Boolean functions (§4.3).
  "boolean",
  "not",
  "true",
  "false",
  "lang",
  // Number functions (§4.4).
  "number",
  "sum",
  "floor",
  "ceiling",
  "round",
];

/// Calls an extension function, the prefix resolved first.
fn extension<M: Model>(
  prefix: &str,
  local: &str,
  arguments: Vec<Value<M::Node>>,
  context: &Context<'_, M>,
) -> Result<Value<M::Node>> {
  let Some(namespace) = context.namespaces.get(prefix) else {
    let message = format!(
      "the prefix \"{prefix}\" of the function \"{prefix}:{local}\" is not bound; \
       bind it before compiling the expression"
    );
    return Err(Error::xpath(message));
  };
  let found = context.functions.and_then(|functions| functions.get(namespace, local));
  let Some(function) = found else {
    return Err(no_extension(namespace, local, context));
  };
  function.call(arguments, context)
}

/// Says an extension function is not registered, and what is.
fn no_extension<M: Model>(namespace: &str, local: &str, context: &Context<'_, M>) -> Error {
  let mut available: Vec<String> = context
    .functions
    .map(|functions| functions.names().map(|(namespace, local)| format!("{{{namespace}}}{local}")).collect())
    .unwrap_or_default();
  available.sort();
  let known = if available.is_empty() {
    "no extension functions are registered".to_owned()
  } else {
    format!("registered: {}", available.join(", "))
  };
  let message = format!("no extension function {{{namespace}}}{local} is registered; {known}");
  Error::xpath(message)
}

// --- The functions that need more than a line ---------------------------------------------------

/// `id()`: the elements the IDs in the argument point at.
///
/// A node-set argument contributes the string-value of each of its nodes; anything else is one
/// string. Either way the strings are lists of IDs separated by whitespace.
fn id<M: Model>(argument: &Value<M::Node>, context: &Context<'_, M>) -> Vec<M::Node> {
  let lists = match argument {
    Value::NodeSet(nodes) => nodes.iter().map(|node| context.model.string_value(*node)).collect(),
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
  normalize(context.model, &mut found);
  found
}

/// `substring()`: the characters whose 1-based position falls in the requested range.
///
/// Both arguments are rounded first, and the arithmetic is done in floating point, which is what
/// makes the spec's awkward cases fall out: a `NaN` bound keeps nothing, and an infinite length
/// keeps everything from the start onwards.
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

/// `normalize-space()`: leading and trailing whitespace removed, inner runs collapsed to one
/// space.
fn normalize_space(text: &str) -> String {
  text.split(is_whitespace).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `translate()`: each character replaced by the one at the same place in `to`, or removed when
/// `to` is the shorter of the two.
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

/// `round()`: the nearest integer, with a half going towards positive infinity.
///
/// Not [`f64::round`], which sends a half away from zero: XPath wants `round(-1.5)` to be `-1`.
fn round(value: f64) -> f64 {
  // An integer — and any value too large to have a fraction — is already the answer, which also
  // keeps the addition below from losing precision.
  if !value.is_finite() || value.fract() == 0.0 {
    return value;
  }
  let rounded = (value + 0.5).floor();
  // §4.4 is explicit that rounding a value between -0.5 and zero gives *negative* zero, which
  // `floor` does not: it returns positive zero. The two print alike, so the difference only
  // shows through division — `1 div round(-0.5)` is -Infinity, not Infinity.
  if rounded == 0.0 && value.is_sign_negative() { -0.0 } else { rounded }
}

/// `lang()`: whether the language in scope on the context node is `wanted`, or a sublanguage of
/// it.
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

/// Whether `value` is `wanted` or a sublanguage of it, compared without regard to case — so
/// `en-GB` answers to `en`, but `england` does not.
fn sublanguage_of(value: &str, wanted: &str) -> bool {
  let value = value.to_ascii_lowercase();
  let wanted = wanted.to_ascii_lowercase();
  value == wanted || value.strip_prefix(&wanted).is_some_and(|rest| rest.starts_with('-'))
}

// --- Arguments ----------------------------------------------------------------------------------

/// The string argument of a function that takes the context node when given none.
fn string_argument<M: Model>(arguments: &[Value<M::Node>], context: &Context<'_, M>) -> String {
  match arguments.first() {
    Some(argument) => argument.string(context.model),
    None => context.model.string_value(context.node),
  }
}

/// The first node in document order of a node-set argument, or the context node when the
/// argument was left out. `None` means the node-set was empty.
fn first_node<M: Model>(name: &str, arguments: &[Value<M::Node>], context: &Context<'_, M>) -> Result<Option<M::Node>> {
  match arguments.first() {
    None => Ok(Some(context.node)),
    Some(Value::NodeSet(nodes)) => Ok(nodes.iter().copied().min_by(|a, b| context.model.document_order(*a, *b))),
    Some(other) => Err(argument_type(name, other.type_name(), "a node-set")),
  }
}

/// The nodes of an argument that has to be a node-set.
fn nodes_of<'a, N>(name: &str, argument: &'a Value<N>) -> Result<&'a [N]> {
  match argument {
    Value::NodeSet(nodes) => Ok(nodes),
    other => Err(argument_type(name, other.type_name(), "a node-set")),
  }
}

/// Checks that a call was given a number of arguments the function accepts. `max` of `None`
/// means there is no upper bound.
fn arity<N>(name: &str, arguments: &[Value<N>], min: usize, max: Option<usize>) -> Result<()> {
  if arguments.len() >= min && max.is_none_or(|max| arguments.len() <= max) {
    return Ok(());
  }
  let expected = match max {
    Some(max) if max == min => format!("{min} argument{}", plural(min)),
    Some(max) => format!("{min} or {max} arguments"),
    None => format!("at least {min} argument{}", plural(min)),
  };
  let message = format!("the function \"{name}()\" takes {expected}, but was given {}", arguments.len());
  Err(Error::xpath(message))
}

const fn plural(count: usize) -> &'static str {
  if count == 1 { "" } else { "s" }
}

fn argument_type(name: &str, found: &str, expected: &str) -> Error {
  let message = format!("the function \"{name}()\" needs {expected}, but was given {found}");
  Error::xpath(message)
}

fn unavailable(name: &str) -> Error {
  Error::xpath(format!("no function named \"{name}\" is available"))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Every name in [`CORE_FUNCTIONS`] is one the dispatch above actually answers to.
  ///
  /// The list exists so that `function-available()` can be told the truth, and a list written
  /// beside a `match` drifts from it unless something checks. Calling each name with no
  /// arguments gives an arity error for most of them — what matters is that none of them comes
  /// back as unknown.
  #[test]
  fn the_named_core_functions_are_the_ones_that_are_dispatched() {
    use xenolith_dom::build;
    use xenolith_xdm::DomModel;

    use crate::context::{Namespaces, Variables};

    let document = build::parse("<a/>".as_bytes()).expect("well-formed");
    let model = DomModel::new(&document);
    let namespaces = Namespaces::new();
    let variables = Variables::new();
    let context = Context::new(&model, model.root_node(), &namespaces, &variables);

    for name in CORE_FUNCTIONS {
      assert!(is_core_function(name), "{name} is in the list but is_core_function says otherwise");
      if let Err(error) = call(None, name, Vec::new(), &context) {
        assert!(!error.message().contains("no function named"), "{name}: {}", error.message());
      }
    }

    // And the other way: a name that is not core is reported as unknown.
    let unknown = call(None, "key", Vec::new(), &context).expect_err("key() is XSLT's");
    assert!(unknown.message().contains("no function named"), "{}", unknown.message());
  }

  #[test]
  fn round_sends_a_half_towards_positive_infinity() {
    assert_eq!(round(1.5), 2.0);
    assert_eq!(round(-1.5), -1.0, "not away from zero, the way f64::round would");
    assert_eq!(round(2.5), 3.0);
    assert_eq!(round(1.4), 1.0);
    assert_eq!(round(-1.6), -2.0);
    assert!(round(f64::NAN).is_nan());
    assert_eq!(round(f64::INFINITY), f64::INFINITY);
  }

  #[test]
  fn rounding_towards_zero_from_below_keeps_the_sign() {
    // §4.4: round(-0.5) is -0, not 0. Both print as "0", so the sign shows only in arithmetic.
    for value in [-0.5, -0.3, -0.0] {
      let rounded = round(value);
      assert_eq!(rounded, 0.0);
      assert!(rounded.is_sign_negative(), "round({value}) should be negative zero");
    }
    assert!(round(0.3).is_sign_positive(), "round(0.3) is positive zero");
  }

  #[test]
  fn substring_handles_the_awkward_bounds_from_the_specification() {
    assert_eq!(substring("12345", 2.0, None), "2345");
    assert_eq!(substring("12345", 1.5, Some(2.6)), "234");
    assert_eq!(substring("12345", 0.0, Some(3.0)), "12");
    assert_eq!(substring("12345", f64::NAN, Some(3.0)), "");
    assert_eq!(substring("12345", 1.0, Some(f64::NAN)), "");
    assert_eq!(substring("12345", -42.0, Some(f64::INFINITY)), "12345");
    assert_eq!(substring("12345", f64::NEG_INFINITY, Some(f64::INFINITY)), "");
  }

  #[test]
  fn translate_replaces_and_removes() {
    assert_eq!(translate("bar", "abc", "ABC"), "BAr");
    assert_eq!(translate("--aaa--", "abc-", "ABC"), "AAA", "a character with no replacement is dropped");
    assert_eq!(translate("aa", "aa", "xy"), "xx", "the first occurrence in the from-string wins");
  }

  #[test]
  fn normalize_space_collapses_runs() {
    assert_eq!(normalize_space("  a  b \t\n c  "), "a b c");
    assert_eq!(normalize_space(" \t "), "");
  }

  #[test]
  fn a_sublanguage_answers_to_its_language() {
    assert!(sublanguage_of("en", "EN"));
    assert!(sublanguage_of("en-GB", "en"));
    assert!(!sublanguage_of("england", "en"), "the match is on whole subtags");
    assert!(!sublanguage_of("en", "en-GB"));
  }
}
