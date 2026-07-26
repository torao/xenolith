//! Running a stylesheet over a source tree.
//!
//! A transformation walks the source through the template rules the stylesheet declared and
//! builds a **result tree** as it goes. The engine here holds the two halves of that: where in
//! the result the next node is added, and what the expressions in the stylesheet are evaluated
//! against — the context node, its position, and the variables in scope.
//!
//! # What this phase runs
//!
//! Literal result elements and text, and the instructions `xsl:apply-templates`,
//! `xsl:call-template`, `xsl:for-each`, `xsl:if`, `xsl:choose`, `xsl:value-of`, `xsl:variable`,
//! `xsl:param`, `xsl:with-param` and `xsl:text`, together with the built-in template rules and
//! attribute value templates. The rest of XSLT — `xsl:copy`, `xsl:element`, `xsl:sort`,
//! `xsl:key` and the others — arrives in Phase 6; an instruction that is not understood is
//! reported rather than skipped, so a stylesheet never half-runs in silence.

use std::collections::HashMap;
use std::rc::Rc;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::{Document, NodeId, NodeType};
use xylograph_xdm::{Model, NodeKind};
use xylograph_xpath::{Context, Expr, Namespaces, Value, Variables};

use crate::avt::{self, Piece};
use crate::stylesheet::{Stylesheet, Template, XSLT_NAMESPACE, in_scope_namespaces};

/// How deep `xsl:apply-templates` and `xsl:call-template` may go before the transformation is
/// refused.
///
/// A stylesheet that recurses without end would otherwise exhaust the stack, and a guard that
/// the stack reaches first is no guard at all — so this is set well below what a debug build
/// with the usual two-megabyte thread stack can carry, not at what a release build could.
/// [`Transform::with_max_depth`] raises it for a stylesheet that recurses deeply on purpose.
pub const DEFAULT_MAX_DEPTH: usize = 200;

/// The tree a transformation produced.
///
/// The content hangs from a document fragment rather than from the document itself, because a
/// result tree need not be one element — a stylesheet may produce several, or only text, which
/// a document may not hold directly.
#[derive(Debug)]
pub struct ResultTree {
  document: Document,
  root: NodeId,
}

impl ResultTree {
  /// The document the result lives in.
  #[must_use]
  pub const fn document(&self) -> &Document {
    &self.document
  }

  /// The fragment the result hangs from; serialize this to write the result out.
  #[must_use]
  pub const fn root(&self) -> NodeId {
    self.root
  }

  /// The text of the result: every character it contains, with the markup left out.
  #[must_use]
  pub fn text(&self) -> String {
    self.document.text_content(self.root)
  }
}

/// Runs `stylesheet` over the tree `model` presents, starting at `node`.
///
/// # Errors
///
/// [`ErrorKind::Xslt`] for an instruction the engine does not understand, a template that cannot
/// be found, or a transformation that recurses too deep; [`ErrorKind::XPath`] for an expression
/// that cannot be read or evaluated.
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::DomModel;
/// use xylograph_xslt::{Stylesheet, transform};
///
/// let stylesheet = Stylesheet::compile(
///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
///         <xsl:template match="/"><xsl:apply-templates select="//name"/></xsl:template>
///         <xsl:template match="name"><xsl:value-of select="."/>;</xsl:template>
///       </xsl:stylesheet>"#,
///   "file:///s.xsl",
/// )?;
///
/// let doc = build::parse("<people><name>Ada</name><name>Alan</name></people>".as_bytes())?;
/// let model = DomModel::new(&doc);
/// let result = transform(&stylesheet, &model, model.root_node())?;
/// assert_eq!(result.text(), "Ada;Alan;");
/// # Ok::<(), xylograph_core::Error>(())
/// ```
pub fn transform<M: Model>(stylesheet: &Stylesheet, model: &M, node: M::Node) -> Result<ResultTree> {
  Transform::new().run(stylesheet, model, node)
}

/// A transformation, with the limits it is run under.
///
/// [`transform`] is this with the defaults; build one of these to change them.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
  max_depth: usize,
}

impl Default for Transform {
  fn default() -> Self {
    Self::new()
  }
}

