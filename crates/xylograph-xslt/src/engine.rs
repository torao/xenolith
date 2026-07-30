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
//! in [`functions`](crate::functions) — `current()`, `document()`, `key()`, `format-number()`,
//! `generate-id()`, `system-property()`, `element-available()` and `function-available()`.
//!
//! `xsl:sort` orders the node list of an `xsl:apply-templates` or an `xsl:for-each`; how two
//! text keys compare is [`collate`](crate::collate)'s business, and depends on the build.
//! `xsl:number` works out where a node sits and writes it as [`number`](crate::number) asks.
//!
//! `xsl:strip-space` decides which source whitespace reaches a template rule, and
//! `xsl:namespace-alias` which namespace a literal result element lands in. An element whose
//! namespace `extension-element-prefixes` names is an extension element (§14): this engine
//! implements none, so such an element defers to its `xsl:fallback` or is reported — never
//! copied into the result, where it would look like output the stylesheet meant.
//!
//! What is still missing is what `xsl:output` asks for beyond its method — indentation, the
//! declaration, a doctype, `disable-output-escaping`; see `ROADMAP.md`. An instruction that is
//! not understood is reported rather than skipped, so a stylesheet never half-runs in silence —
//! and `element-available()` says so beforehand, from the same list. A stylesheet written for a
//! later XSLT (§2.5) is read forgivingly instead: an element this does not know waits until it
//! is reached, and then defers to its `xsl:fallback`.
//!
//! # Whitespace stripping, and how far it reaches
//!
//! §3.4 describes `xsl:strip-space` as removing nodes from the source tree. Here it is applied
//! where the engine takes a list of source nodes — the children a built-in rule walks, and what
//! `xsl:apply-templates` and `xsl:for-each` select — so what is *processed* is right. An XPath
//! expression evaluated for its own sake still counts them: `count(//text())` sees the
//! whitespace that `xsl:apply-templates` would not have processed. Filtering inside the model
//! instead would need a wrapping [`Model`] whose node type is the same, which the
//! [`Functions`] registry, fixed to one model type, cannot be given.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::{Document, NodeId, NodeType};
use xylograph_xdm::{Model, NodeKind};
use xylograph_xpath::{Context, Expr, Functions, Namespaces, PathStart, Value, Variables};

use crate::avt::{self, Piece};
use crate::collate::{CaseOrder, Collator};
use crate::functions::Running;
use crate::loader::{DocumentSource, NoDocuments};
use crate::number::{Format, Grouping, LetterValue};
use crate::output::{self, Output, Writer};
use crate::pattern::Pattern;
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
  "apply-imports",
  "apply-templates",
  "attribute",
  "call-template",
  "choose",
  "comment",
  "copy",
  "copy-of",
  "element",
  "fallback",
  "for-each",
  "if",
  "message",
  "number",
  "param",
  "processing-instruction",
  "sort",
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
  output: Output,
  /// The text nodes written with `disable-output-escaping`, which go out as they stand.
  raw: HashSet<NodeId>,
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

  /// What `xsl:output` asked for.
  #[must_use]
  pub const fn output(&self) -> &Output {
    &self.output
  }

  /// Writes the result out the way `xsl:output` asked (XSLT 1.0 §16).
  ///
  /// The XML method writes XML, the HTML method HTML — which is not the same thing, since an
  /// empty element is written `<br>` and a `script` holds text that must not be escaped — and
  /// the text method writes the characters and no markup.
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
  ///         <xsl:output method="html" omit-xml-declaration="yes"/>
  ///         <xsl:template match="/"><p>text<br/></p></xsl:template>
  ///       </xsl:stylesheet>"#,
  ///   "file:///s.xsl",
  /// )?;
  ///
  /// let doc = build::parse("<a/>".as_bytes())?;
  /// let model = DomModel::new(&doc);
  /// let result = transform(&stylesheet, &model, model.root_node())?;
  /// assert_eq!(result.serialize(), "<p>text<br></p>", "the HTML method leaves br open");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  #[must_use]
  pub fn serialize(&self) -> String {
    Writer::new(&self.document, &self.output, &self.raw).write(self.root)
  }

  /// Writes the result as bytes, in the encoding `xsl:output` asked for.
  ///
  /// # Errors
  ///
  /// [`ErrorKind::Xslt`] if the encoding is not one this build can write — which needs the
  /// `encodings` feature for anything but UTF-8.
  pub fn to_bytes(&self) -> Result<Vec<u8>> {
    output::encode(&self.serialize(), self.output.encoding())
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
#[derive(Clone, Debug)]
pub struct Transform {
  max_depth: usize,
  /// Values for the stylesheet's top-level `xsl:param`s, as the caller supplied them.
  parameters: Vec<(String, String)>,
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
    Self { max_depth: DEFAULT_MAX_DEPTH, parameters: Vec::new() }
  }

  /// How deep template application may go before the transformation is refused.
  ///
  /// The default is deliberately conservative — see [`DEFAULT_MAX_DEPTH`]'s reasoning. Raise it
  /// for a stylesheet that recurses deeply on purpose, on a thread with the stack to match.
  #[must_use]
  pub fn with_max_depth(mut self, depth: usize) -> Self {
    self.max_depth = depth;
    self
  }

  /// Supplies a value for one of the stylesheet's top-level `xsl:param`s (XSLT 1.0 §11.4).
  ///
  /// A parameter given here takes that value and its own default is not evaluated. A top-level
  /// `xsl:variable` is not a parameter and cannot be set this way, whatever it is called — which
  /// is the difference the two declarations exist to draw.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_dom::build;
  /// use xylograph_xdm::DomModel;
  /// use xylograph_xslt::{Stylesheet, Transform};
  ///
  /// let stylesheet = Stylesheet::compile(
  ///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  ///         <xsl:param name="greeting">Hello</xsl:param>
  ///         <xsl:template match="/"><xsl:value-of select="$greeting"/></xsl:template>
  ///       </xsl:stylesheet>"#,
  ///   "file:///s.xsl",
  /// )?;
  ///
  /// let doc = build::parse("<a/>".as_bytes())?;
  /// let model = DomModel::new(&doc);
  ///
  /// let default = Transform::new().run(&stylesheet, &model, model.root_node())?;
  /// assert_eq!(default.text(), "Hello");
  ///
  /// let given = Transform::new().with_parameter("greeting", "Good day");
  /// assert_eq!(given.run(&stylesheet, &model, model.root_node())?.text(), "Good day");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  #[must_use]
  pub fn with_parameter(mut self, name: &str, value: &str) -> Self {
    // The last value given for a name is the one used, so setting one twice is not an error.
    self.parameters.retain(|(supplied, _)| supplied != name);
    self.parameters.push((name.to_owned(), value.to_owned()));
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
    self.run_with_documents(stylesheet, model, node, functions, Rc::new(NoDocuments))
  }

  /// Runs `stylesheet`, with `documents` for `document()` to fetch trees through.
  ///
  /// The source must put what it fetches into the same node space `model` reads, or the nodes it
  /// hands back name documents that model cannot see.
  /// [`LoadedDocuments`](crate::LoadedDocuments) does that by sharing a
  /// [`Documents`](xylograph_xdm::Documents) handle with the model.
  ///
  /// # Errors
  ///
  /// As [`transform`], and whatever the document source raises for a document it cannot serve.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::rc::Rc;
  /// use xylograph_core::error::Result;
  /// use xylograph_dom::build;
  /// use xylograph_xdm::{DomModel, Documents};
  /// use xylograph_xpath::Functions;
  /// use xylograph_xslt::{LoadedDocuments, Loader, Stylesheet, Transform};
  ///
  /// // A loader that serves one document, whatever it is asked for.
  /// struct Fixed;
  /// impl Loader for Fixed {
  ///   fn load(&mut self, _uri: &str) -> Result<Vec<u8>> {
  ///     Ok(b"<extra><name>Ada</name></extra>".to_vec())
  ///   }
  /// }
  ///
  /// let stylesheet = Stylesheet::compile(
  ///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  ///         <xsl:template match="/">
  ///           <xsl:value-of select="document('other.xml')//name"/>
  ///         </xsl:template>
  ///       </xsl:stylesheet>"#,
  ///   "file:///s.xsl",
  /// )?;
  ///
  /// let source = build::parse("<a/>".as_bytes())?;
  /// // The handle is shared: the model reads what the source fetches.
  /// let documents = Documents::new();
  /// let model = DomModel::with_documents(&source, &documents);
  /// let available = Rc::new(LoadedDocuments::new(&documents, Fixed));
  ///
  /// let result = Transform::new()
  ///   .run_with_documents(&stylesheet, &model, model.root_node(), Functions::new(), available)?;
  /// assert_eq!(result.text().trim(), "Ada");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn run_with_documents<M: Model>(
    &self,
    stylesheet: &Stylesheet,
    model: &M,
    node: M::Node,
    functions: Functions<M>,
    documents: Rc<dyn DocumentSource<M::Node>>,
  ) -> Result<ResultTree> {
    let mut output = Document::new();
    let root = output.create_document_fragment();
    let running = Rc::new(Running::new());
    running.set_decimal_formats(stylesheet.decimal_formats());
    running.set_documents(documents);
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
      raw: HashSet::new(),
      attribute_set_chain: Vec::new(),
      current_rule: None,
      depth: 0,
      max_depth: self.max_depth,
    };
    engine.bind_global_variables(node, &self.parameters)?;
    // Keys are built before the walk: a global variable may call key(), and a key's `use` may
    // read a global variable, so the variables go first and then the tables.
    engine.build_keys(node)?;
    engine.apply(Focus { node, position: 1, size: 1 }, None)?;
    Ok(ResultTree {
      document: engine.output,
      root,
      messages: engine.messages,
      output: stylesheet.output().clone(),
      raw: engine.raw,
    })
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
  /// The result tree fragment the binding holds, when its value came from content rather than
  /// from a `select`. §11.1 lets `xsl:copy-of` copy it; everything else sees only `value`.
  fragment: Option<NodeId>,
}

