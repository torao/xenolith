//! Functions the caller supplies, beyond the core library.
//!
//! XPath reserves unprefixed names for its own functions, so an extension is always written with
//! a prefix — `exsl:node-set(…)`, `my:format(…)`. What that prefix stands for is resolved like
//! any other, and the function is then looked up by its expanded name.
//!
//! This is Java's `XPathFunctionResolver` and `XPathFunction`: [`Functions`] is the set that is
//! consulted, [`Function`] is one of them. A plain closure is a `Function`, so registering one
//! usually needs no type of its own.

use std::collections::HashMap;
use std::fmt;

use xylograph_core::error::Result;
use xylograph_xdm::Model;

use crate::context::Context;
use crate::value::Value;

/// One extension function: Java's `XPathFunction`.
///
/// The arguments arrive evaluated, in the order they were written. The context is there for a
/// function that needs more than them — the tree, the context node, or where it sits.
pub trait Function<M: Model> {
  /// Calls the function.
  ///
  /// # Errors
  ///
  /// Whatever the function decides is wrong: the wrong number of arguments, a value of the
  /// wrong type, or a failure of its own.
  fn call(&self, arguments: Vec<Value<M::Node>>, context: &Context<'_, M>) -> Result<Value<M::Node>>;
}

impl<M, F> Function<M> for F
where
  M: Model,
  F: for<'a> Fn(Vec<Value<M::Node>>, &Context<'a, M>) -> Result<Value<M::Node>>,
{
  fn call(&self, arguments: Vec<Value<M::Node>>, context: &Context<'_, M>) -> Result<Value<M::Node>> {
    self(arguments, context)
  }
}

/// The extension functions an expression may call: Java's `XPathFunctionResolver`.
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::DomModel;
/// use xylograph_xpath::{Context, Functions, Namespaces, Value, Variables, evaluate_in, parse};
///
/// let doc = build::parse("<a/>".as_bytes())?;
/// let model = DomModel::new(&doc);
///
/// // A closure is a Function, so this is all it takes to add one.
/// let functions = Functions::new().with("urn:my", "twice", |arguments: Vec<Value<_>>, context: &Context<'_, _>| {
///   Ok(Value::Number(arguments[0].number(context.model) * 2.0))
/// });
///
/// let namespaces = Namespaces::new().with("my", "urn:my");
/// let variables = Variables::new();
/// let context = Context::new(&model, model.root_node(), &namespaces, &variables).with_functions(&functions);
///
/// let expression = parse("my:twice(21)")?;
/// assert_eq!(evaluate_in(&expression, &context)?.number(&model), 42.0);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
pub struct Functions<M: Model> {
  /// By expanded name: the namespace the prefix resolved to, and the local part.
  registered: HashMap<(String, String), Box<dyn Function<M>>>,
}

impl<M: Model> Default for Functions<M> {
  fn default() -> Self {
    Self::new()
  }
}

impl<M: Model> Functions<M> {
  /// No extension functions at all.
  #[must_use]
  pub fn new() -> Self {
    Self { registered: HashMap::new() }
  }

  /// Registers a function under an expanded name.
  ///
  /// The namespace is the one the expression's prefix resolves to, not the prefix itself: an
  /// expression may call the same function by whatever prefix it has bound.
  #[must_use]
  pub fn with(mut self, namespace: &str, local: &str, function: impl Function<M> + 'static) -> Self {
    self.registered.insert((namespace.to_owned(), local.to_owned()), Box::new(function));
    self
  }

  /// The function registered under an expanded name, if there is one.
  #[must_use]
  pub fn get(&self, namespace: &str, local: &str) -> Option<&dyn Function<M>> {
    // The key owns its strings; an expression calls few functions, so this is cheap enough.
    self.registered.get(&(namespace.to_owned(), local.to_owned())).map(AsRef::as_ref)
  }

  /// The expanded names registered, for a message that lists what is available.
  pub(crate) fn names(&self) -> impl Iterator<Item = (&str, &str)> {
    self.registered.keys().map(|(namespace, local)| (namespace.as_str(), local.as_str()))
  }
}

// Written by hand: a boxed function cannot be printed, but its name can.
impl<M: Model> fmt::Debug for Functions<M> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut names: Vec<String> =
      self.registered.keys().map(|(namespace, local)| format!("{{{namespace}}}{local}")).collect();
    names.sort();
    f.debug_struct("Functions").field("registered", &names).finish()
  }
}
