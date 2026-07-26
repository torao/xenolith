//! Reading a stylesheet into the form the engine runs.
//!
//! A stylesheet is itself an XML document, so compiling one means walking a [`Document`] and
//! recording what its top-level elements declare. What comes out is a [`Stylesheet`]: the
//! template rules, in the order and with the precedences that decide which of them wins for a
//! given node, and the top-level variables.
//!
//! The bodies of the templates stay where they are, as elements of the documents the stylesheet
//! owns. Executing them is the engine's business (Phase 5c); this phase settles *which* body
//! runs.

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_core::uri;
use xylograph_dom::{Document, NodeId, NodeType, build};
use xylograph_xdm::Model;
use xylograph_xpath::{Namespaces, Variables};

use crate::loader::{Loader, NoLoader};
use crate::pattern::Pattern;

/// The namespace that marks an element as an XSLT instruction rather than a result element.
pub const XSLT_NAMESPACE: &str = "http://www.w3.org/1999/XSL/Transform";

/// A compiled stylesheet: its template rules and top-level variables.
///
/// It owns the documents it was built from — the principal one and everything `xsl:import` and
/// `xsl:include` brought in — because the template bodies are elements of those documents.
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::{DomModel, Model};
/// use xylograph_xslt::Stylesheet;
///
/// let stylesheet = Stylesheet::compile(
///   br#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
///         <xsl:template match="a"/>
///         <xsl:template match="a/b" priority="2"/>
///       </xsl:stylesheet>"#,
///   "file:///s.xsl",
/// )?;
/// assert_eq!(stylesheet.templates().len(), 2);
///
/// // For a `b` inside an `a`, the second rule wins: it is the more specific pattern, and says so.
/// let doc = build::parse("<a><b/></a>".as_bytes())?;
/// let model = DomModel::new(&doc);
/// let b = model.children(model.children(model.root_node())[0])[0];
/// let chosen = stylesheet.template_for(&model, b, None)?.expect("a rule matches");
/// assert_eq!(chosen.priority(), 2.0);
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Debug)]
pub struct Stylesheet {
  modules: Vec<Module>,
  templates: Vec<Template>,
  variables: Vec<Variable>,
}

/// One document of a stylesheet, with the import precedence it was given.
#[derive(Debug)]
struct Module {
  document: Document,
  system_id: String,
  precedence: i32,
}

impl Stylesheet {
  /// Compiles a stylesheet held in a single document.
  ///
  /// # Errors
  ///
  /// [`ErrorKind::Xslt`] if the document is not a stylesheet, or names a module — use
  /// [`compile_with`](Self::compile_with) for a stylesheet built from several. Whatever the
  /// parser raises if the document is not well-formed, and [`ErrorKind::XPath`] if a pattern
  /// cannot be read.
  pub fn compile(source: &[u8], system_id: &str) -> Result<Self> {
    Self::compile_with(source, system_id, &mut NoLoader)
  }

  /// Compiles a stylesheet, fetching the modules it imports and includes through `loader`.
  ///
  /// # Errors
  ///
  /// As [`compile`](Self::compile), and whatever the loader raises for a module it cannot serve.
  ///
  /// # Note on what is read
  ///
  /// The top-level elements this phase understands are `xsl:import`, `xsl:include`,
  /// `xsl:template`, `xsl:variable` and `xsl:param`. Other XSLT top-level elements —
  /// `xsl:output`, `xsl:key`, `xsl:strip-space` and the rest — are read past without complaint;
  /// they arrive in later phases. See `ROADMAP.md`.
  pub fn compile_with<L: Loader>(source: &[u8], system_id: &str, loader: &mut L) -> Result<Self> {
    let mut stylesheet = Self { modules: Vec::new(), templates: Vec::new(), variables: Vec::new() };
    let principal = stylesheet.load_module(source, system_id)?;
    let mut counter = 0;
    stylesheet.assign_precedence(principal, &mut counter, loader)?;
    stylesheet.collect(principal)?;
    // Later declarations win ties, so recording the order settles the last tiebreak.
    for (order, template) in stylesheet.templates.iter_mut().enumerate() {
      template.order = order;
    }
    Ok(stylesheet)
  }