/// One `xsl:sort`, read once for a whole list (XSLT 1.0 §10).
struct SortSpecification {
  /// What to take the key from; `.` when the sort does not say.
  select: String,
  /// `data-type="number"`, which compares as numbers rather than as text.
  numeric: bool,
  /// `order="descending"`.
  descending: bool,
  /// How text keys are compared, and how case breaks a tie.
  collator: Collator,
  /// The `xsl:sort` element, which the `select` expression is read against.
  element: NodeId,
}

/// One node's key for one `xsl:sort`.
enum SortKey {
  Text(String),
  Number(f64),
}

impl SortKey {
  /// Compares two keys of the same sort.
  fn compare(&self, other: &Self, collator: &Collator) -> Ordering {
    match (self, other) {
      (Self::Text(a), Self::Text(b)) => collator.compare(a, b),
      // A key that cannot be read as a number becomes NaN, and XSLT 1.0 §10 does not say where
      // those go — this is a choice, not a rule. They go first, which is what XSLT 2.0 later
      // settled on and what the processors of the time did, so a stylesheet written against one
      // of those behaves the same here. `total_cmp` orders the rest, so no comparison is ever
      // undefined and the sort cannot misbehave.
      (Self::Number(a), Self::Number(b)) => match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.total_cmp(b),
      },
      // Both keys come from the same xsl:sort, so they are always the same kind.
      _ => Ordering::Equal,
    }
  }
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
  /// Result text nodes written with `disable-output-escaping`.
  raw: HashSet<NodeId>,
  /// The attribute sets being applied, outermost first, so one that uses itself is caught.
  attribute_set_chain: Vec<String>,
  /// The template rule being instantiated, which `xsl:apply-imports` reaches past (§5.6).
  current_rule: Option<CurrentRule>,
  depth: usize,
  max_depth: usize,
}