impl Transform {
  /// A transformation with the default limits.
  #[must_use]
  pub const fn new() -> Self {
    Self { max_depth: DEFAULT_MAX_DEPTH }
  }

  /// How deep template application may go before the transformation is refused.
  ///
  /// The default is deliberately conservative — see [`DEFAULT_MAX_DEPTH`]'s reasoning. Raise it
  /// for a stylesheet that recurses deeply on purpose, on a thread with the stack to match.
  #[must_use]
  pub const fn with_max_depth(mut self, depth: usize) -> Self {
    self.max_depth = depth;
    self
  }

  /// Runs `stylesheet` over the tree `model` presents, starting at `node`.
  ///
  /// # Errors
  ///
  /// As [`transform`].
  pub fn run<M: Model>(&self, stylesheet: &Stylesheet, model: &M, node: M::Node) -> Result<ResultTree> {
    let mut output = Document::new();
    let root = output.create_document_fragment();
    let mut engine = Engine {
      stylesheet,
      model,
      output,
      insertion: vec![root],
      scopes: Vec::new(),
      expressions: HashMap::new(),
      namespaces: HashMap::new(),
      depth: 0,
      max_depth: self.max_depth,
    };
    engine.bind_global_variables(node)?;
    engine.apply(Focus { node, position: 1, size: 1 }, None)?;
    Ok(ResultTree { document: engine.output, root })
  }
}

/// The node an instruction is being run against, and where it sits among its siblings.
#[derive(Clone, Copy, Debug)]
struct Focus<N> {
  node: N,
  position: usize,
  size: usize,
}

/// One variable binding.
struct Binding<N> {
  name: String,
  value: Value<N>,
}

struct Engine<'a, M: Model> {
  stylesheet: &'a Stylesheet,
  model: &'a M,
  output: Document,
  /// The result nodes being filled, innermost last; new content is appended to the last.
  insertion: Vec<NodeId>,
  /// Variable scopes, innermost last. The first is the stylesheet's global variables.
  scopes: Vec<Vec<Binding<M::Node>>>,
  /// Parsed expressions, so a path inside a loop is read once rather than once per node.
  expressions: HashMap<String, Expr>,
  /// The namespace bindings in scope at a stylesheet element, which its expressions are read
  /// against. Gathering them walks the ancestors, so the answers are kept.
  namespaces: HashMap<(usize, NodeId), Rc<Namespaces>>,
  depth: usize,
  max_depth: usize,
}