  /// The template rules, in the order they were declared.
  #[must_use]
  pub fn templates(&self) -> &[Template] {
    &self.templates
  }

  /// The top-level variables and parameters, in the order they were declared.
  #[must_use]
  pub fn variables(&self) -> &[Variable] {
    &self.variables
  }

  /// The document a module's elements belong to.
  ///
  /// # Panics
  ///
  /// If `module` is not one of this stylesheet's; the indices come from [`Template::module`]
  /// and [`Variable::module`], so they always are.
  #[must_use]
  pub fn document(&self, module: usize) -> &Document {
    &self.modules[module].document
  }

  /// The template with a given name, for `xsl:call-template`.
  ///
  /// Where two declare the same name, the one with the higher import precedence wins, and then
  /// the later declaration.
  #[must_use]
  pub fn template_named(&self, name: &str) -> Option<&Template> {
    self
      .templates
      .iter()
      .filter(|template| template.name.as_deref() == Some(name))
      .max_by(|a, b| (a.precedence, a.order).cmp(&(b.precedence, b.order)))
  }

  /// The template rule that applies to `node` in `mode`, if any.
  ///
  /// Conflict resolution is XSLT 1.0 §5.5: of the rules whose pattern matches, the highest
  /// import precedence wins; among those, the highest priority; among *those*, the one declared
  /// last. The specification calls the last case an error and allows recovering this way, which
  /// is what happens here.
  ///
  /// # Errors
  ///
  /// Whatever evaluating a pattern's predicates raises.
  pub fn template_for<M: Model>(&self, model: &M, node: M::Node, mode: Option<&str>) -> Result<Option<&Template>> {
    let variables = Variables::new();
    let mut best: Option<&Template> = None;
    for template in &self.templates {
      if template.mode.as_deref() != mode {
        continue;
      }
      let Some(pattern) = &template.pattern else { continue };
      if !pattern.alternatives()[template.alternative].matches_with(model, node, &template.namespaces, &variables)? {
        continue;
      }
      let better = match best {
        None => true,
        Some(current) => template.rank() > current.rank(),
      };
      if better {
        best = Some(template);
      }
    }
    Ok(best)
  }

  // --- Compiling ----------------------------------------------------------------------------

  /// Parses a module and adds it, returning its index.
  fn load_module(&mut self, source: &[u8], system_id: &str) -> Result<usize> {
    let document = build::parse_with_system_id(source, system_id)?;
    self.modules.push(Module { document, system_id: system_id.to_owned(), precedence: 0 });
    Ok(self.modules.len() - 1)
  }

  /// Gives `module` and everything it includes an import precedence, having first given one to
  /// everything they import.
  ///
  /// XSLT 1.0 §2.6.2 orders the import tree by a post-order walk: a module has a higher
  /// precedence than anything it imports, and a later import a higher precedence than an
  /// earlier one. An `xsl:include` is not part of that tree at all — the included module is
  /// treated as though its content were written where the `xsl:include` stands — so it shares
  /// the precedence of the module that includes it, while its own imports join that module's.
  fn assign_precedence<L: Loader>(&mut self, module: usize, counter: &mut i32, loader: &mut L) -> Result<()> {
    let mut included = vec![module];
    let mut imported = Vec::new();
    self.gather(module, &mut included, &mut imported, loader)?;
    for import in imported {
      self.assign_precedence(import, counter, loader)?;
    }
    let precedence = *counter;
    *counter += 1;
    for module in included {
      self.modules[module].precedence = precedence;
    }
    Ok(())
  }