/// The template rule a body belongs to, as §5.6's "current template rule".
///
/// A rule becomes current by *matching* — `xsl:apply-templates` and `xsl:apply-imports` set it.
/// `xsl:call-template` does not: §5.6 is explicit that calling a template by name leaves the
/// current rule where it was, so an `xsl:apply-imports` inside a called template still means the
/// rule that was matched.
#[derive(Clone, Debug)]
struct CurrentRule {
  /// Its import precedence: `xsl:apply-imports` looks only below this.
  precedence: i32,
  /// The mode it was matched in, which does not change.
  mode: Option<String>,
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
    let rule = (template.module(), template.element(), template.precedence());
    self.run_rule(rule, mode, focus, Vec::new())
  }

  /// Runs a template rule that was reached by matching, as the current rule while it runs.
  ///
  /// What makes this different from [`run_template`](Self::run_template) is only the bookkeeping
  /// §5.6 needs: which rule is current, so that an `xsl:apply-imports` in its body knows what to
  /// reach past. The previous rule is put back afterwards, since a rule may apply templates and
  /// so nest inside another.
  fn run_rule(
    &mut self,
    rule: (usize, NodeId, i32),
    mode: Option<&str>,
    focus: Focus<M::Node>,
    parameters: Vec<Binding<M::Node>>,
  ) -> Result<()> {
    let (module, element, precedence) = rule;
    let previous = self.current_rule.replace(CurrentRule { precedence, mode: mode.map(ToOwned::to_owned) });
    self.depth += 1;
    let outcome = self.run_template(module, element, focus, parameters);
    self.depth -= 1;
    self.current_rule = previous;
    outcome
  }

  /// `xsl:apply-imports`: the rule this one overrode (§5.6).
  ///
  /// The node and the mode do not change — only which rules are eligible, which is what lets a
  /// rule add to an imported one rather than replace it. With no rule of lower precedence to
  /// reach, the built-in rule applies, exactly as it would to a node no rule matches.
  fn apply_imports(&mut self, focus: Focus<M::Node>) -> Result<()> {
    let Some(rule) = self.current_rule.clone() else {
      // §5.6: there has to *be* a current template rule. In the content of a top-level variable,
      // which runs before any rule has matched, there is none — and nothing this could mean.
      let message = "xsl:apply-imports was used where no template rule is current".to_owned();
      return Err(Error::new(ErrorKind::Xslt, message));
    };
    if self.depth >= self.max_depth {
      let message = format!("the transformation is more than {} templates deep", self.max_depth);
      return Err(Error::new(ErrorKind::Xslt, message));
    }
    let mode = rule.mode.clone();
    let matched = self.stylesheet.imported_template_for(
      self.model,
      focus.node,
      mode.as_deref(),
      self.running.as_ref(),
      rule.precedence,
    )?;
    let Some(template) = matched else {
      return self.built_in(focus, mode.as_deref());
    };
    let imported = (template.module(), template.element(), template.precedence());
    self.run_rule(imported, mode.as_deref(), focus, Vec::new())
  }

  /// The rule that applies when the stylesheet declares none (XSLT 1.0 §5.8).
  ///
  /// The root and an element pass the work on to their children; text and an attribute yield
  /// their own characters; a comment, a processing instruction and a namespace yield nothing.
  fn built_in(&mut self, focus: Focus<M::Node>, mode: Option<&str>) -> Result<()> {
    match self.model.kind(focus.node) {
      NodeKind::Root | NodeKind::Element => {
        let mut children = self.model.children(focus.node);
        self.strip_whitespace(&mut children);
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
        // §14: an element whose namespace a `extension-element-prefixes` names is an extension
        // *element*, not something to copy into the result. This engine implements none, so it
        // falls back or reports — never leaks the element into the output.
        if self.stylesheet.is_extension_element(module, node) {
          return self.extension_element(module, node, focus);
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
      "apply-imports" => self.apply_imports(focus),
      "call-template" => self.call_template(module, element, focus),
      "variable" | "param" => self.declare(module, element, local, focus),
      // These are read by the instruction they belong to, not run on their own.
      "with-param" | "sort" => Ok(()),
      "element" => self.element_instruction(module, element, focus),
      "attribute" => self.attribute_instruction(module, element, focus),
      "comment" => self.comment(module, element, focus),
      "processing-instruction" => self.processing_instruction(module, element, focus),
      "copy" => self.copy(module, element, focus),
      "copy-of" => self.copy_of(module, element, focus),
      "number" => self.number(module, element, focus),
      "message" => self.message(module, element, focus),
      "fallback" => Self::fallback(),
      // A top-level declaration reached here is not an instruction; it was read at compile time.
      "attribute-set" | "output" | "import" | "include" | "template" | "key" | "decimal-format" | "strip-space"
      | "preserve-space" | "namespace-alias" => Ok(()),
      other => self.not_implemented(module, element, other, focus),
    }
  }

  /// `xsl:value-of`: the string value of an expression.
  fn value_of(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let select = self.required(module, element, "select", "xsl:value-of")?;
    let text = self.string_of(&select, module, element, focus)?;
    self.append_maybe_raw(module, element, &text);
    Ok(())
  }

  /// `xsl:text`: literal characters, including the whitespace §3.4 would otherwise strip.
  fn text(&mut self, module: usize, element: NodeId) -> Result<()> {
    let text = self.stylesheet.document(module).text_content(element);
    self.append_maybe_raw(module, element, &text);
    Ok(())
  }

  /// Appends text, remembering it if `disable-output-escaping="yes"` was asked for (§16.4).
  ///
  /// Only `xsl:value-of` and `xsl:text` may ask, and only these two call this. The mark is on
  /// the node rather than on the text, so text that reaches the result any other way — a copy,
  /// a literal — is escaped as it should be even if it says the same thing.
  fn append_maybe_raw(&mut self, module: usize, element: NodeId, text: &str) {
    let disabled = self.stylesheet.document(module).attribute(element, "disable-output-escaping") == Some("yes");
    let node = self.append_text_node(text);
    if disabled {
      if let Some(node) = node {
        self.raw.insert(node);
      }
    }
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
    let mut nodes = self.node_set(&select, module, element, focus, "xsl:for-each")?;
    self.strip_whitespace(&mut nodes);
    self.sort_nodes(&mut nodes, module, element, focus)?;
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

  /// `xsl:copy-of`: a node-set copied whole, a result tree fragment copied whole, or anything
  /// else as its string.
  fn copy_of(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let select = self.required(module, element, "select", "xsl:copy-of")?;
    // §11.1 gives a result tree fragment no expression that can carry it, so a variable
    // reference is the only way one can reach here — which is why looking for exactly that is
    // not a shortcut but the whole of the case.
    if let Some(fragment) = self.selected_fragment(&select)? {
      return self.copy_fragment(fragment);
    }
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

  /// The fragment an expression names, if it is exactly `$name` for a variable holding one.
  fn selected_fragment(&mut self, select: &str) -> Result<Option<NodeId>> {
    // Variables are bound by the name as written, prefix and all, as everywhere in this engine.
    let Expr::Variable { prefix, local } = self.expression(select)? else { return Ok(None) };
    let name = match prefix {
      Some(prefix) => format!("{prefix}:{local}"),
      None => local,
    };
    // Innermost binding of the name wins, as everywhere else.
    Ok(
      self
        .scopes
        .iter()
        .rev()
        .flat_map(|scope| scope.iter().rev())
        .find(|binding| binding.name == name)
        .and_then(|binding| binding.fragment),
    )
  }

  /// Copies a result tree fragment of the engine's own document into the result.
  ///
  /// Both trees are the engine's, so this copies node to node rather than going through the
  /// source model as [`copy_deep`](Self::copy_deep) does.
  fn copy_fragment(&mut self, fragment: NodeId) -> Result<()> {
    for child in self.output.children(fragment).collect::<Vec<_>>() {
      let copy = self.output.clone_node(child, true).map_err(dom_error)?;
      self.append(copy)?;
    }
    Ok(())
  }

  /// `xsl:fallback`: what to do instead of an element this cannot run.
  ///
  /// §15 says a fallback reached on its own does nothing: it is only meaningful inside an
  /// element that was not understood, and this engine understood the one it is in.
  const fn fallback() -> Result<()> {
    Ok(())
  }

  /// An XSLT element this engine does not run.
  ///
  /// §2.5: in a module written for a later XSLT, such an element is not an error until it is
  /// reached, and then its `xsl:fallback` children are run instead. Without a fallback — or in a
  /// module that says it is XSLT 1.0, where the element is simply wrong — it is reported, since
  /// a stylesheet that half-runs is worse than one that stops.
  fn not_implemented(&mut self, module: usize, element: NodeId, local: &str, focus: Focus<M::Node>) -> Result<()> {
    if self.stylesheet.forwards_compatible(module) {
      let fallbacks = self.fallback_children(module, element);
      if !fallbacks.is_empty() {
        for fallback in fallbacks {
          self.run_body(module, fallback, focus)?;
        }
        return Ok(());
      }
    }
    let message = format!("xsl:{local} is not implemented yet; see ROADMAP.md for which phase brings it");
    Err(Error::new(ErrorKind::Xslt, message))
  }

  /// An extension element (XSLT 1.0 §14).
  ///
  /// This engine implements none, so every one of them takes §15's route: run the
  /// `xsl:fallback` children if there are any, and report otherwise. What must not happen — and
  /// did, before extension elements were told apart from literal ones — is the element being
  /// copied into the result, where it would look like output the stylesheet meant to produce.
  fn extension_element(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let fallbacks = self.fallback_children(module, element);
    if !fallbacks.is_empty() {
      for fallback in fallbacks {
        self.run_body(module, fallback, focus)?;
      }
      return Ok(());
    }
    let name = self.stylesheet.document(module).node_name(element);
    let message = format!(
      "{name} is an extension element, and this implements none; \
       give it an xsl:fallback, or ask element-available() before relying on it"
    );
    Err(Error::new(ErrorKind::Xslt, message))
  }

  /// The `xsl:fallback` children of an element.
  fn fallback_children(&self, module: usize, element: NodeId) -> Vec<NodeId> {
    let document = self.stylesheet.document(module);
    document
      .children(element)
      .filter(|&child| {
        document.node_type(child) == NodeType::Element
          && document.namespace_uri(child) == Some(XSLT_NAMESPACE)
          && document.local_name(child) == Some("fallback")
      })
      .collect()
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
    let mut nodes = match select {
      Some(select) => self.node_set(&select, module, element, focus, "xsl:apply-templates")?,
      None => self.model.children(focus.node),
    };
    self.strip_whitespace(&mut nodes);
    self.sort_nodes(&mut nodes, module, element, focus)?;
    if parameters.is_empty() {
      return self.apply_to_all(&nodes, mode.as_deref());
    }
    // With parameters the rule has to be found here, so that they can be handed to it.
    for (index, node) in nodes.iter().enumerate() {
      let inner = Focus { node: *node, position: index + 1, size: nodes.len() };
      let matched: Option<(usize, NodeId, i32)> = self
        .stylesheet
        .template_for_using(self.model, inner.node, mode.as_deref(), self.running.as_ref())?
        .map(|template: &Template| (template.module(), template.element(), template.precedence()));
      match matched {
        Some(rule) => {
          let copies = parameters
            .iter()
            .map(|p| Binding { name: p.name.clone(), value: p.value.clone(), fragment: p.fragment })
            .collect();
          self.run_rule(rule, mode.as_deref(), inner, copies)?;
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
      let (value, fragment) = self.declared_value(module, child, focus)?;
      parameters.push(Binding { name, value, fragment });
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
    let (value, fragment) = self.declared_value(module, element, focus)?;
    if let Some(scope) = self.scopes.last_mut() {
      scope.push(Binding { name, value, fragment });
    }
    Ok(())
  }

  /// The value a declaration carries: its `select`, or what its content produces.
  ///
  /// XSLT calls the second a result tree fragment, and §11.1 lets it be used in only two ways:
  /// as a string, and by `xsl:copy-of`. So both are kept — the string, which is what an
  /// expression sees, and the fragment itself, which is what `xsl:copy-of` copies. A fragment
  /// lives in the engine's own result document, so keeping it costs a node identifier and needs
  /// nothing of the source model.
  fn declared_value(
    &mut self,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
  ) -> Result<(Value<M::Node>, Option<NodeId>)> {
    if let Some(select) = self.stylesheet.document(module).attribute(element, "select").map(ToOwned::to_owned) {
      return Ok((self.evaluate(&select, module, element, focus)?, None));
    }
    // Run the content into a fragment of its own, and keep both it and its text.
    let fragment = self.output.create_document_fragment();
    self.insertion.push(fragment);
    let outcome = self.run_body(module, element, focus);
    self.insertion.pop();
    outcome?;
    Ok((Value::String(self.output.text_content(fragment)), Some(fragment)))
  }

  // --- Instructions that build result nodes -------------------------------------------------

  /// The namespace an instruction builds a name in, when it did or did not say which.
  ///
  /// §7.1.2 and §7.1.3: with a `namespace` attribute, that is the namespace, and an empty one
  /// means no namespace at all. Without it, a prefixed name means what that prefix means *in the
  /// stylesheet*, where the instruction was written — the result tree has no declarations of its
  /// own to look in.
  fn namespace_of(
    &mut self,
    said: Option<String>,
    name: &str,
    module: usize,
    element: NodeId,
    what: &str,
  ) -> Result<Option<String>> {
    if let Some(namespace) = said {
      return Ok((!namespace.is_empty()).then_some(namespace));
    }
    let Some((prefix, _)) = name.split_once(':') else { return Ok(None) };
    match self.namespaces_at(module, element).get(prefix) {
      Some(namespace) => Ok(Some(namespace.to_owned())),
      // A prefix the stylesheet never bound names nothing, and building the name anyway would
      // put a prefix in the result that means nothing there either.
      None => {
        let message = format!("{what} names {name:?}, and the prefix {prefix:?} is not bound in the stylesheet");
        Err(Error::new(ErrorKind::Xslt, message))
      }
    }
  }

  /// `xsl:element`: an element whose name is worked out while running.
  fn element_instruction(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let name = self.required(module, element, "name", "xsl:element")?;
    let name = self.attribute_value(&name, module, element, focus)?;
    let said = match self.stylesheet.document(module).attribute(element, "namespace").map(ToOwned::to_owned) {
      Some(namespace) => Some(self.attribute_value(&namespace, module, element, focus)?),
      None => None,
    };
    let namespace = self.namespace_of(said, &name, module, element, "xsl:element")?;
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
    let said = match self.stylesheet.document(module).attribute(element, "namespace").map(ToOwned::to_owned) {
      Some(namespace) => Some(self.attribute_value(&namespace, module, element, focus)?),
      None => None,
    };
    let namespace = self.namespace_of(said, &name, module, element, "xsl:attribute")?;
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
  /// The namespace a literal result element or attribute goes into the result as (§7.1.1).
  ///
  /// The common case is no aliasing at all, and then this is what was written.
  fn aliased_namespace(&self, written: Option<String>) -> Option<String> {
    if !self.stylesheet.has_aliases() {
      return written;
    }
    match self.stylesheet.aliased(written.as_deref()) {
      Some(result) => result,
      None => written,
    }
  }

  fn literal_element(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let document = self.stylesheet.document(module);
    let name = document.node_name(element);
    let written = document.namespace_uri(element).map(ToOwned::to_owned);
    // §7.1.1: an xsl:namespace-alias sends a namespace of the stylesheet into the result as a
    // different one, which is how a stylesheet writes elements of the namespace it is itself in.
    let namespace = self.aliased_namespace(written);
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
      let attribute_namespace = self.aliased_namespace(attribute_namespace);
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
    let _ = self.append_text_node(text);
  }

  /// Appends text, giving the node it became so that a caller can mark it.
  fn append_text_node(&mut self, text: &str) -> Option<NodeId> {
    if text.is_empty() {
      return None;
    }
    let node = self.output.create_text_node(text);
    let parent = *self.insertion.last().expect("there is always somewhere to put the result");
    let _ = self.output.append_child(parent, node);
    Some(node)
  }

  // --- Expressions --------------------------------------------------------------------------

  /// Binds the stylesheet's top-level variables, highest import precedence winning.
  ///
  /// They are evaluated in the order they were declared, so one that refers to another declared
  /// after it is not resolved. XSLT allows that order; sorting the declarations by what they
  /// depend on arrives with the rest of the top-level elements.
  fn bind_global_variables(&mut self, node: M::Node, supplied: &[(String, String)]) -> Result<()> {
    self.scopes.push(Vec::new());
    let declarations: Vec<(usize, NodeId, String, i32, bool)> = self
      .stylesheet
      .variables()
      .iter()
      .map(|variable| {
        (
          variable.module(),
          variable.element(),
          variable.name().to_owned(),
          variable.precedence(),
          variable.is_parameter(),
        )
      })
      .collect();

    for (module, element, name, precedence, is_parameter) in declarations {
      // A declaration of the same name in a module of higher precedence has already won.
      let shadowed =
        self.stylesheet.variables().iter().any(|other| other.name() == name && other.precedence() > precedence);
      if shadowed {
        continue;
      }
      // §11.4: a parameter the caller supplied takes the value given, and its own `select` or
      // content — its *default* — is not evaluated at all. A variable is not a parameter and
      // cannot be set from outside, however the caller spells its name.
      let given = if is_parameter { supplied.iter().find(|(supplied, _)| *supplied == name) } else { None };
      let (value, fragment) = match given {
        Some((_, value)) => (Value::String(value.clone()), None),
        None => {
          let focus = Focus { node, position: 1, size: 1 };
          self.declared_value(module, element, focus)?
        }
      };
      let scope = self.scopes.last_mut().expect("the global scope was just pushed");
      scope.retain(|binding| binding.name != name);
      scope.push(Binding { name, value, fragment });
    }
    Ok(())
  }

  // --- Numbering ----------------------------------------------------------------------------

  /// `xsl:number`: a number worked out from where the node sits, written as `format` asks (§7.7).
  fn number(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<()> {
    let document = self.stylesheet.document(module);
    let value = document.attribute(element, "value").map(ToOwned::to_owned);
    let level = document.attribute(element, "level").unwrap_or("single").to_owned();
    let count = document.attribute(element, "count").map(ToOwned::to_owned);
    let from = document.attribute(element, "from").map(ToOwned::to_owned);

    // `value` says the number outright, and then where the node sits does not come into it.
    let numbers = match value {
      Some(expression) => vec![self.evaluate(&expression, module, element, focus)?.number(self.model)],
      None => {
        let count = match count {
          Some(pattern) => Some(Pattern::compile(&pattern)?),
          None => None,
        };
        let from = match from {
          Some(pattern) => Some(Pattern::compile(&pattern)?),
          None => None,
        };
        let namespaces = self.namespaces_at(module, element);
        self.numbers_for(focus.node, &level, count.as_ref(), from.as_ref(), &namespaces)?
      }
    };

    let format = self.number_format(module, element, focus)?;
    let separator = self.optional_attribute(module, element, "grouping-separator", focus)?;
    let size = match self.optional_attribute(module, element, "grouping-size", focus)? {
      Some(text) => text.trim().parse::<usize>().ok(),
      None => None,
    };
    let grouping = Grouping { separator: separator.as_deref(), size };

    let written = format.format(&numbers, grouping);
    self.append_text(&written);
    Ok(())
  }

  /// Reads the `format` and `letter-value` of an `xsl:number`, both attribute value templates.
  fn number_format(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<Format> {
    let format = self.optional_attribute(module, element, "format", focus)?.unwrap_or_else(|| "1".to_owned());
    let letter_value = match self.optional_attribute(module, element, "letter-value", focus)?.as_deref() {
      None => LetterValue::Unstated,
      Some("alphabetic") => LetterValue::Alphabetic,
      Some("traditional") => LetterValue::Traditional,
      Some(other) => {
        let message = format!("xsl:number letter-value {other:?} is neither alphabetic nor traditional");
        return Err(Error::new(ErrorKind::Xslt, message));
      }
    };
    Ok(Format::parse(&format, letter_value))
  }

  /// The numbers a `level` asks for, outermost first.
  fn numbers_for(
    &mut self,
    node: M::Node,
    level: &str,
    count: Option<&Pattern>,
    from: Option<&Pattern>,
    namespaces: &Namespaces,
  ) -> Result<Vec<f64>> {
    match level {
      "single" => {
        let Some(target) = self.nearest_counted(node, count, from, namespaces)? else {
          return Ok(Vec::new());
        };
        Ok(vec![self.place_among_siblings(target, count, namespaces)?])
      }
      "multiple" => {
        // The node and its ancestors, innermost first, stopping where `from` says to.
        let mut numbers = Vec::new();
        let mut current = Some(node);
        let boundary = self.nearest_matching(node, from, namespaces)?;
        while let Some(candidate) = current {
          if Some(candidate) == boundary {
            break;
          }
          if self.counts(candidate, node, count, namespaces)? {
            numbers.push(self.place_among_siblings(candidate, count, namespaces)?);
          }
          current = self.model.parent(candidate);
        }
        // Gathered from the inside out, but §7.7 writes them from the outside in.
        numbers.reverse();
        Ok(numbers)
      }
      "any" => Ok(vec![self.count_everything_before(node, count, from, namespaces)?]),
      other => {
        let message = format!("xsl:number level {other:?} is not single, multiple or any");
        Err(Error::new(ErrorKind::Xslt, message))
      }
    }
  }

  /// The nearest ancestor-or-self that the count pattern picks out, without passing `from`.
  fn nearest_counted(
    &mut self,
    node: M::Node,
    count: Option<&Pattern>,
    from: Option<&Pattern>,
    namespaces: &Namespaces,
  ) -> Result<Option<M::Node>> {
    let boundary = self.nearest_matching(node, from, namespaces)?;
    let mut current = Some(node);
    while let Some(candidate) = current {
      // §7.7: with `from`, only the ancestors below the nearest one it matches are searched.
      if Some(candidate) == boundary {
        return Ok(None);
      }
      if self.counts(candidate, node, count, namespaces)? {
        return Ok(Some(candidate));
      }
      current = self.model.parent(candidate);
    }
    Ok(None)
  }

  /// The nearest ancestor-or-self a pattern matches, or `None` when there is no pattern.
  fn nearest_matching(
    &mut self,
    node: M::Node,
    pattern: Option<&Pattern>,
    namespaces: &Namespaces,
  ) -> Result<Option<M::Node>> {
    let Some(pattern) = pattern else { return Ok(None) };
    let mut current = self.model.parent(node);
    while let Some(candidate) = current {
      let variables = self.variables();
      if pattern.matches_using(self.model, candidate, namespaces, &variables, self.running.as_ref())? {
        return Ok(Some(candidate));
      }
      current = self.model.parent(candidate);
    }
    Ok(None)
  }

  /// One more than the number of preceding siblings that also count.
  fn place_among_siblings(&mut self, node: M::Node, count: Option<&Pattern>, namespaces: &Namespaces) -> Result<f64> {
    let Some(parent) = self.model.parent(node) else { return Ok(1.0) };
    let siblings = self.model.children(parent);
    let mut place = 1.0;
    for sibling in siblings {
      if sibling == node {
        break;
      }
      if self.counts(sibling, node, count, namespaces)? {
        place += 1.0;
      }
    }
    Ok(place)
  }

  /// Every node at or before `node` in document order that counts, plus one for itself.
  fn count_everything_before(
    &mut self,
    node: M::Node,
    count: Option<&Pattern>,
    from: Option<&Pattern>,
    namespaces: &Namespaces,
  ) -> Result<f64> {
    let mut nodes = Vec::new();
    gather_countable(self.model, self.model.root(node), &mut nodes);
    // §7.7: with `from`, counting restarts after the last node before this one that matches it.
    let mut start = 0;
    if let Some(from) = from {
      for (index, candidate) in nodes.iter().enumerate() {
        if self.model.document_order(*candidate, node) == Ordering::Greater {
          break;
        }
        let variables = self.variables();
        let matched = from.matches_using(self.model, *candidate, namespaces, &variables, self.running.as_ref())?;
        if matched {
          start = index;
        }
      }
    }

    let mut total = 0.0;
    for candidate in &nodes[start..] {
      if self.model.document_order(*candidate, node) == Ordering::Greater {
        break;
      }
      if self.counts(*candidate, node, count, namespaces)? {
        total += 1.0;
      }
    }
    Ok(total)
  }

  /// Whether a node is one the numbering counts.
  ///
  /// With no `count` pattern, §7.7 counts nodes of the same kind as the current one, and of the
  /// same expanded name where it has one — which is not a pattern that can be written down for
  /// every kind, so it is tested directly.
  fn counts(
    &mut self,
    candidate: M::Node,
    current: M::Node,
    count: Option<&Pattern>,
    namespaces: &Namespaces,
  ) -> Result<bool> {
    let Some(pattern) = count else {
      let same_kind = self.model.kind(candidate) == self.model.kind(current);
      return Ok(same_kind && self.model.expanded_name(candidate) == self.model.expanded_name(current));
    };
    let variables = self.variables();
    pattern.matches_using(self.model, candidate, namespaces, &variables, self.running.as_ref())
  }

  /// An attribute value template that need not be there.
  fn optional_attribute(
    &mut self,
    module: usize,
    element: NodeId,
    attribute: &str,
    focus: Focus<M::Node>,
  ) -> Result<Option<String>> {
    let raw = self.stylesheet.document(module).attribute(element, attribute).map(ToOwned::to_owned);
    match raw {
      Some(value) => Ok(Some(self.attribute_value(&value, module, element, focus)?)),
      None => Ok(None),
    }
  }

  // --- Sorting ------------------------------------------------------------------------------

  /// Puts a node list into the order the `xsl:sort` children of an instruction ask for (§10).
  ///
  /// Nothing happens without an `xsl:sort`, and a node list is then in document order, which is
  /// what §5.4 says it should be.
  fn sort_nodes(
    &mut self,
    nodes: &mut Vec<M::Node>,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
  ) -> Result<()> {
    let sorts = self.sort_specifications(module, element, focus)?;
    if sorts.is_empty() || nodes.len() < 2 {
      return Ok(());
    }

    // Every key is worked out before anything moves. The context position a key is evaluated
    // with is the node's place in the list *as selected*, so it must not be read from a list
    // that is half sorted — and it also means each key is computed once rather than once per
    // comparison.
    let mut keyed: Vec<(Vec<SortKey>, usize, M::Node)> = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
      let inner = Focus { node: *node, position: index + 1, size: nodes.len() };
      let mut keys = Vec::with_capacity(sorts.len());
      for sort in &sorts {
        keys.push(self.sort_key(sort, module, inner)?);
      }
      keyed.push((keys, index, *node));
    }

    keyed.sort_by(|a, b| {
      for (position, sort) in sorts.iter().enumerate() {
        let ordering = a.0[position].compare(&b.0[position], &sort.collator);
        let ordering = if sort.descending { ordering.reverse() } else { ordering };
        if ordering != Ordering::Equal {
          return ordering;
        }
      }
      // §10 requires the sort to be stable, so equal keys keep the order they were selected in.
      a.1.cmp(&b.1)
    });

    *nodes = keyed.into_iter().map(|(_, _, node)| node).collect();
    Ok(())
  }

  /// Reads the `xsl:sort` children of an instruction, major key first.
  fn sort_specifications(
    &mut self,
    module: usize,
    element: NodeId,
    focus: Focus<M::Node>,
  ) -> Result<Vec<SortSpecification>> {
    let children: Vec<NodeId> = self.stylesheet.document(module).children(element).collect();
    let mut sorts = Vec::new();
    for child in children {
      let document = self.stylesheet.document(module);
      let is_sort = document.node_type(child) == NodeType::Element
        && document.namespace_uri(child) == Some(XSLT_NAMESPACE)
        && document.local_name(child) == Some("sort");
      if !is_sort {
        continue;
      }
      sorts.push(self.read_sort(module, child, focus)?);
    }
    Ok(sorts)
  }

  /// Reads one `xsl:sort`.
  ///
  /// Its attributes other than `select` are attribute value templates, so they are expanded
  /// once here — §10 evaluates them against the node the instruction is on, not against each of
  /// the nodes being sorted, which is what makes one collator serve the whole list.
  fn read_sort(&mut self, module: usize, element: NodeId, focus: Focus<M::Node>) -> Result<SortSpecification> {
    let document = self.stylesheet.document(module);
    let select = document.attribute(element, "select").unwrap_or(".").to_owned();
    let raw: Vec<Option<String>> = ["lang", "data-type", "order", "case-order"]
      .iter()
      .map(|name| document.attribute(element, name).map(ToOwned::to_owned))
      .collect();

    let mut expanded = Vec::with_capacity(raw.len());
    for value in raw {
      expanded.push(match value {
        Some(value) => Some(self.attribute_value(&value, module, element, focus)?),
        None => None,
      });
    }
    let [lang, data_type, order, case_order] =
      <[Option<String>; 4]>::try_from(expanded).expect("four attribute names were expanded, so four values came back");

    let numeric = match data_type.as_deref() {
      None | Some("text") => false,
      Some("number") => true,
      // A qualified name here names a type an implementation invented.
      Some(other) => {
        let message = format!("xsl:sort data-type {other:?} is not one this understands");
        return Err(Error::new(ErrorKind::Xslt, message));
      }
    };
    let descending = match order.as_deref() {
      None | Some("ascending") => false,
      Some("descending") => true,
      Some(other) => {
        let message = format!("xsl:sort order {other:?} is neither ascending nor descending");
        return Err(Error::new(ErrorKind::Xslt, message));
      }
    };
    let case = match case_order.as_deref() {
      None => CaseOrder::Unstated,
      Some("upper-first") => CaseOrder::UpperFirst,
      Some("lower-first") => CaseOrder::LowerFirst,
      Some(other) => {
        let message = format!("xsl:sort case-order {other:?} is neither upper-first nor lower-first");
        return Err(Error::new(ErrorKind::Xslt, message));
      }
    };

    Ok(SortSpecification { select, numeric, descending, collator: Collator::new(lang.as_deref(), case), element })
  }

  /// Works out one node's key for one `xsl:sort`.
  fn sort_key(&mut self, sort: &SortSpecification, module: usize, focus: Focus<M::Node>) -> Result<SortKey> {
    let value = self.evaluate(&sort.select, module, sort.element, focus)?;
    if sort.numeric {
      return Ok(SortKey::Number(value.number(self.model)));
    }
    Ok(SortKey::Text(value.string(self.model)))
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
    let parsed = self.expression(expression)?;
    let variables = self.variables_for(&parsed, &namespaces)?;
    // The node the instruction is working on is the current node for as long as this expression
    // runs. It is set here rather than where the focus changes because a predicate moves the
    // context node without moving the current one — that difference is what `current()` is for.
    self.running.set_current(focus.node);
    // §12.1 resolves a relative URI in document() against the base URI of the stylesheet element
    // the expression was written on, so that travels with the expression too.
    self.running.set_base_uri(&self.stylesheet.base_uri(module, element));
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

  /// The variables in scope, with any result tree fragment the expression asks to see as a tree.
  ///
  /// XSLT 1.0 §11.1 makes a fragment a string everywhere, and the one thing that lifts that is
  /// `exsl:node-set()`. So the lifting happens here, for the names that expression actually
  /// passes to it and no others: a fragment used any other way stays a string, and `$rtf/foo`
  /// without `exsl:node-set()` is refused, as a conforming XSLT 1.0 processor refuses it.
  ///
  /// That the engine knows one extension function by name is not an accident of layering. §11.1
  /// restricts fragments *because* converting one needs the processor's help, and every XSLT 1.0
  /// processor answers that with the same function; there is nothing for an extension to hook.
  fn variables_for(&mut self, expression: &Expr, namespaces: &Namespaces) -> Result<Variables<M::Node>> {
    let mut wanted = Vec::new();
    fragments_wanted(expression, namespaces, &mut wanted);
    let mut variables = self.variables();
    for name in wanted {
      let Some(fragment) = self.bound_fragment(&name) else { continue };
      let root = self.as_tree(fragment)?;
      variables = variables.with(&name, Value::NodeSet(vec![root]));
    }
    Ok(variables)
  }

  /// The fragment a name is bound to, if its innermost binding holds one.
  fn bound_fragment(&self, name: &str) -> Option<NodeId> {
    self
      .scopes
      .iter()
      .rev()
      .flat_map(|scope| scope.iter().rev())
      .find(|binding| binding.name == name)
      .and_then(|binding| binding.fragment)
  }

  /// Copies a fragment into a document of its own and puts it in the model's node space.
  ///
  /// The answer is kept, so that asking twice gives the same nodes: two calls to
  /// `exsl:node-set()` on one variable have to compare equal, or `count($a | $a)` would be two.
  fn as_tree(&mut self, fragment: NodeId) -> Result<M::Node> {
    if let Some(root) = self.running.adopted(fragment) {
      return Ok(root);
    }
    // The tree hangs from a fragment rather than from the document node: §11.1 lets a result
    // tree fragment hold several elements side by side, and a document may hold only one.
    let mut document = Document::new();
    let root = document.create_document_fragment();
    for child in self.output.children(fragment).collect::<Vec<_>>() {
      let imported = document.import_node(&self.output, child, true).map_err(dom_error)?;
      document.append_child(root, imported).map_err(dom_error)?;
    }
    let adopted = self.running.adopt(document, root)?.ok_or_else(|| {
      Error::new(
        ErrorKind::Xslt,
        "exsl:node-set() needs somewhere to put the tree; run the transformation with a \
         TreeSpace (or a LoadedDocuments) sharing the model's Documents handle"
          .to_owned(),
      )
    })?;
    self.running.remember_adopted(fragment, adopted);
    Ok(adopted)
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

  // --- Whitespace in the source ---------------------------------------------------------------

  /// Drops from a list of source nodes the whitespace `xsl:strip-space` asked to be rid of.
  fn strip_whitespace(&self, nodes: &mut Vec<M::Node>) {
    if !self.stylesheet.strips_anything() {
      return;
    }
    nodes.retain(|node| !self.is_stripped(*node));
  }

  /// Whether a source node is whitespace `xsl:strip-space` asked to be rid of (§3.4).
  ///
  /// Only whitespace-only text counts, and only where the element holding it was named — the
  /// default is to keep every character of the source. `xml:space="preserve"` on the element or
  /// an ancestor overrules the stylesheet, as §3.4 says it must.
  fn is_stripped(&self, node: M::Node) -> bool {
    if self.model.kind(node) != NodeKind::Text {
      return false;
    }
    if !self.model.string_value(node).chars().all(xylograph_core::chars::is_whitespace) {
      return false;
    }
    let Some(parent) = self.model.parent(node) else { return false };
    let Some(name) = self.model.expanded_name(parent) else { return false };
    if !self.stylesheet.strips_whitespace(&name) {
      return false;
    }
    !self.space_is_preserved(parent)
  }

  /// Whether `xml:space="preserve"` is in force on an element or above it.
  fn space_is_preserved(&self, element: M::Node) -> bool {
    let mut current = Some(element);
    while let Some(node) = current {
      for attribute in self.model.attributes(node) {
        let is_xml_space = self.model.expanded_name(attribute).is_some_and(|name| {
          name.local == "space" && name.namespace.as_deref() == Some(xylograph_core::name::XML_NS_URI)
        });
        if is_xml_space {
          // The nearest declaration decides, whichever way it goes.
          return self.model.string_value(attribute) == "preserve";
        }
      }
      current = self.model.parent(node);
    }
    false
  }
}

/// The namespace of EXSLT's common module, whose `node-set()` lifts §11.1's restriction.
const EXSLT_COMMON: &str = "http://exslt.org/common";

/// Collects the variable names an expression passes to `exsl:node-set()`.
///
/// Only a variable reference is looked for, because only a variable can hold a result tree
/// fragment: no expression produces one, so `exsl:node-set(<anything else>)` cannot be about a
/// fragment and needs no lifting.
fn fragments_wanted(expression: &Expr, namespaces: &Namespaces, into: &mut Vec<String>) {
  if let Expr::Function { prefix: Some(prefix), local, arguments } = expression {
    if local == "node-set" && namespaces.get(prefix) == Some(EXSLT_COMMON) {
      if let [Expr::Variable { prefix, local }] = arguments.as_slice() {
        let name = match prefix {
          Some(prefix) => format!("{prefix}:{local}"),
          None => local.clone(),
        };
        into.push(name);
      }
    }
  }
  for child in children_of(expression) {
    fragments_wanted(child, namespaces, into);
  }
}

/// The expressions written inside another.
fn children_of(expression: &Expr) -> Vec<&Expr> {
  match expression {
    Expr::Binary { left, right, .. } => vec![left.as_ref(), right.as_ref()],
    Expr::Negate(inner) => vec![inner.as_ref()],
    Expr::Function { arguments, .. } => arguments.iter().collect(),
    Expr::Filter { expr, predicates } => {
      let mut inside = vec![expr.as_ref()];
      inside.extend(predicates.iter());
      inside
    }
    Expr::Path(path) => {
      let mut inside: Vec<&Expr> = Vec::new();
      if let PathStart::Expr(start) = &path.start {
        inside.push(start.as_ref());
      }
      for step in &path.steps {
        inside.extend(step.predicates.iter());
      }
      inside
    }
    Expr::Literal(_) | Expr::Number(_) | Expr::Variable { .. } => Vec::new(),
  }
}

/// Collects a node and everything below it, in document order, for `level="any"`.
///
/// Attributes and namespace nodes are left out: §7.7 excludes them from the nodes `level="any"`
/// counts over.
fn gather_countable<M: Model>(model: &M, node: M::Node, into: &mut Vec<M::Node>) {
  into.push(node);
  for child in model.children(node) {
    gather_countable(model, child, into);
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
