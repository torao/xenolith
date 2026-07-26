//! The functions XSLT adds to XPath (XSLT 1.0 §12).
//!
//! These are written without a prefix — `current()`, `generate-id()` — because XSLT defines them
//! rather than the caller, so they are registered in the empty namespace, which is where
//! [`Functions`] expects a host language to put its own. XPath's core library is consulted first,
//! so none of these can shadow one of XPath's twenty-seven.
//!
//! A function is registered once but called throughout a transformation, and what `current()`
//! answers changes as the transformation moves. So the two of them that depend on where the
//! transformation has got to read it from [`Running`], which the engine updates and the closures
//! share.
//!
//! # Specifications
//!
//! - [`current()` (§12.4)], [`generate-id()` (§12.4)], [`system-property()` (§12.4)]
//! - [`key()` and `xsl:key` (§12.2)] — the tables it reads are filled by the engine before the
//!   walk begins
//! - [`element-available()` and `function-available()` (§15)] — the two that let a stylesheet ask
//!   what the processor can do before it relies on it
//!
//! [`current()` (§12.4)]: https://www.w3.org/TR/1999/REC-xslt-19991116#function-current
//! [`key()` and `xsl:key` (§12.2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#key
//! [`generate-id()` (§12.4)]: https://www.w3.org/TR/1999/REC-xslt-19991116#function-generate-id
//! [`system-property()` (§12.4)]: https://www.w3.org/TR/1999/REC-xslt-19991116#function-system-property
//! [`element-available()` and `function-available()` (§15)]: https://www.w3.org/TR/1999/REC-xslt-19991116#function-element-available

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_xdm::Model;
use xylograph_xpath::{Context, Functions, Value, is_core_function};

use crate::pattern::KeyTable;
use crate::stylesheet::XSLT_NAMESPACE;

/// What the transformation is doing, as far as the XSLT functions need to know.
///
/// The engine holds one of these and the registered closures hold clones, so a function called
/// from inside an expression sees where the transformation has got to. Everything in it is a
/// node handle or a string — nothing borrowed from the tree — which is what lets the closures
/// outlive any one step of the run.
#[derive(Debug)]
pub(crate) struct Running<N: Copy + Eq + std::hash::Hash> {
  /// The node the innermost instruction is working on: what `current()` reports.
  ///
  /// This is not the context node. Inside a predicate the context node moves along the node-set
  /// being tested, while the current node stays where the instruction left it — which is the
  /// whole reason §12.4 gives `current()` a name of its own.
  current: Cell<Option<N>>,
  /// The identifiers `generate-id()` has handed out, so that a node keeps the one it was given.
  identifiers: RefCell<HashMap<N, String>>,
  /// What `key()` looks in: the nodes found by a key name and a value.
  ///
  /// Built before the transformation starts rather than when a key is first asked for. Building
  /// one means walking the whole tree testing a pattern, which needs the stylesheet and the
  /// model — neither of which a registered function can hold, since both are borrowed. Nodes
  /// are what comes out, and a node handle owns nothing, so the table can be kept here.
  keys: RefCell<HashMap<KeyEntry, Vec<N>>>,
}

/// A key name and the value being looked up, which is what indexes a key table.
type KeyEntry = (Option<String>, String, String);

impl<N: Copy + Eq + std::hash::Hash> Running<N> {
  /// A transformation that has not started.
  pub(crate) fn new() -> Self {
    Self { current: Cell::new(None), identifiers: RefCell::new(HashMap::new()), keys: RefCell::new(HashMap::new()) }
  }

  /// Records that `node` is found by `value` under a key name.
  ///
  /// §12.2 has every `xsl:key` of a name contribute, and a node may be reached by more than one
  /// value, so entries add up rather than replace.
  pub(crate) fn add_key_entry(&self, namespace: Option<&str>, local: &str, value: &str, node: N) {
    let entry = (namespace.map(ToOwned::to_owned), local.to_owned(), value.to_owned());
    self.keys.borrow_mut().entry(entry).or_default().push(node);
  }