  /// Walks a module's top level, loading what it includes into `included` and noting what it
  /// imports in `imported`.
  fn gather<L: Loader>(
    &mut self,
    module: usize,
    included: &mut Vec<usize>,
    imported: &mut Vec<usize>,
    loader: &mut L,
  ) -> Result<()> {
    for element in self.top_level(module)? {
      let Some(local) = self.xslt_name(module, element) else { continue };
      if local != "import" && local != "include" {
        continue;
      }
      let loaded = self.load_referenced(module, element, &local, loader)?;
      if local == "import" {
        imported.push(loaded);
      } else {
        included.push(loaded);
        // The included module's own content joins this one, imports and all.
        self.gather(loaded, included, imported, loader)?;
      }
    }
    Ok(())
  }

  /// Loads the module an `xsl:import` or `xsl:include` names, resolving its `href`.
  fn load_referenced<L: Loader>(
    &mut self,
    module: usize,
    element: NodeId,
    what: &str,
    loader: &mut L,
  ) -> Result<usize> {
    let document = &self.modules[module].document;
    let Some(href) = document.attribute(element, "href") else {
      return Err(xslt_error(format!("xsl:{what} needs an href")));
    };
    let base = document.base_uri(element).unwrap_or_else(|| self.modules[module].system_id.clone());
    let target = uri::resolve(&base, href)?;
    // A module reached twice would be compiled twice, and a cycle would not terminate at all.
    if self.modules.iter().any(|module| module.system_id == target) {
      return Err(xslt_error(format!("the module {target:?} is brought in more than once")));
    }
    let source = loader.load(&target)?;
    self.load_module(&source, &target)
  }

  /// Records the declarations of a module and everything it includes, in document order.
  fn collect(&mut self, module: usize) -> Result<()> {
    for element in self.top_level(module)? {
      let Some(local) = self.xslt_name(module, element) else { continue };
      match local.as_str() {
        "template" => {
          let templates = self.read_template(module, element)?;
          self.templates.extend(templates);
        }
        "variable" | "param" => {
          let variable = self.read_variable(module, element, local == "param")?;
          self.variables.push(variable);
        }
        "include" => {
          let included = self.referenced_module(module, element)?;
          self.collect(included)?;
        }
        // xsl:import brings in a whole stylesheet, which is collected on its own.
        "import" => {
          let imported = self.referenced_module(module, element)?;
          self.collect(imported)?;
        }
        // Everything else is a top-level element a later phase will read.
        _ => {}
      }
    }
    Ok(())
  }

  /// The module an already-loaded `xsl:import` or `xsl:include` refers to.
  fn referenced_module(&self, module: usize, element: NodeId) -> Result<usize> {
    let document = &self.modules[module].document;
    let href = document.attribute(element, "href").unwrap_or_default();
    let base = document.base_uri(element).unwrap_or_else(|| self.modules[module].system_id.clone());
    let target = uri::resolve(&base, href)?;
    self
      .modules
      .iter()
      .position(|m| m.system_id == target)
      .ok_or_else(|| xslt_error(format!("the module {target:?} was not loaded")))
  }

  /// Reads an `xsl:template`, one [`Template`] per alternative of its pattern.
  fn read_template(&self, module: usize, element: NodeId) -> Result<Vec<Template>> {
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);
    let name = document.attribute(element, "name").map(ToOwned::to_owned);
    let mode = document.attribute(element, "mode").map(ToOwned::to_owned);
    let stated = match document.attribute(element, "priority") {
      Some(text) => Some(read_priority(text)?),
      None => None,
    };
    let precedence = self.modules[module].precedence;

    let Some(match_attribute) = document.attribute(element, "match") else {
      if name.is_none() {
        return Err(xslt_error("xsl:template needs a match or a name"));
      }
      // A template with only a name is called, never matched.
      return Ok(vec![Template {
        pattern: None,
        alternative: 0,
        name,
        mode,
        priority: stated.unwrap_or(0.0),
        precedence,
        order: 0,
        module,
        element,
        namespaces,
      }]);
    };

