//! Running a stylesheet over a source tree.
//!
//! A transformation walks the source through the template rules the stylesheet declared and
//! builds a **result tree** as it goes. The engine here holds the two halves of that: where in
//! the result the next node is added, and what the expressions in the stylesheet are evaluated
//! against — the context node, its position, and the variables in scope.
//!
//! # What this phase runs
//!
//! Literal result elements and text; the instructions that direct the walk —
//! `xsl:apply-templates`, `xsl:call-template`, `xsl:for-each`, `xsl:if`, `xsl:choose`,
//! `xsl:value-of`, `xsl:variable`, `xsl:param`, `xsl:with-param` and `xsl:text`; and the
//! instructions that build result nodes — `xsl:element`, `xsl:attribute`, `xsl:comment`,
//! `xsl:processing-instruction`, `xsl:copy`, `xsl:copy-of` and `xsl:message`, with
//! `xsl:attribute-set` and `use-attribute-sets` behind them. The built-in template rules and
//! attribute value templates run throughout.
//!
//! On top of XPath's own functions, the expressions in a stylesheet can call the ones XSLT adds
//! in [`functions`](crate::functions) — `current()`, `key()`, `generate-id()`,
//! `system-property()`, `element-available()` and `function-available()`.
//!
//! What is still missing is `xsl:sort`, `xsl:number`, `xsl:decimal-format`,
//! `xsl:document`-style multi-document work and the output controls; see `ROADMAP.md`. An
//! instruction that is not understood is reported rather than skipped, so a stylesheet never
//! half-runs in silence — and `element-available()` says so beforehand, from the same list.

use std::collections::HashMap;
use std::rc::Rc;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::{Document, NodeId, NodeType};
use xylograph_xdm::{Model, NodeKind};
use xylograph_xpath::{Context, Expr, Functions, Namespaces, Value, Variables};

use crate::avt::{self, Piece};
use crate::functions::Running;
use crate::stylesheet::{Stylesheet, Template, XSLT_NAMESPACE, in_scope_namespaces};

/// How deep `xsl:apply-templates` and `xsl:call-template` may go before the transformation is
/// refused.
///
/// A stylesheet that recurses without end would otherwise exhaust the stack, and a guard the
/// stack reaches first is no guard at all — so this is set against what a level actually costs
/// in a debug build, which is the expensive case, rather than what a release build could carry.
/// The cost is measured rather than assumed: `the_depth_guard_is_reached_before_the_stack_is`
/// runs a recursing stylesheet, reads the stack addresses off the levels, and fails with the
/// figures if this limit no longer fits in one mebibyte — half of what Rust gives a spawned
/// thread, so that a transformation has as much stack left over as it takes.
///
/// [`Transform::with_max_depth`] raises it for a stylesheet that recurses deeply on purpose, on
/// a thread with the stack to match.
pub const DEFAULT_MAX_DEPTH: usize = 200;

/// The instructions this engine runs, which is what `element-available()` reports.
///
/// XSLT 1.0 §15 has a stylesheet ask before it relies on something, so the answer has to be
/// about this implementation rather than about XSLT on paper. These are the names
/// [`Engine::instruction`] dispatches, minus the top-level declarations that are read at compile
/// time and are not instructions at all.
const INSTRUCTIONS: &[&str] = &[
  "apply-templates",
  "attribute",
  "call-template",
  "choose",
  "comment",
  "copy",
  "copy-of",
  "element",
  "for-each",
  "if",
  "message",
  "param",
  "processing-instruction",
  "text",
  "value-of",
  "variable",
];

/// The stack [`DEFAULT_MAX_DEPTH`] is chosen to fit inside.
///
/// Half of the two mebibytes Rust gives a spawned thread by default, so that a transformation
/// running on such a thread has as much stack left over as it takes.
#[cfg(test)]
const STACK_BUDGET: usize = 1024 * 1024;

/// The tree a transformation produced.
///
/// The content hangs from a document fragment rather than from the document itself, because a
/// result tree need not be one element — a stylesheet may produce several, or only text, which
/// a document may not hold directly.
#[derive(Debug)]
pub struct ResultTree {
  document: Document,
  root: NodeId,
  messages: Vec<String>,
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