  /// Records the node the instruction about to run is working on.
  pub(crate) fn set_current(&self, node: N) {
    self.current.set(Some(node));
  }

  /// The identifier for a node, minting one the first time it is asked for.
  ///
  /// §12.4 asks only that an identifier be the same every time for one node and different for
  /// different ones, within a transformation; it says nothing about what it looks like beyond
  /// being alphanumeric and starting with a letter. Handing them out in the order they are asked
  /// for satisfies that without pretending to be derived from anything.
  fn identifier(&self, node: N) -> String {
    let mut identifiers = self.identifiers.borrow_mut();
    let next = identifiers.len() + 1;
    identifiers.entry(node).or_insert_with(|| format!("id{next}")).clone()
  }
}

/// The key tables are also what a pattern beginning `key(…)` is matched against.
impl<N: Copy + Eq + std::hash::Hash> KeyTable<N> for Running<N> {
  fn lookup(&self, namespace: Option<&str>, local: &str, value: &str) -> Vec<N> {
    let entry = (namespace.map(ToOwned::to_owned), local.to_owned(), value.to_owned());
    self.keys.borrow().get(&entry).cloned().unwrap_or_default()
  }
}

/// Adds XSLT's own functions to the ones the caller supplied.
///
/// `instructions` is what the engine will actually run, so that `element-available()` answers
/// for this implementation rather than for XSLT on paper.
pub(crate) fn register<M: Model>(
  functions: Functions<M>,
  running: &Rc<Running<M::Node>>,
  instructions: &'static [&'static str],
) -> Functions<M> {
  let for_current = Rc::clone(running);
  let for_identifier = Rc::clone(running);
  let for_key = Rc::clone(running);

  functions
    .with("", "key", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("key", &arguments, 2, Some(2))?;
      let (namespace, local) = expand(&arguments[0].string(context.model), context)?;
      // §12.2: a node-set second argument asks for the union over each of its nodes' string
      // values, which is what makes `key('k', $set)` a join rather than a single lookup.
      let wanted: Vec<String> = match &arguments[1] {
        Value::NodeSet(nodes) => nodes.iter().map(|node| context.model.string_value(*node)).collect(),
        other => vec![other.string(context.model)],
      };
      let table = for_key.keys.borrow();
      let mut found: Vec<M::Node> = Vec::new();
      for value in wanted {
        let entry = (namespace.clone(), local.clone(), value);
        if let Some(nodes) = table.get(&entry) {
          found.extend(nodes.iter().copied());
        }
      }
      // A node-set holds each node once and comes out in document order, however the values
      // that reached it were spelled.
      found.sort_by(|a, b| context.model.document_order(*a, *b));
      found.dedup();
      Ok(Value::NodeSet(found))
    })
    .with("", "current", move |arguments: Vec<Value<M::Node>>, _: &Context<'_, M>| {
      arity("current", &arguments, 0, Some(0))?;
      // There is always a current node once a transformation is running, and these functions
      // are only reachable from inside one.
      let node = for_current
        .current
        .get()
        .ok_or_else(|| Error::new(ErrorKind::Xslt, "current() was called outside a transformation".to_owned()))?;
      Ok(Value::NodeSet(vec![node]))
    })
    .with("", "generate-id", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("generate-id", &arguments, 0, Some(1))?;
      // With no argument the context node is meant; with an empty node-set, the empty string.
      let node = match arguments.first() {
        None => Some(context.node),
        Some(Value::NodeSet(nodes)) => first_in_document_order(nodes, context),
        Some(other) => {
          let message = format!("generate-id() takes a node-set, but was given {}", describe(other));
          return Err(Error::new(ErrorKind::Xslt, message));
        }
      };
      Ok(Value::String(node.map(|node| for_identifier.identifier(node)).unwrap_or_default()))
    })
    .with("", "system-property", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("system-property", &arguments, 1, Some(1))?;
      let name = arguments[0].string(context.model);
      Ok(system_property(&name, context))
    })
    .with("", "element-available", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("element-available", &arguments, 1, Some(1))?;
      let name = arguments[0].string(context.model);
      let (namespace, local) = expand(&name, context)?;
      // Only the XSLT namespace has instructions this implementation knows; an extension
      // element belongs to whoever supplied it, and none are supplied yet.
      let available = namespace.as_deref() == Some(XSLT_NAMESPACE) && instructions.contains(&local.as_str());
      Ok(Value::Boolean(available))
    })
    .with("", "function-available", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("function-available", &arguments, 1, Some(1))?;
      let name = arguments[0].string(context.model);
      let (namespace, local) = expand(&name, context)?;
      // An unprefixed name is XPath's own or one XSLT added; a prefixed one is an extension,
      // and the registry is what knows whether it was supplied. Asking the registry rather
      // than a list means this answer cannot drift from what a call would actually do.
      let namespace = namespace.unwrap_or_default();
      let registered = context.functions.is_some_and(|functions| functions.get(&namespace, &local).is_some());
      let available = registered || (namespace.is_empty() && is_core_function(&local));
      Ok(Value::Boolean(available))
    })
}