    let pattern = Pattern::compile(match_attribute)?;
    // Each alternative of a `|` pattern is its own rule, with its own default priority.
    Ok(
      (0..pattern.alternatives().len())
        .map(|alternative| Template {
          priority: stated.unwrap_or_else(|| pattern.alternatives()[alternative].default_priority()),
          pattern: Some(pattern.clone()),
          alternative,
          name: name.clone(),
          mode: mode.clone(),
          precedence,
          order: 0,
          module,
          element,
          namespaces: namespaces.clone(),
        })
        .collect(),
    )
  }

  /// Reads a top-level `xsl:variable` or `xsl:param`.
  fn read_variable(&self, module: usize, element: NodeId, is_parameter: bool) -> Result<Variable> {
    let document = &self.modules[module].document;
    let Some(name) = document.attribute(element, "name").map(ToOwned::to_owned) else {
      let what = if is_parameter { "xsl:param" } else { "xsl:variable" };
      return Err(xslt_error(format!("{what} needs a name")));
    };
    Ok(Variable {
      name,
      select: document.attribute(element, "select").map(ToOwned::to_owned),
      is_parameter,
      precedence: self.modules[module].precedence,
      module,
      element,
      namespaces: in_scope_namespaces(document, element),
    })
  }

  /// The element children of a module's `xsl:stylesheet`, having checked that it is one.
  fn top_level(&self, module: usize) -> Result<Vec<NodeId>> {
    let document = &self.modules[module].document;
    let Some(root) = document.document_element() else {
      return Err(xslt_error("a stylesheet has no document element"));
    };
    let is_stylesheet = document.namespace_uri(root) == Some(XSLT_NAMESPACE)
      && matches!(document.local_name(root), Some("stylesheet" | "transform"));
    if !is_stylesheet {
      let name = document.node_name(root);
      return Err(xslt_error(format!("a stylesheet begins with xsl:stylesheet or xsl:transform, not {name:?}")));
    }
    if document.attribute(root, "version").is_none() {
      return Err(xslt_error("xsl:stylesheet needs a version"));
    }
    Ok(document.children(root).filter(|&child| document.node_type(child) == NodeType::Element).collect())
  }

  /// The local name of an element, if it is in the XSLT namespace.
  fn xslt_name(&self, module: usize, element: NodeId) -> Option<String> {
    let document = &self.modules[module].document;
    (document.namespace_uri(element) == Some(XSLT_NAMESPACE))
      .then(|| document.local_name(element))?
      .map(ToOwned::to_owned)
  }
}

/// One template rule, or one named template.
#[derive(Debug)]
pub struct Template {
  pattern: Option<Pattern>,
  alternative: usize,
  name: Option<String>,
  mode: Option<String>,
  priority: f64,
  precedence: i32,
  order: usize,
  module: usize,
  element: NodeId,
  namespaces: Namespaces,
}

impl Template {
  /// The pattern this rule matches on, or `None` for a template that only has a name.
  #[must_use]
  pub const fn pattern(&self) -> Option<&Pattern> {
    self.pattern.as_ref()
  }

  /// The name `xsl:call-template` calls it by, if it has one.
  #[must_use]
  pub fn name(&self) -> Option<&str> {
    self.name.as_deref()
  }

  /// The mode it applies in, or `None` for the unnamed mode.
  #[must_use]
  pub fn mode(&self) -> Option<&str> {
    self.mode.as_deref()
  }

  /// Its priority: the one the template stated, or the pattern's default.
  #[must_use]
  pub const fn priority(&self) -> f64 {
    self.priority
  }

  /// Its import precedence. Higher wins; the principal stylesheet has the highest.
  #[must_use]
  pub const fn precedence(&self) -> i32 {
    self.precedence
  }

  /// Which of the stylesheet's [documents](Stylesheet::document) its body is in.
  #[must_use]
  pub const fn module(&self) -> usize {
    self.module
  }

  /// The `xsl:template` element itself; its children are the body to run.
  #[must_use]
  pub const fn element(&self) -> NodeId {
    self.element
  }