impl<M: Model> Engine<'_, M> {
  // --- Template rules -----------------------------------------------------------------------

  /// Applies the template rule that matches `focus`, or the built-in rule if none does.
  fn apply(&mut self, focus: Focus<M::Node>, mode: Option<&str>) -> Result<()> {
    if self.depth >= self.max_depth {
      let message = format!("the transformation is more than {} templates deep", self.max_depth);
      return Err(Error::new(ErrorKind::Xslt, message));
    }
    let matched = self.stylesheet.template_for(self.model, focus.node, mode)?;
    let Some(template) = matched else {
      return self.built_in(focus, mode);
    };
    let (module, element) = (template.module(), template.element());
    self.depth += 1;
    let outcome = self.run_template(module, element, focus, Vec::new());
    self.depth -= 1;
    outcome
  }

  /// The rule that applies when the stylesheet declares none (XSLT 1.0 §5.8).
  ///
  /// The root and an element pass the work on to their children; text and an attribute yield
  /// their own characters; a comment, a processing instruction and a namespace yield nothing.
  fn built_in(&mut self, focus: Focus<M::Node>, mode: Option<&str>) -> Result<()> {
    match self.model.kind(focus.node) {
      NodeKind::Root | NodeKind::Element => {
        let children = self.model.children(focus.node);
        self.apply_to_all(&children, mode)
      }
      NodeKind::Text | NodeKind::Attribute => {
        let text = self.model.string_value(focus.node);
        self.append_text(&text);
        Ok(())
      }
      NodeKind::Comment | NodeKind::ProcessingInstruction | NodeKind::Namespace => Ok(()),
    }
  }

  /// Applies rules to each of a list of nodes, each knowing its place in it.
  fn apply_to_all(&mut self, nodes: &[M::Node], mode: Option<&str>) -> Result<()> {
    for (index, node) in nodes.iter().enumerate() {
      self.apply(Focus { node: *node, position: index + 1, size: nodes.len() }, mode)?;
    }
    Ok(())
  }

  /// Runs a template's body, with `parameters` supplied for its `xsl:param` children.
  fn run_template(
    &mut self,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
    parameters: Vec<Binding<M::Node>>,
  ) -> Result<()> {
    // A template's own scope holds its parameters and whatever its body declares.
    self.scopes.push(parameters);
    let outcome = self.run_body(module, element, focus);
    self.scopes.pop();
    outcome
  }

  // --- Running a body -----------------------------------------------------------------------

  /// Runs the children of a stylesheet element.
  fn run_body(&mut self, module: usize, parent: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let children: Vec<NodeId> = self.stylesheet.document(module).children(parent).collect();
    for child in children {
      self.run_node(module, child, focus)?;
    }
    Ok(())
  }

  /// Runs one node of a template body.
  fn run_node(&mut self, module: usize, node: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let document = self.stylesheet.document(module);
    match document.node_type(node) {
      NodeType::Text | NodeType::CdataSection => {
        let text = document.node_value(node).unwrap_or_default().to_owned();
        // XSLT 1.0 §3.4 strips whitespace-only text from a stylesheet, so that the indentation
        // a stylesheet is written with does not turn up in the result. `xsl:text` keeps it,
        // and reaches the output by another path.
        if !text.chars().all(xylograph_core::chars::is_whitespace) {
          self.append_text(&text);
        }
        Ok(())
      }
      // A comment or processing instruction in the stylesheet is not part of the result;
      // xsl:comment and xsl:processing-instruction are how those are produced.
      NodeType::Comment | NodeType::ProcessingInstruction => Ok(()),
      NodeType::Element => {
        if document.namespace_uri(node) == Some(XSLT_NAMESPACE) {
          let local = document.local_name(node).unwrap_or_default().to_owned();
          return self.instruction(module, node, &local, focus);
        }
        self.literal_element(module, node, focus)
      }
      _ => Ok(()),
    }
  }

  /// Runs one XSLT instruction.
  fn instruction(&mut self, module: usize, element: NodeId, local: &str, focus: Focus<M::Node>) -> Result<()> {
    match local {
      "value-of" => {
        let select = self.required(module, element, "select", "xsl:value-of")?;
        let text = self.string_of(&select, module, element, focus)?;
        self.append_text(&text);
        Ok(())
      }
      "text" => {
        let text = self.stylesheet.document(module).text_content(element);
        self.append_text(&text);
        Ok(())
      }
      "if" => {
        let test = self.required(module, element, "test", "xsl:if")?;
        if self.evaluate(&test, module, element, focus)?.boolean() {
          return self.run_body(module, element, focus);
        }
        Ok(())
      }
      "choose" => self.choose(module, element, focus),
      "for-each" => {
        let select = self.required(module, element, "select", "xsl:for-each")?;
        let nodes = self.node_set(&select, module, element, focus, "xsl:for-each")?;
        for (index, node) in nodes.iter().enumerate() {
          let inner = Focus { node: *node, position: index + 1, size: nodes.len() };
          // Each turn of the loop is its own scope, so a variable declared inside does not
          // leak into the next.
          self.scopes.push(Vec::new());
          let outcome = self.run_body(module, element, inner);
          self.scopes.pop();
          outcome?;
        }
        Ok(())
      }
      "apply-templates" => self.apply_templates(module, element, focus),
      "call-template" => self.call_template(module, element, focus),
      "variable" | "param" => self.declare(module, element, local, focus),
      // xsl:with-param is read by the instruction it belongs to, not run on its own.
      "with-param" => Ok(()),
      other => {
        let message = format!("xsl:{other} is not implemented yet; see ROADMAP.md for which phase brings it");
        Err(Error::new(ErrorKind::Xslt, message))
      }
    }
  }

  fn choose(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let branches: Vec<NodeId> = self.stylesheet.document(module).children(element).collect();
    let mut otherwise = None;
    for branch in branches {
      let document = self.stylesheet.document(module);
      if document.node_type(branch) != NodeType::Element || document.namespace_uri(branch) != Some(XSLT_NAMESPACE) {
        continue;
      }
      match document.local_name(branch) {
        Some("when") => {
          let test = self.required(module, branch, "test", "xsl:when")?;
          if self.evaluate(&test, module, branch, focus)?.boolean() {
            return self.run_body(module, branch, focus);
          }
        }
        Some("otherwise") => otherwise = Some(branch),
        _ => {}
      }
    }
    match otherwise {
      Some(branch) => self.run_body(module, branch, focus),
      None => Ok(()),
    }
  }

  fn apply_templates(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let document = self.stylesheet.document(module);
    let select = document.attribute(element, "select").map(ToOwned::to_owned);
    let mode = document.attribute(element, "mode").map(ToOwned::to_owned);
    let parameters = self.with_params(module, element, focus)?;

    // Without a select, the children are what the rules are applied to.
    let nodes = match select {
      Some(select) => self.node_set(&select, module, element, focus, "xsl:apply-templates")?,
      None => self.model.children(focus.node),
    };
    if parameters.is_empty() {
      return self.apply_to_all(&nodes, mode.as_deref());
    }
    // With parameters the rule has to be found here, so that they can be handed to it.
    for (index, node) in nodes.iter().enumerate() {
      let inner = Focus { node: *node, position: index + 1, size: nodes.len() };
      let matched: Option<(usize, NodeId)> = self
        .stylesheet
        .template_for(self.model, inner.node, mode.as_deref())?
        .map(|template: &Template| (template.module(), template.element()));
      match matched {
        Some((template_module, template_element)) => {
          let copies = parameters.iter().map(|p| Binding { name: p.name.clone(), value: p.value.clone() }).collect();
          self.depth += 1;
          let outcome = self.run_template(template_module, template_element, inner, copies);
          self.depth -= 1;
          outcome?;
        }
        // A built-in rule takes no parameters.
        None => self.built_in(inner, mode.as_deref())?,
      }
    }
    Ok(())
  }

  fn call_template(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let name = self.required(module, element, "name", "xsl:call-template")?;
    let parameters = self.with_params(module, element, focus)?;
    let Some(template) = self.stylesheet.template_named(&name) else {
      return Err(Error::new(ErrorKind::Xslt, format!("no template is named {name:?}")));
    };
    let (target_module, target_element) = (template.module(), template.element());
    if self.depth >= self.max_depth {
      let message = format!("the transformation is more than {} templates deep", self.max_depth);
      return Err(Error::new(ErrorKind::Xslt, message));
    }
    self.depth += 1;
    let outcome = self.run_template(target_module, target_element, focus, parameters);
    self.depth -= 1;
    outcome
  }

  /// Evaluates the `xsl:with-param` children of an instruction, in the current context.
  fn with_params(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<Vec<Binding<M::Node>>> {
    let children: Vec<NodeId> = self.stylesheet.document(module).children(element).collect();
    let mut parameters = Vec::new();
    for child in children {
      let document = self.stylesheet.document(module);
      let is_param = document.node_type(child) == NodeType::Element
        && document.namespace_uri(child) == Some(XSLT_NAMESPACE)
        && document.local_name(child) == Some("with-param");
      if !is_param {
        continue;
      }
      let name = self.required(module, child, "name", "xsl:with-param")?;
      let value = self.declared_value(module, child, focus)?;
      parameters.push(Binding { name, value });
    }
    Ok(parameters)
  }

  /// Binds an `xsl:variable`, or an `xsl:param` that was not supplied a value.
  fn declare(&mut self, module: usize, element: NodeId, local: &str, focus: Focus<M::Node>) -> Result<()> {
    let what = if local == "param" { "xsl:param" } else { "xsl:variable" };
    let name = self.required(module, element, "name", what)?;
    // A parameter the caller supplied is already bound; its default is not evaluated.
    if local == "param" && self.scopes.last().is_some_and(|scope| scope.iter().any(|b| b.name == name)) {
      return Ok(());
    }
    let value = self.declared_value(module, element, focus)?;
    if let Some(scope) = self.scopes.last_mut() {
      scope.push(Binding { name, value });
    }
    Ok(())
  }

  /// The value a declaration carries: its `select`, or the text its content produces.
  ///
  /// XSLT calls the second a result tree fragment. Until `exsl:node-set` and `xsl:copy-of` give
  /// one somewhere to go, its string is all that can be observed of it — which is what XSLT 1.0
  /// allows a fragment to be used as — so that is what is kept.
  fn declared_value(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<Value<M::Node>> {
    if let Some(select) = self.stylesheet.document(module).attribute(element, "select").map(ToOwned::to_owned) {
      return self.evaluate(&select, module, element, focus);
    }
    // Run the content into a fragment of its own, then take its text.
    let fragment = self.output.create_document_fragment();
    self.insertion.push(fragment);
    let outcome = self.run_body(module, element, focus);
    self.insertion.pop();
    outcome?;
    Ok(Value::String(self.output.text_content(fragment)))
  }

  // --- The result tree ----------------------------------------------------------------------

  /// Copies a literal result element into the result, with its attribute value templates
  /// expanded.
  ///
  /// Namespace declarations are not copied: the element keeps its name and namespace, and the
  /// serializer writes whatever declarations the result needs. That also keeps the stylesheet's
  /// own `xmlns:xsl` out of the output without having to exclude it by hand.
  fn literal_element(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let document = self.stylesheet.document(module);
    let name = document.node_name(element);
    let namespace = document.namespace_uri(element).map(ToOwned::to_owned);
    let attributes: Vec<(String, Option<String>, String)> = document
      .attributes(element)
      .iter()
      .filter(|attribute| document.namespace_uri(*attribute) != Some(xylograph_core::XMLNS_NS_URI))
      .map(|attribute| {
        (
          document.node_name(attribute),
          document.namespace_uri(attribute).map(ToOwned::to_owned),
          document.node_value(attribute).unwrap_or_default().to_owned(),
        )
      })
      .collect();

    let created = self.output.create_element_ns(namespace.as_deref(), &name).map_err(dom_error)?;
    for (attribute_name, attribute_namespace, value) in attributes {
      let expanded = self.attribute_value(&value, module, element, focus)?;
      let outcome = match attribute_namespace {
        Some(namespace) => self.output.set_attribute_ns(created, Some(&namespace), &attribute_name, &expanded),
        None => self.output.set_attribute(created, &attribute_name, &expanded),
      };
      outcome.map_err(dom_error)?;
    }
    self.append(created)?;
    self.insertion.push(created);
    let outcome = self.run_body(module, element, focus);
    self.insertion.pop();
    outcome
  }

  /// Appends a node to whatever the result is currently being built into.
  fn append(&mut self, node: NodeId) -> Result<()> {
    let parent = *self.insertion.last().expect("there is always somewhere to put the result");
    self.output.append_child(parent, node).map_err(dom_error)?;
    Ok(())
  }

  fn append_text(&mut self, text: &str) {
    if text.is_empty() {
      return;
    }
    let node = self.output.create_text_node(text);
    let parent = *self.insertion.last().expect("there is always somewhere to put the result");
    let _ = self.output.append_child(parent, node);
  }

  // --- Expressions --------------------------------------------------------------------------

  /// Binds the stylesheet's top-level variables, highest import precedence winning.
  ///
  /// They are evaluated in the order they were declared, so one that refers to another declared
  /// after it is not resolved. XSLT allows that order; sorting the declarations by what they
  /// depend on arrives with the rest of the top-level elements.
  fn bind_global_variables(&mut self, node: M::Node) -> Result<()> {
    self.scopes.push(Vec::new());
    let declarations: Vec<(usize, NodeId, String, i32)> = self
      .stylesheet
      .variables()
      .iter()
      .map(|variable| (variable.module(), variable.element(), variable.name().to_owned(), variable.precedence()))
      .collect();

    for (module, element, name, precedence) in declarations {
      // A declaration of the same name in a module of higher precedence has already won.
      let shadowed =
        self.stylesheet.variables().iter().any(|other| other.name() == name && other.precedence() > precedence);
      if shadowed {
        continue;
      }
      let focus = Focus { node, position: 1, size: 1 };
      let value = self.declared_value(module, element, focus)?;
      let scope = self.scopes.last_mut().expect("the global scope was just pushed");
      scope.retain(|binding| binding.name != name);
      scope.push(Binding { name, value });
    }
    Ok(())
  }

  /// Evaluates an expression written on a stylesheet element.
  fn evaluate(
    &mut self,
    expression: &str,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
  ) -> Result<Value<M::Node>> {
    let namespaces = self.namespaces_at(module, element);
    let variables = self.variables();
    let parsed = self.expression(expression)?;
    // The focus carries where the node sits in the list being processed, which is what
    // `position()` and `last()` report.
    let context =
      Context::new(self.model, focus.node, &namespaces, &variables).at(focus.node, focus.position, focus.size);
    xylograph_xpath::evaluate_in(&parsed, &context)
  }

  /// Evaluates an expression and takes its string-value.
  fn string_of(&mut self, expression: &str, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<String> {
    Ok(self.evaluate(expression, module, element, focus)?.string(self.model))
  }

  /// Evaluates an expression that has to yield a node-set, in document order.
  fn node_set(
    &mut self,
    expression: &str,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
    what: &str,
  ) -> Result<Vec<M::Node>> {
    match self.evaluate(expression, module, element, focus)? {
      Value::NodeSet(nodes) => Ok(nodes),
      other => {
        let message = format!("{what} selects a node-set, but {expression:?} gave {}", describe(&other));
        Err(Error::new(ErrorKind::Xslt, message))
      }
    }
  }

  /// Expands an attribute value template.
  fn attribute_value(&mut self, value: &str, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<String> {
    let pieces = avt::parse(value)?;
    // The common case is one literal, and then there is nothing to expand.
    if let [Piece::Literal(text)] = pieces.as_slice() {
      return Ok(text.clone());
    }
    let mut expanded = String::new();
    for piece in pieces {
      match piece {
        Piece::Literal(text) => expanded.push_str(&text),
        Piece::Expression(expression) => {
          expanded.push_str(&self.string_of(&expression, module, element, focus)?);
        }
      }
    }
    Ok(expanded)
  }

  /// The parsed form of an expression, read once and kept.
  fn expression(&mut self, expression: &str) -> Result<Expr> {
    if let Some(parsed) = self.expressions.get(expression) {
      return Ok(parsed.clone());
    }
    let parsed = xylograph_xpath::parse(expression)?;
    self.expressions.insert(expression.to_owned(), parsed.clone());
    Ok(parsed)
  }

  /// The namespace bindings in scope at a stylesheet element.
  fn namespaces_at(&mut self, module: usize, element: NodeId) -> Rc<Namespaces> {
    if let Some(namespaces) = self.namespaces.get(&(module, element)) {
      return Rc::clone(namespaces);
    }
    let namespaces = Rc::new(in_scope_namespaces(self.stylesheet.document(module), element));
    self.namespaces.insert((module, element), Rc::clone(&namespaces));
    namespaces
  }

  /// The variables in scope, innermost binding of a name winning.
  fn variables(&self) -> Variables<M::Node> {
    let mut variables = Variables::new();
    for scope in &self.scopes {
      for binding in scope {
        variables = variables.with(&binding.name, binding.value.clone());
      }
    }
    variables
  }

  /// Reads an attribute an instruction cannot do without.
  fn required(&self, module: usize, element: NodeId, attribute: &str, what: &str) -> Result<String> {
    self
      .stylesheet
      .document(module)
      .attribute(element, attribute)
      .map(ToOwned::to_owned)
      .ok_or_else(|| Error::new(ErrorKind::Xslt, format!("{what} needs a {attribute}")))
  }
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

/// The result tree is built by this engine, so a DOM refusal is a bug here rather than
/// something a stylesheet did.
fn dom_error(error: xylograph_dom::DomException) -> Error {
  Error::internal(format!("building the result tree: {error}"))
}