/// `system-property()`: what §12.4 requires, and nothing invented beyond it.
///
/// The three `xsl:` properties are the ones the specification names. Anything else — including a
/// property in some other namespace — is the empty string, which is what §12.4 says an unknown
/// property gives, rather than an error.
fn system_property<M: Model>(name: &str, context: &Context<'_, M>) -> Value<M::Node> {
  let Ok((namespace, local)) = expand(name, context) else {
    return Value::String(String::new());
  };
  if namespace.as_deref() != Some(XSLT_NAMESPACE) {
    return Value::String(String::new());
  }
  match local.as_str() {
    // A number, not a string: §12.4 says so, and `system-property('xsl:version') >= 1.0` is
    // how a stylesheet is meant to ask.
    "version" => Value::Number(1.0),
    "vendor" => Value::String("xylograph".to_owned()),
    "vendor-url" => Value::String("https://github.com/torao/xylograph".to_owned()),
    _ => Value::String(String::new()),
  }
}

/// Resolves a QName written in an argument against the namespaces in scope.
fn expand<M: Model>(name: &str, context: &Context<'_, M>) -> Result<(Option<String>, String)> {
  match name.split_once(':') {
    None => Ok((None, name.to_owned())),
    Some((prefix, local)) => match context.namespaces.get(prefix) {
      Some(namespace) => Ok((Some(namespace.to_owned()), local.to_owned())),
      None => {
        let message = format!("the prefix \"{prefix}\" of \"{name}\" is not bound");
        Err(Error::new(ErrorKind::Xslt, message))
      }
    },
  }
}

/// The first node of a node-set in document order, which is the one a function is given.
fn first_in_document_order<M: Model>(nodes: &[M::Node], context: &Context<'_, M>) -> Option<M::Node> {
  nodes.iter().copied().min_by(|a, b| context.model.document_order(*a, *b))
}

/// Checks how many arguments a function was given.
fn arity<N>(name: &str, arguments: &[Value<N>], least: usize, most: Option<usize>) -> Result<()> {
  let found = arguments.len();
  if found < least || most.is_some_and(|most| found > most) {
    let expected = match most {
      Some(most) if most == least => format!("{least} argument(s)"),
      Some(most) => format!("between {least} and {most} arguments"),
      None => format!("at least {least} argument(s)"),
    };
    let message = format!("the function \"{name}()\" needs {expected}, but was given {found}");
    return Err(Error::new(ErrorKind::Xslt, message));
  }
  Ok(())
}

/// Names a value's type for a message.
fn describe<N>(value: &Value<N>) -> &'static str {
  match value {
    Value::NodeSet(_) => "a node-set",
    Value::Boolean(_) => "a boolean",
    Value::Number(_) => "a number",
    Value::String(_) => "a string",
  }
}