  /// The namespace bindings in scope where it was declared, which its pattern and the
  /// expressions in its body are read against.
  #[must_use]
  pub const fn namespaces(&self) -> &Namespaces {
    &self.namespaces
  }

  /// How this rule ranks against another: precedence first, then priority, then declaration
  /// order. Priority is compared through [`f64::total_cmp`], so a stated `NaN` orders rather
  /// than making the comparison inconsistent.
  fn rank(&self) -> (i32, RankedPriority, usize) {
    (self.precedence, RankedPriority(self.priority), self.order)
  }
}

/// A priority that can be ordered, so the rules can be ranked without a partial comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RankedPriority(f64);

impl Eq for RankedPriority {}

impl PartialOrd for RankedPriority {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for RankedPriority {
  fn cmp(&self, other: &Self) -> std::cmp::Ordering {
    self.0.total_cmp(&other.0)
  }
}

/// A top-level `xsl:variable` or `xsl:param`.
#[derive(Debug)]
pub struct Variable {
  name: String,
  select: Option<String>,
  is_parameter: bool,
  precedence: i32,
  module: usize,
  element: NodeId,
  namespaces: Namespaces,
}

impl Variable {
  /// Its name, as `$name` refers to it.
  #[must_use]
  pub fn name(&self) -> &str {
    &self.name
  }

  /// The `select` expression, if it has one. Without it the value is the element's content.
  #[must_use]
  pub fn select(&self) -> Option<&str> {
    self.select.as_deref()
  }

  /// Whether it is an `xsl:param`, whose value the caller may supply.
  #[must_use]
  pub const fn is_parameter(&self) -> bool {
    self.is_parameter
  }

  /// Its import precedence.
  #[must_use]
  pub const fn precedence(&self) -> i32 {
    self.precedence
  }

  /// Which of the stylesheet's [documents](Stylesheet::document) it is in.
  #[must_use]
  pub const fn module(&self) -> usize {
    self.module
  }

  /// The declaring element; without a `select`, its content is the value.
  #[must_use]
  pub const fn element(&self) -> NodeId {
    self.element
  }

  /// The namespace bindings in scope where it was declared.
  #[must_use]
  pub const fn namespaces(&self) -> &Namespaces {
    &self.namespaces
  }
}

/// Reads a `priority` attribute, which XSLT requires to be a number.
fn read_priority(text: &str) -> Result<f64> {
  let value = xylograph_xpath::string_to_number(text);
  if value.is_nan() {
    return Err(xslt_error(format!("a template's priority must be a number, not {text:?}")));
  }
  Ok(value)
}

/// The namespace bindings in scope on an element, innermost first.
///
/// A prefix written in a pattern or an expression means what the stylesheet's own declarations
/// say it means, so they have to be gathered from the element the expression was written on.
fn in_scope_namespaces(document: &Document, element: NodeId) -> Namespaces {
  let mut namespaces = Namespaces::new();
  let mut bound: Vec<String> = Vec::new();
  let mut current = Some(element);
  while let Some(node) = current {
    if document.node_type(node) == NodeType::Element {
      for attribute in document.attributes(node).iter() {
        // `xmlns:p="…"` declares p; a bare `xmlns` declares the default namespace, which a
        // prefix in an expression can never name.
        if document.prefix(attribute) != Some("xmlns") {
          continue;
        }
        let Some(prefix) = document.local_name(attribute) else { continue };
        let value = document.node_value(attribute).unwrap_or_default();
        // The innermost declaration wins, so a prefix already seen is not overwritten.
        if !bound.iter().any(|seen| seen == prefix) {
          bound.push(prefix.to_owned());
          if !value.is_empty() {
            namespaces = namespaces.with(prefix, value);
          }
        }
      }
    }
    current = document.parent(node);
  }
  namespaces
}

fn xslt_error(message: impl Into<String>) -> Error {
  Error::new(ErrorKind::Xslt, message.into())
}