  /// What `xsl:message` said while the transformation ran, in the order it was said.
  ///
  /// A message is for whoever is watching rather than part of the result, so it is kept beside
  /// the tree rather than in it. One with `terminate="yes"` stops the transformation instead,
  /// and comes back as the error.
  #[must_use]
  pub fn messages(&self) -> &[String] {
    &self.messages
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
    self.run_with(stylesheet, model, node, Functions::new())
  }

  /// Runs `stylesheet`, with `functions` available to the expressions in it.
  ///
  /// An extension function is called by a prefixed name — `my:format(…)` — with the prefix
  /// resolved through the stylesheet's own namespace declarations, so a stylesheet chooses what
  /// to call it and the caller only says what it does.
  ///
  /// The set is taken rather than borrowed because XSLT adds its own functions to it —
  /// `current()`, `generate-id()` and the rest of §12.4 — and they are only meaningful for the
  /// transformation they were built for.
  ///
  /// # Errors
  ///
  /// As [`transform`], and whatever an extension function itself raises.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_dom::build;
  /// use xylograph_xdm::DomModel;
  /// use xylograph_xpath::{Context, Functions, Value};
  /// use xylograph_xslt::{Stylesheet, Transform};
  ///
  /// let stylesheet = Stylesheet::compile(
  ///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"
  ///                       xmlns:my="urn:my">
  ///         <xsl:template match="/"><xsl:value-of select="my:shout('hi')"/></xsl:template>
  ///       </xsl:stylesheet>"#,
  ///   "file:///s.xsl",
  /// )?;
  ///
  /// let doc = build::parse("<a/>".as_bytes())?;
  /// let model = DomModel::new(&doc);
  ///
  /// // The set is tied to the model it will run against, so it is built after it.
  /// let functions = Functions::new().with("urn:my", "shout", |arguments: Vec<Value<_>>, context: &Context<'_, _>| {
  ///   Ok(Value::String(arguments[0].string(context.model).to_uppercase()))
  /// });
  ///
  /// let result = Transform::new().run_with(&stylesheet, &model, model.root_node(), functions)?;
  /// assert_eq!(result.text(), "HI");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn run_with<M: Model>(
    &self,
    stylesheet: &Stylesheet,
    model: &M,
    node: M::Node,
    functions: Functions<M>,
  ) -> Result<ResultTree> {
    let mut output = Document::new();
    let root = output.create_document_fragment();
    let running = Rc::new(Running::new());
    let functions = crate::functions::register(functions, &running, INSTRUCTIONS);
    let mut engine = Engine {
      stylesheet,
      model,
      functions,
      running,
      output,
      insertion: vec![root],
      scopes: Vec::new(),
      expressions: HashMap::new(),
      namespaces: HashMap::new(),
      messages: Vec::new(),
      attribute_set_chain: Vec::new(),
      depth: 0,
      max_depth: self.max_depth,
    };
    engine.bind_global_variables(node)?;
    // Keys are built before the walk: a global variable may call key(), and a key's `use` may
    // read a global variable, so the variables go first and then the tables.
    engine.build_keys(node)?;
    engine.apply(Focus { node, position: 1, size: 1 }, None)?;
    Ok(ResultTree { document: engine.output, root, messages: engine.messages })
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
  /// The functions the expressions in the stylesheet may call: whatever the caller supplied,
  /// with XSLT's own added.
  functions: Functions<M>,
  /// What the XSLT functions read to see where the transformation has got to.
  running: Rc<Running<M::Node>>,
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
  /// What xsl:message has said so far.
  messages: Vec<String>,
  /// The attribute sets being applied, outermost first, so one that uses itself is caught.
  attribute_set_chain: Vec<String>,
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
    let matched = self.stylesheet.template_for_using(self.model, focus.node, mode, self.running.as_ref())?;
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
  ///
  /// Every arm is a call rather than a body. A template that recurses passes through here on
  /// each turn, and a debug build gives one frame slot to each arm's locals whichever arm runs —
  /// so writing the work inline here would charge the whole instruction set to every level of
  /// the recursion. See [`DEFAULT_MAX_DEPTH`] for what that costs.
  ///
  /// The names below are also listed in [`INSTRUCTIONS`], which is what `element-available()`
  /// answers from; `every_instruction_named_as_available_is_one_that_runs` checks they agree.
  fn instruction(&mut self, module: usize, element: NodeId, local: &str, focus: Focus<M::Node>) -> Result<()> {
    match local {
      "value-of" => self.value_of(module, element, focus),
      "text" => self.text(module, element),
      "if" => self.if_instruction(module, element, focus),
      "choose" => self.choose(module, element, focus),
      "for-each" => self.for_each(module, element, focus),
      "apply-templates" => self.apply_templates(module, element, focus),
      "call-template" => self.call_template(module, element, focus),
      "variable" | "param" => self.declare(module, element, local, focus),
      // xsl:with-param is read by the instruction it belongs to, not run on its own.
      "with-param" => Ok(()),
      "element" => self.element_instruction(module, element, focus),
      "attribute" => self.attribute_instruction(module, element, focus),
      "comment" => self.comment(module, element, focus),
      "processing-instruction" => self.processing_instruction(module, element, focus),
      "copy" => self.copy(module, element, focus),
      "copy-of" => self.copy_of(module, element, focus),
      "message" => self.message(module, element, focus),
      // A top-level declaration reached here is not an instruction; it was read at compile time.
      "attribute-set" | "output" | "import" | "include" | "template" | "key" => Ok(()),
      other => self.not_implemented(other),
    }
  }

  /// `xsl:value-of`: the string value of an expression.
  fn value_of(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let select = self.required(module, element, "select", "xsl:value-of")?;
    let text = self.string_of(&select, module, element, focus)?;
    self.append_text(&text);
    Ok(())
  }

  /// `xsl:text`: literal characters, including the whitespace §3.4 would otherwise strip.
  fn text(&mut self, module: usize, element: NodeId) -> Result<()> {
    let text = self.stylesheet.document(module).text_content(element);
    self.append_text(&text);
    Ok(())
  }

  /// `xsl:if`: the body, when the test is true.
  fn if_instruction(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let test = self.required(module, element, "test", "xsl:if")?;
    if self.evaluate(&test, module, element, focus)?.boolean() {
      return self.run_body(module, element, focus);
    }
    Ok(())
  }

  /// `xsl:for-each`: the body once per node, each knowing its place in the list.
  fn for_each(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let select = self.required(module, element, "select", "xsl:for-each")?;
    let nodes = self.node_set(&select, module, element, focus, "xsl:for-each")?;
    for (index, node) in nodes.iter().enumerate() {
      let inner = Focus { node: *node, position: index + 1, size: nodes.len() };
      // Each turn of the loop is its own scope, so a variable declared inside does not leak
      // into the next.
      self.scopes.push(Vec::new());
      let outcome = self.run_body(module, element, inner);
      self.scopes.pop();
      outcome?;
    }
    Ok(())
  }

  /// `xsl:comment`: a comment whose data is what the body produced.
  fn comment(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let text = self.captured_text(module, element, focus)?;
    let node = self.output.create_comment(&text);
    self.append(node)
  }

  /// `xsl:processing-instruction`: a PI whose target is an AVT and whose data is the body.
  fn processing_instruction(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let target = self.required(module, element, "name", "xsl:processing-instruction")?;
    let target = self.attribute_value(&target, module, element, focus)?;
    let data = self.captured_text(module, element, focus)?;
    let node = self.output.create_processing_instruction(&target, &data).map_err(dom_error)?;
    self.append(node)
  }

  /// `xsl:copy-of`: a node-set copied whole, or anything else as its string.
  fn copy_of(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let select = self.required(module, element, "select", "xsl:copy-of")?;
    match self.evaluate(&select, module, element, focus)? {
      // A node-set is copied whole, each node with everything below it.
      Value::NodeSet(nodes) => {
        for node in nodes {
          self.copy_deep(node)?;
        }
        Ok(())
      }
      // Anything else is its string, which is what xsl:value-of would have given.
      other => {
        let text = other.string(self.model);
        self.append_text(&text);
        Ok(())
      }
    }
  }

  /// An instruction a later phase brings, named so that the stylesheet author knows which.
  fn not_implemented(&self, local: &str) -> Result<()> {
    let message = format!("xsl:{local} is not implemented yet; see ROADMAP.md for which phase brings it");
    Err(Error::new(ErrorKind::Xslt, message))
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
        .template_for_using(self.model, inner.node, mode.as_deref(), self.running.as_ref())?
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
    #[cfg(test)]
    stack_probe::record(self.depth);
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

  // --- Instructions that build result nodes -------------------------------------------------

  /// `xsl:element`: an element whose name is worked out while running.
  fn element_instruction(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let name = self.required(module, element, "name", "xsl:element")?;
    let name = self.attribute_value(&name, module, element, focus)?;
    let namespace = match self.stylesheet.document(module).attribute(element, "namespace").map(ToOwned::to_owned) {
      Some(namespace) => Some(self.attribute_value(&namespace, module, element, focus)?),
      None => None,
    };
    let created = self.output.create_element_ns(namespace.as_deref(), &name).map_err(dom_error)?;
    self.append(created)?;
    self.use_attribute_sets(module, element, created, focus)?;
    self.insertion.push(created);
    let outcome = self.run_body(module, element, focus);
    self.insertion.pop();
    outcome
  }

  /// `xsl:attribute`: an attribute added to the element being built.
  fn attribute_instruction(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let name = self.required(module, element, "name", "xsl:attribute")?;
    let name = self.attribute_value(&name, module, element, focus)?;
    let namespace = match self.stylesheet.document(module).attribute(element, "namespace").map(ToOwned::to_owned) {
      Some(namespace) => Some(self.attribute_value(&namespace, module, element, focus)?),
      None => None,
    };
    let value = self.captured_text(module, element, focus)?;
    let target = *self.insertion.last().expect("there is always somewhere to put the result");
    // An attribute added where no element is open has nowhere to go; XSLT calls that an error.
    if self.output.node_type(target) != NodeType::Element {
      let message = format!("xsl:attribute {name:?} has no element to be added to");
      return Err(Error::new(ErrorKind::Xslt, message));
    }
    match namespace {
      Some(namespace) => self.output.set_attribute_ns(target, Some(&namespace), &name, &value),
      None => self.output.set_attribute(target, &name, &value),
    }
    .map_err(dom_error)
  }

  /// `xsl:copy`: the current node itself, without what is under it.
  fn copy(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    match self.model.kind(focus.node) {
      // The root copies as nothing; its content is whatever the body puts there.
      NodeKind::Root => self.run_body(module, element, focus),
      NodeKind::Element => {
        let name = self.model.qualified_name(focus.node).unwrap_or_default();
        let namespace = self.model.expanded_name(focus.node).and_then(|name| name.namespace);
        let created = self.output.create_element_ns(namespace.as_deref(), &name).map_err(dom_error)?;
        self.append(created)?;
        self.use_attribute_sets(module, element, created, focus)?;
        self.insertion.push(created);
        let outcome = self.run_body(module, element, focus);
        self.insertion.pop();
        outcome
      }
      // Everything else copies as itself, and its body is not run.
      _ => self.copy_shallow(focus.node),
    }
  }

  /// Copies one node, and everything below it, into the result.
  fn copy_deep(&mut self, node: M::Node) -> Result<()> {
    if self.model.kind(node) != NodeKind::Element {
      // The root has no node of its own; its children are what there is to copy.
      if self.model.kind(node) == NodeKind::Root {
        for child in self.model.children(node) {
          self.copy_deep(child)?;
        }
        return Ok(());
      }
      return self.copy_shallow(node);
    }
    let name = self.model.qualified_name(node).unwrap_or_default();
    let namespace = self.model.expanded_name(node).and_then(|name| name.namespace);
    let created = self.output.create_element_ns(namespace.as_deref(), &name).map_err(dom_error)?;
    for attribute in self.model.attributes(node) {
      let attribute_name = self.model.qualified_name(attribute).unwrap_or_default();
      let attribute_namespace = self.model.expanded_name(attribute).and_then(|name| name.namespace);
      let value = self.model.string_value(attribute);
      match attribute_namespace {
        Some(namespace) => self.output.set_attribute_ns(created, Some(&namespace), &attribute_name, &value),
        None => self.output.set_attribute(created, &attribute_name, &value),
      }
      .map_err(dom_error)?;
    }
    self.append(created)?;
    self.insertion.push(created);
    let children = self.model.children(node);
    let mut outcome = Ok(());
    for child in children {
      outcome = self.copy_deep(child);
      if outcome.is_err() {
        break;
      }
    }
    self.insertion.pop();
    outcome
  }

  /// Copies a node that has no children of its own — text, a comment, a PI, or an attribute.
  fn copy_shallow(&mut self, node: M::Node) -> Result<()> {
    let value = self.model.string_value(node);
    match self.model.kind(node) {
      NodeKind::Text => {
        self.append_text(&value);
        Ok(())
      }
      NodeKind::Comment => {
        let created = self.output.create_comment(&value);
        self.append(created)
      }
      NodeKind::ProcessingInstruction => {
        let target = self.model.qualified_name(node).unwrap_or_default();
        let created = self.output.create_processing_instruction(&target, &value).map_err(dom_error)?;
        self.append(created)
      }
      NodeKind::Attribute => {
        let name = self.model.qualified_name(node).unwrap_or_default();
        let namespace = self.model.expanded_name(node).and_then(|name| name.namespace);
        let target = *self.insertion.last().expect("there is always somewhere to put the result");
        if self.output.node_type(target) != NodeType::Element {
          return Err(Error::new(ErrorKind::Xslt, format!("the attribute {name:?} has no element to be copied onto")));
        }
        match namespace {
          Some(namespace) => self.output.set_attribute_ns(target, Some(&namespace), &name, &value),
          None => self.output.set_attribute(target, &name, &value),
        }
        .map_err(dom_error)
      }
      // A namespace node is carried by the element that has it, not copied on its own.
      NodeKind::Namespace | NodeKind::Root | NodeKind::Element => Ok(()),
    }
  }

  /// `xsl:message`: text for whoever is watching, and a way to stop.
  fn message(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let text = self.captured_text(module, element, focus)?;
    let terminate = self.stylesheet.document(module).attribute(element, "terminate") == Some("yes");
    if terminate {
      return Err(Error::new(ErrorKind::Xslt, format!("the stylesheet stopped: {text}")));
    }
    self.messages.push(text);
    Ok(())
  }

  /// Adds the attributes of the sets an element's `use-attribute-sets` names.
  fn use_attribute_sets(
    &mut self,
    module: usize,
    element: NodeId,
    target: NodeId,
    focus: Focus<M::Node>,
  ) -> Result<()> {
    // A literal result element names them in the XSLT namespace; an instruction, without one.
    let document = self.stylesheet.document(module);
    let named = document
      .attribute(element, "use-attribute-sets")
      .or_else(|| document.attribute_ns(element, Some(XSLT_NAMESPACE), "use-attribute-sets"))
      .map(ToOwned::to_owned);
    let Some(named) = named else { return Ok(()) };

    for name in named.split_whitespace() {
      // §7.1.4: a set that uses itself, however far around, has no meaning.
      if self.attribute_set_chain.iter().any(|used| used == name) {
        return Err(Error::new(ErrorKind::Xslt, format!("the attribute set {name:?} uses itself")));
      }
      let sets: Vec<(usize, NodeId)> =
        self.stylesheet.attribute_sets_named(name).map(|set| (set.module(), set.element())).collect();
      if sets.is_empty() {
        return Err(Error::new(ErrorKind::Xslt, format!("no attribute set is named {name:?}")));
      }
      self.attribute_set_chain.push(name.to_owned());
      let outcome = self.apply_attribute_sets(&sets, target, focus);
      self.attribute_set_chain.pop();
      outcome?;
    }
    Ok(())
  }

  /// Applies every declaration of one attribute set, lowest import precedence first.
  ///
  /// §7.1.4 merges the declarations of a name rather than choosing between them, so all of them
  /// run and the highest precedence is left standing where two set the same attribute.
  fn apply_attribute_sets(&mut self, sets: &[(usize, NodeId)], target: NodeId, focus: Focus<M::Node>) -> Result<()> {
    for &(set_module, set_element) in sets {
      // A set may use others; those go on first, so that its own attributes win.
      self.use_attribute_sets(set_module, set_element, target, focus)?;
      self.insertion.push(target);
      let outcome = self.run_body(set_module, set_element, focus);
      self.insertion.pop();
      outcome?;
    }
    Ok(())
  }

  /// Runs an instruction's content into a fragment of its own and takes its text.
  ///
  /// `xsl:comment`, `xsl:message` and the value of `xsl:attribute` are all text, however they
  /// were produced, so what their bodies build is flattened rather than kept.
  fn captured_text(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<String> {
    let fragment = self.output.create_document_fragment();
    self.insertion.push(fragment);
    let outcome = self.run_body(module, element, focus);
    self.insertion.pop();
    outcome?;
    Ok(self.output.text_content(fragment))
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
      // A namespace declaration is not copied, and neither is the XSLT attribute that names
      // attribute sets — that is an instruction to this engine, not part of the result.
      .filter(|attribute| {
        document.namespace_uri(*attribute) != Some(xylograph_core::XMLNS_NS_URI)
          && document.namespace_uri(*attribute) != Some(XSLT_NAMESPACE)
      })
      .map(|attribute| {
        (
          document.node_name(attribute),
          document.namespace_uri(attribute).map(ToOwned::to_owned),
          document.node_value(attribute).unwrap_or_default().to_owned(),
        )
      })
      .collect();

    let created = self.output.create_element_ns(namespace.as_deref(), &name).map_err(dom_error)?;
    // The sets are applied first, so an attribute written on the element itself wins.
    self.insertion.push(created);
    let sets = self.use_attribute_sets(module, element, created, focus);
    self.insertion.pop();
    sets?;
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

  /// Fills the key tables `key()` reads (XSLT 1.0 §12.2).
  ///
  /// Every node of the tree is offered to every key's pattern, and a node the pattern covers has
  /// the key's `use` expression evaluated over it. Doing this up front rather than when a key is
  /// first asked for is what lets `key()` be a plain lookup: building a table needs the
  /// stylesheet and the model, and a registered function can hold neither.
  fn build_keys(&mut self, root: M::Node) -> Result<()> {
    if self.stylesheet.keys().is_empty() {
      return Ok(());
    }
    let mut nodes = Vec::new();
    gather(self.model, self.model.root(root), &mut nodes);

    for index in 0..self.stylesheet.keys().len() {
      let key = &self.stylesheet.keys()[index];
      let (namespace, local) = (key.namespace().map(ToOwned::to_owned), key.name().to_owned());
      let (module, element) = (key.module(), key.element());
      let use_expression = key.use_expression().to_owned();
      let namespaces = key.namespaces().clone();
      let pattern = key.pattern().clone();

      for node in &nodes {
        let variables = self.variables();
        // A key's own `match` may not itself be anchored at a key — §12.2 forbids it — so the
        // tables being half-filled here cannot be observed.
        if !pattern.matches_with(self.model, *node, &namespaces, &variables)? {
          continue;
        }
        let focus = Focus { node: *node, position: 1, size: 1 };
        // §12.2: a `use` that gives a node-set puts the node under each of its nodes' string
        // values, so one node can be found by several keys.
        let values = match self.evaluate(&use_expression, module, element, focus)? {
          Value::NodeSet(found) => found.iter().map(|found| self.model.string_value(*found)).collect(),
          other => vec![other.string(self.model)],
        };
        for value in values {
          self.running.add_key_entry(namespace.as_deref(), &local, &value, *node);
        }
      }
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
    // The node the instruction is working on is the current node for as long as this expression
    // runs. It is set here rather than where the focus changes because a predicate moves the
    // context node without moving the current one — that difference is what `current()` is for.
    self.running.set_current(focus.node);
    // The focus carries where the node sits in the list being processed, which is what
    // `position()` and `last()` report.
    let context = Context::new(self.model, focus.node, &namespaces, &variables).with_functions(&self.functions).at(
      focus.node,
      focus.position,
      focus.size,
    );
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

/// Collects a node and everything below it, attributes included.
///
/// A key may cover an attribute as readily as an element, so the walk visits the attribute axis
/// too. Namespace nodes are left out: a pattern cannot select one.
fn gather<M: Model>(model: &M, node: M::Node, into: &mut Vec<M::Node>) {
  into.push(node);
  into.extend(model.attributes(node));
  for child in model.children(node) {
    gather(model, child, into);
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

/// Where the stack stood at each level of a recursing transformation.
///
/// [`DEFAULT_MAX_DEPTH`] is only a guard if it is reached before the stack runs out, and what a
/// level costs is a property of the compiled code rather than something that can be read off the
/// source. So it is measured: `call_template` leaves a mark, and the test below reads the marks.
#[cfg(test)]
mod stack_probe {
  use std::cell::RefCell;

  thread_local! {
    static LEVELS: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
  }

  /// Records how far down the stack this level of the recursion sits.
  pub(super) fn record(depth: usize) {
    let marker = 0u8;
    let address = std::ptr::from_ref(&marker) as usize;
    LEVELS.with(|levels| levels.borrow_mut().push((depth, address)));
  }

  /// Takes what has been recorded so far, leaving the probe empty for the next run.
  pub(super) fn take() -> Vec<(usize, usize)> {
    LEVELS.with(|levels| std::mem::take(&mut *levels.borrow_mut()))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::Stylesheet;

  /// A stylesheet that calls itself for ever, so that the guard is what stops it.
  const ENDLESS: &str = r#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
      <xsl:template match="/"><xsl:call-template name="loop"/></xsl:template>
      <xsl:template name="loop"><xsl:call-template name="loop"/></xsl:template>
    </xsl:stylesheet>"#;

  #[test]
  fn the_depth_guard_is_reached_before_the_stack_is() {
    let stylesheet = Stylesheet::compile(ENDLESS.as_bytes(), "file:///s.xsl").expect("compiles");
    let document = xylograph_dom::build::parse("<a/>".as_bytes()).expect("well-formed");
    let model = xylograph_xdm::DomModel::new(&document);

    let _ = stack_probe::take();
    let outcome = transform(&stylesheet, &model, model.root_node());
    let levels = stack_probe::take();

    let error = outcome.expect_err("the guard stops it");
    assert!(error.message().contains("templates deep"), "{}", error.message());

    // The first level or two pay for setting the transformation up, so measure between two
    // levels that are both well inside the recursion.
    let (shallow_depth, shallow_address) = levels[1];
    let (deep_depth, deep_address) = *levels.last().expect("the guard was reached, so levels were recorded");
    let levels_apart = deep_depth - shallow_depth;
    // The stack grows downwards on every target this runs on, but the subtraction is written so
    // that it would not underflow if that ever stopped being true.
    let bytes_apart = shallow_address.abs_diff(deep_address);
    let per_level = bytes_apart / levels_apart;
    let needed = per_level * DEFAULT_MAX_DEPTH;

    assert!(
      needed <= STACK_BUDGET,
      "a template level costs {per_level} bytes, so DEFAULT_MAX_DEPTH ({DEFAULT_MAX_DEPTH}) needs \
       {} KiB of stack — more than the {} KiB budget. Either the engine's frames have grown and \
       should be trimmed, or DEFAULT_MAX_DEPTH should come down.",
      needed / 1024,
      STACK_BUDGET / 1024,
    );
  }
}
