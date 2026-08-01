//! `http://exslt.org/math` — the arithmetic XPath 1.0 leaves out.
//!
//! XPath 1.0 has four operators and `sum()`, and nothing else: no minimum, no square root, no
//! logarithm. This module is what EXSLT adds, and most of it is a thin layer over `f64` — which
//! is the right layer, since XPath's number *is* an IEEE double and its rules for NaN and
//! infinity are the ones `f64` already follows.
//!
//! # Where a node-set is empty
//!
//! `math:min` and `math:max` over an empty node-set give NaN, because there is no smallest of
//! nothing and NaN is what XPath 1.0 §4.4 uses for a number that is not one. `math:highest` and
//! `math:lowest` give the empty node-set, since they answer with nodes rather than numbers.
//!
//! # Examples
//!
//! ```
//! use xylogue_dom::build;
//! use xylogue_xdm::DomModel;
//! use xylogue_xpath::Functions;
//! use xylogue_xslt::{Stylesheet, Transform};
//!
//! let stylesheet = Stylesheet::compile(
//!   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
//!                       xmlns:math="http://exslt.org/math">
//!         <xsl:template match="/"><xsl:value-of select="math:max(//n)"/></xsl:template>
//!       </xsl:stylesheet>"#,
//!   "file:///s.xsl",
//! )?;
//!
//! let doc = build::parse("<r><n>3</n><n>11</n><n>7</n></r>".as_bytes())?;
//! let model = DomModel::new(&doc);
//! let functions = xylogue_exslt::register(Functions::new());
//!
//! let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions)?;
//! assert_eq!(result.text(), "11");
//! # Ok::<(), xylogue_core::Error>(())
//! ```
//!
//! # Specifications
//!
//! - [`exslt:math`](http://exslt.org/math/index.html)

use xylogue_core::error::Result;
use xylogue_xdm::Model;
use xylogue_xpath::{Context, Functions, Value};

use crate::support::{arity, in_document_order, nodes};

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/math";

/// Adds this module's functions.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>) -> Functions<M> {
  functions
    .with(NAMESPACE, "min", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:min", &arguments, 1, Some(1))?;
      Ok(Value::Number(extreme::<M>(&arguments[0], context, Extreme::Least)?.0))
    })
    .with(NAMESPACE, "max", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:max", &arguments, 1, Some(1))?;
      Ok(Value::Number(extreme::<M>(&arguments[0], context, Extreme::Greatest)?.0))
    })
    .with(NAMESPACE, "lowest", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:lowest", &arguments, 1, Some(1))?;
      Ok(Value::NodeSet(extreme::<M>(&arguments[0], context, Extreme::Least)?.1))
    })
    .with(NAMESPACE, "highest", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:highest", &arguments, 1, Some(1))?;
      Ok(Value::NodeSet(extreme::<M>(&arguments[0], context, Extreme::Greatest)?.1))
    })
    // Each of these is written out rather than made by a helper: a helper returning a closure
    // would be an opaque type mentioning `M`, which the registry could only hold for a model
    // that outlives everything — and a model borrowing a document does not.
    .with(NAMESPACE, "abs", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:abs", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).abs()))
    })
    .with(NAMESPACE, "sqrt", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:sqrt", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).sqrt()))
    })
    .with(NAMESPACE, "exp", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:exp", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).exp()))
    })
    .with(NAMESPACE, "log", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:log", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).ln()))
    })
    .with(NAMESPACE, "sin", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:sin", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).sin()))
    })
    .with(NAMESPACE, "cos", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:cos", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).cos()))
    })
    .with(NAMESPACE, "tan", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:tan", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).tan()))
    })
    .with(NAMESPACE, "asin", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:asin", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).asin()))
    })
    .with(NAMESPACE, "acos", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:acos", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).acos()))
    })
    .with(NAMESPACE, "atan", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:atan", &arguments, 1, Some(1))?;
      Ok(Value::Number(arguments[0].number(context.model).atan()))
    })
    .with(NAMESPACE, "power", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:power", &arguments, 2, Some(2))?;
      let base = arguments[0].number(context.model);
      let exponent = arguments[1].number(context.model);
      Ok(Value::Number(base.powf(exponent)))
    })
    .with(NAMESPACE, "atan2", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:atan2", &arguments, 2, Some(2))?;
      let y = arguments[0].number(context.model);
      let x = arguments[1].number(context.model);
      Ok(Value::Number(y.atan2(x)))
    })
    .with(NAMESPACE, "constant", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("math:constant", &arguments, 2, Some(2))?;
      let name = arguments[0].string(context.model);
      let precision = arguments[1].number(context.model);
      Ok(Value::Number(constant(&name, precision)))
    })
}

/// Whether the least or the greatest is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extreme {
  Least,
  Greatest,
}

/// The extreme value of a node-set, and the nodes that carry it.
///
/// Both are worked out together because both functions need the same walk, and because a node
/// whose value is NaN must not be picked as an extreme by accident: a comparison with NaN is
/// false either way, so it is skipped rather than compared.
fn extreme<M: Model>(value: &Value<M::Node>, context: &Context<'_, M>, which: Extreme) -> Result<(f64, Vec<M::Node>)> {
  let name = if which == Extreme::Least { "math:min" } else { "math:max" };
  let nodes = nodes::<M>(name, value)?;
  let mut best = f64::NAN;
  let mut carriers: Vec<M::Node> = Vec::new();
  for node in nodes {
    let number = xylogue_xpath::string_to_number(&context.model.string_value(node));
    // One node that is not a number makes the whole answer NaN, which is what arithmetic over
    // it would have given.
    if number.is_nan() {
      return Ok((f64::NAN, Vec::new()));
    }
    let better = match which {
      Extreme::Least => number < best,
      Extreme::Greatest => number > best,
    };
    if best.is_nan() || better {
      best = number;
      carriers.clear();
      carriers.push(node);
    } else if number == best {
      carriers.push(node);
    }
  }
  Ok((best, in_document_order(carriers, context)))
}

/// One of the constants `math:constant` names, rounded to `precision` decimal places.
///
/// EXSLT names E, PI, SQRRT2, LN2, LN10, LOG2E and SQRT1_2 — the spelling of the square root of
/// two included, which is theirs and not a slip here. A name it does not know gives NaN, since
/// there is no number to give.
fn constant(name: &str, precision: f64) -> f64 {
  use std::f64::consts;
  let value = match name {
    "E" => consts::E,
    "LN2" => consts::LN_2,
    "LN10" => consts::LN_10,
    "LOG2E" => consts::LOG2_E,
    "SQRT2" | "SQRRT2" => consts::SQRT_2,
    "SQRT1_2" => consts::FRAC_1_SQRT_2,
    "PI" => consts::PI,
    _ => return f64::NAN,
  };
  if !precision.is_finite() || precision < 0.0 {
    return f64::NAN;
  }
  // The precision is a number of decimal places, so the value is cut rather than rounded: EXSLT
  // describes it as the constant "to the given precision", and 3.14159 to two places is 3.14.
  let scale = 10f64.powf(precision.trunc());
  (value * scale).trunc() / scale
}

#[cfg(test)]
mod tests {
  use super::*;

  // The literals below are approximations of PI and E on purpose — they are what cutting to a
  // few places gives, which is the whole of what this checks.
  #[allow(clippy::approx_constant)]
  #[test]
  fn a_constant_is_cut_to_the_places_asked_for() {
    assert!((constant("PI", 0.0) - 3.0).abs() < f64::EPSILON);
    assert!((constant("PI", 2.0) - 3.14).abs() < 1e-12);
    assert!((constant("PI", 5.0) - 3.14159).abs() < 1e-12);
    assert!((constant("E", 3.0) - 2.718).abs() < 1e-12);
  }

  #[test]
  fn the_square_root_of_two_answers_to_the_spelling_exslt_uses() {
    // EXSLT writes it `SQRRT2`; the ordinary spelling is accepted as well rather than being a
    // trap for anyone who did not notice.
    assert_eq!(constant("SQRRT2", 3.0), constant("SQRT2", 3.0));
    assert!((constant("SQRT2", 3.0) - 1.414).abs() < 1e-12);
  }

  #[test]
  fn a_constant_nobody_named_is_not_a_number() {
    assert!(constant("TAU", 3.0).is_nan());
    assert!(constant("PI", -1.0).is_nan(), "a precision below zero names no rounding");
  }
}
