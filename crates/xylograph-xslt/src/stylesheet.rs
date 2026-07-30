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
use xylograph_xdm::{ExpandedName, Model};
use xylograph_xpath::{Namespaces, Variables};

use crate::decimal::{Formats, Symbols};
use crate::loader::{Loader, NoLoader};
use crate::output::Output;
use crate::pattern::{KeyTable, NoKeys, Pattern};

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
  attribute_sets: Vec<AttributeSet>,
  keys: Vec<Key>,
  decimal_formats: Formats,
  space: Vec<SpaceRule>,
  aliases: Vec<NamespaceAlias>,
  output: Output,
  /// The import precedence that set each `xsl:output` attribute, so a lower one cannot undo it.
  output_set: OutputPrecedence,
}

/// Which import precedence set each attribute of [`Output`], for §16's merging.
#[derive(Debug, Default)]
struct OutputPrecedence {
  method: Option<i32>,
  version: Option<i32>,
  encoding: Option<i32>,
  omit_xml_declaration: Option<i32>,
  standalone: Option<i32>,
  doctype_public: Option<i32>,
  doctype_system: Option<i32>,
  indent: Option<i32>,
  media_type: Option<i32>,
}

/// One element name in an `xsl:strip-space` or `xsl:preserve-space` (XSLT 1.0 §3.4).
#[derive(Debug)]
struct SpaceRule {
  test: NameTest,
  /// True for `xsl:strip-space`, false for `xsl:preserve-space`.
  strip: bool,
  precedence: i32,
}

/// A name test as `elements` writes one: `*`, `prefix:*`, or a qualified name.
#[derive(Debug)]
enum NameTest {
  /// `*` — every element.
  Any,
  /// `prefix:*` — every element of a namespace.
  Namespace(Option<String>),
  /// A name, its prefix already resolved.
  Name { namespace: Option<String>, local: String },
}

impl NameTest {
  /// Reads one name of an `elements` list, resolving its prefix where it was written.
  fn parse(text: &str, namespaces: &Namespaces) -> Result<Self> {
    let resolve = |prefix: &str| {
      namespaces
        .get(prefix)
        .map(ToOwned::to_owned)
        .ok_or_else(|| xslt_error(format!("the prefix {prefix:?} of the element name {text:?} is not bound")))
    };
    match text.split_once(':') {
      None if text == "*" => Ok(Self::Any),
      None => Ok(Self::Name { namespace: None, local: text.to_owned() }),
      Some((prefix, "*")) => Ok(Self::Namespace(Some(resolve(prefix)?))),
      Some((prefix, local)) => Ok(Self::Name { namespace: Some(resolve(prefix)?), local: local.to_owned() }),
    }
  }

  /// Whether an element's expanded name passes, and how specific the test is.
  ///
  /// §3.4 settles a conflict between two rules the way §5.5 settles one between two template
  /// rules, so the same default priorities decide: a name is 0, `prefix:*` is -0.25, `*` -0.5.
  fn matches(&self, name: &ExpandedName) -> Option<f64> {
    match self {
      Self::Any => Some(-0.5),
      Self::Namespace(namespace) => (name.namespace == *namespace).then_some(-0.25),
      Self::Name { namespace, local } => (name.namespace == *namespace && name.local == *local).then_some(0.0),
    }
  }
}

/// An `xsl:namespace-alias`: a namespace of the stylesheet standing for one of the result
/// (XSLT 1.0 §7.1.1).
#[derive(Debug)]
struct NamespaceAlias {
  /// The namespace a literal result element is written in.
  stylesheet: Option<String>,
  /// The namespace it goes into the result as.
  result: Option<String>,
  precedence: i32,
}

/// An `xsl:key`: which nodes it covers, and what value each is found by (XSLT 1.0 §12.2).
///
/// A key is not a declaration one of which wins. Every `xsl:key` of a name contributes, whatever
/// its import precedence, so a name may be declared several times over and the entries add up —
/// which is why there is no precedence here.
#[derive(Debug)]
pub struct Key {
  name: String,
  namespace: Option<String>,
  pattern: Pattern,
  use_expression: String,
  module: usize,
  element: NodeId,
  namespaces: Namespaces,
}

impl Key {
  /// Its local name.
  #[must_use]
  pub fn name(&self) -> &str {
    &self.name
  }

  /// The namespace its name is in, if the name was written with a prefix.
  #[must_use]
  pub fn namespace(&self) -> Option<&str> {
    self.namespace.as_deref()
  }

  /// The pattern saying which nodes the key covers.
  #[must_use]
  pub const fn pattern(&self) -> &Pattern {
    &self.pattern
  }

  /// The `use` expression, evaluated with a covered node as the context node.
  #[must_use]
  pub fn use_expression(&self) -> &str {
    &self.use_expression
  }

  /// Which of the stylesheet's [documents](Stylesheet::document) it is in.
  #[must_use]
  pub const fn module(&self) -> usize {
    self.module
  }

  /// The `xsl:key` element itself.
  #[must_use]
  pub const fn element(&self) -> NodeId {
    self.element
  }

  /// The namespace bindings its pattern and `use` expression are read against.
  #[must_use]
  pub const fn namespaces(&self) -> &Namespaces {
    &self.namespaces
  }
}

/// A named set of attributes, which `use-attribute-sets` adds to an element.
#[derive(Debug)]
pub struct AttributeSet {
  name: String,
  precedence: i32,
  module: usize,
  element: NodeId,
}

impl AttributeSet {
  /// The name `use-attribute-sets` refers to it by.
  #[must_use]
  pub fn name(&self) -> &str {
    &self.name
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

  /// The `xsl:attribute-set` element; its `xsl:attribute` children are what it adds.
  #[must_use]
  pub const fn element(&self) -> NodeId {
    self.element
  }
}

/// How `xsl:output` asks for the result to be written (XSLT 1.0 §16).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputMethod {
  /// `method="xml"`, and what a stylesheet that says nothing gets.
  #[default]
  Xml,
  /// `method="html"`.
  Html,
  /// `method="text"`: the character data of the result, and nothing else.
  Text,
}

/// One document of a stylesheet, with the import precedence it was given.
#[derive(Debug)]
struct Module {
  document: Document,
  system_id: String,
  precedence: i32,
  /// Whether its `version` names a later XSLT than this one, which §2.5 asks to be forgiving of.
  forwards_compatible: bool,
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
  /// The top-level elements read here are `xsl:import`, `xsl:include`, `xsl:template`,
  /// `xsl:variable`, `xsl:param`, `xsl:attribute-set`, `xsl:key`, `xsl:decimal-format` and
  /// `xsl:output`. The rest — `xsl:strip-space`, `xsl:namespace-alias` — are read past without
  /// complaint; they arrive in later phases. See `ROADMAP.md`.
  pub fn compile_with<L: Loader>(source: &[u8], system_id: &str, loader: &mut L) -> Result<Self> {
    let mut stylesheet = Self {
      modules: Vec::new(),
      templates: Vec::new(),
      variables: Vec::new(),
      attribute_sets: Vec::new(),
      keys: Vec::new(),
      decimal_formats: Formats::new(),
      space: Vec::new(),
      aliases: Vec::new(),
      output: Output::default(),
      output_set: OutputPrecedence::default(),
    };
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

  /// The attribute sets with a given name, lowest import precedence first.
  ///
  /// Several declarations of one name are all used, not one chosen: XSLT 1.0 §7.1.4 says they
  /// are merged, with a higher precedence winning where two set the same attribute — which is
  /// what applying them in this order does.
  pub fn attribute_sets_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a AttributeSet> + 'a {
    let mut matching: Vec<&AttributeSet> = self.attribute_sets.iter().filter(|set| set.name == name).collect();
    matching.sort_by_key(|set| set.precedence);
    matching.into_iter()
  }

  /// Every attribute set the stylesheet declares.
  #[must_use]
  pub fn attribute_sets(&self) -> &[AttributeSet] {
    &self.attribute_sets
  }

  /// Whether whitespace-only text under an element of this name is stripped (§3.4).
  ///
  /// The default is to keep it: XSLT strips source whitespace only where a stylesheet asks. Of
  /// the rules that name an element, the one of highest import precedence wins, and among those
  /// the most specific name test — which is §5.5's ordering, as §3.4 says to use.
  pub(crate) fn strips_whitespace(&self, name: &ExpandedName) -> bool {
    let mut best: Option<(i32, f64, bool)> = None;
    for rule in &self.space {
      let Some(specificity) = rule.test.matches(name) else { continue };
      let candidate = (rule.precedence, specificity, rule.strip);
      let better = match best {
        None => true,
        Some((precedence, current, _)) => (rule.precedence, specificity) > (precedence, current),
      };
      if better {
        best = Some(candidate);
      }
    }
    best.is_some_and(|(_, _, strip)| strip)
  }

  /// Whether any `xsl:strip-space` was declared at all, so the engine can skip the question.
  pub(crate) fn strips_anything(&self) -> bool {
    self.space.iter().any(|rule| rule.strip)
  }

  /// The namespace a literal result element of `namespace` goes into the result as (§7.1.1).
  pub(crate) fn aliased(&self, namespace: Option<&str>) -> Option<Option<String>> {
    self
      .aliases
      .iter()
      .filter(|alias| alias.stylesheet.as_deref() == namespace)
      .max_by_key(|alias| alias.precedence)
      .map(|alias| alias.result.clone())
  }

  /// Whether the stylesheet asked for any namespace aliasing.
  pub(crate) fn has_aliases(&self) -> bool {
    !self.aliases.is_empty()
  }

  /// Whether a module was written for a version of XSLT later than this one (§2.5).
  ///
  /// Such a module is processed *forwards-compatibly*: an element this does not know is not an
  /// error until it is actually run, and then only if it has no `xsl:fallback`.
  pub(crate) fn forwards_compatible(&self, module: usize) -> bool {
    self.modules[module].forwards_compatible
  }

  /// Whether an element is an extension element rather than a literal result element (§14.1).
  ///
  /// `extension-element-prefixes` names prefixes, and applies to the element it is written on
  /// and everything below it — so the question is answered by walking up from the element until
  /// one of the declarations names the prefix this element's namespace is bound to. An XSLT
  /// element writes the attribute without a prefix; a literal result element writes it in the
  /// XSLT namespace, since on it an unprefixed attribute would be part of the result.
  pub(crate) fn is_extension_element(&self, module: usize, element: NodeId) -> bool {
    let document = &self.modules[module].document;
    let Some(namespace) = document.namespace_uri(element).map(ToOwned::to_owned) else {
      // An element in no namespace can be named by no prefix, so it is never an extension one.
      return false;
    };

    let mut current = Some(element);
    while let Some(node) = current {
      let declared = document
        .attribute(node, "extension-element-prefixes")
        .or_else(|| document.attribute_ns(node, Some(XSLT_NAMESPACE), "extension-element-prefixes"));
      if let Some(declared) = declared {
        let namespaces = in_scope_namespaces(document, node);
        for prefix in declared.split_whitespace() {
          // `#default` names the default namespace, which a literal result element may be in.
          let named = if prefix == "#default" {
            default_namespace(document, node)
          } else {
            namespaces.get(prefix).map(ToOwned::to_owned)
          };
          if named.as_deref() == Some(namespace.as_str()) {
            return true;
          }
        }
      }
      current = document.parent(node);
    }
    false
  }

  /// The base URI of a stylesheet element: its `xml:base` if it has one, else its module's own.
  pub(crate) fn base_uri(&self, module: usize, element: NodeId) -> String {
    let module = &self.modules[module];
    module.document.base_uri(element).unwrap_or_else(|| module.system_id.clone())
  }

  /// The `xsl:decimal-format` declarations, by expanded name; the unnamed one is the default.
  pub(crate) fn decimal_formats(&self) -> &Formats {
    &self.decimal_formats
  }

  /// Every `xsl:key` the stylesheet declares, in declaration order.
  ///
  /// All of them count: §12.2 has the declarations of a name add their entries together rather
  /// than one of them winning.
  #[must_use]
  pub fn keys(&self) -> &[Key] {
    &self.keys
  }

  /// The method `xsl:output` asked the result to be written by, `xml` if it said nothing.
  #[must_use]
  pub const fn output_method(&self) -> OutputMethod {
    self.output.method()
  }

  /// Everything `xsl:output` asked for, with every declaration merged (§16).
  ///
  /// [`ResultTree::serialize`](crate::ResultTree::serialize) carries it out; this is here for a
  /// caller who wants to write the result some other way and still honour what was asked.
  #[must_use]
  pub const fn output(&self) -> &Output {
    &self.output
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
    self.template_for_using(model, node, mode, &NoKeys)
  }

  /// The template rule that applies to `node` in `mode`, with key tables available.
  ///
  /// A rule whose pattern begins `key(…)` needs the tables a transformation built, so `keys` is
  /// how the engine passes them; [`template_for`](Self::template_for) is this with none.
  ///
  /// # Errors
  ///
  /// Whatever evaluating a pattern's predicates raises.
  pub fn template_for_using<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    mode: Option<&str>,
    keys: &dyn KeyTable<M::Node>,
  ) -> Result<Option<&Template>> {
    self.best_template(model, node, mode, keys, None)
  }

  /// The template rule `xsl:apply-imports` would reach from a rule of import precedence
  /// `below`: the best of those the stylesheet holding the current rule *imported*.
  ///
  /// XSLT 1.0 §5.6 says only rules of lower import precedence are considered, which is what
  /// makes `xsl:apply-imports` the way a rule extends the one it overrode rather than replacing
  /// it outright.
  ///
  /// # Errors
  ///
  /// As [`template_for_using`](Self::template_for_using).
  pub fn imported_template_for<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    mode: Option<&str>,
    keys: &dyn KeyTable<M::Node>,
    below: i32,
  ) -> Result<Option<&Template>> {
    self.best_template(model, node, mode, keys, Some(below))
  }

  /// The best rule for a node, optionally only among those below an import precedence.
  fn best_template<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    mode: Option<&str>,
    keys: &dyn KeyTable<M::Node>,
    below: Option<i32>,
  ) -> Result<Option<&Template>> {
    let variables = Variables::new();
    let mut best: Option<&Template> = None;
    for template in &self.templates {
      if template.mode.as_deref() != mode {
        continue;
      }
      if below.is_some_and(|below| template.precedence >= below) {
        continue;
      }
      let Some(pattern) = &template.pattern else { continue };
      let alternative = &pattern.alternatives()[template.alternative];
      if !alternative.matches_using(model, node, &template.namespaces, &variables, keys)? {
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
    // §2.5: a stylesheet whose version is not 1.0 was written for a later XSLT, and this must
    // read it forgivingly rather than refuse it. A version that is not a number at all is not
    // a later XSLT, so it is read as 1.0 and its unknown elements stay errors.
    let forwards_compatible = document
      .document_element()
      .and_then(|root| document.attribute(root, "version"))
      .and_then(|version| version.trim().parse::<f64>().ok())
      .is_some_and(|version| version > 1.0);
    self.modules.push(Module { document, system_id: system_id.to_owned(), precedence: 0, forwards_compatible });
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
        "attribute-set" => {
          let document = &self.modules[module].document;
          let Some(name) = document.attribute(element, "name").map(ToOwned::to_owned) else {
            return Err(xslt_error("xsl:attribute-set needs a name"));
          };
          let precedence = self.modules[module].precedence;
          self.attribute_sets.push(AttributeSet { name, precedence, module, element });
        }
        "key" => {
          let key = self.read_key(module, element)?;
          self.keys.push(key);
        }
        "decimal-format" => self.read_decimal_format(module, element)?,
        "strip-space" | "preserve-space" => self.read_space(module, element, local == "strip-space")?,
        "namespace-alias" => self.read_namespace_alias(module, element)?,
        "output" => self.read_output(module, element)?,
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

  /// Reads a top-level `xsl:output` (§16).
  ///
  /// §16 merges the declarations attribute by attribute rather than choosing one of them, so a
  /// module may set the encoding and an imported one the indentation. Where two set the same
  /// attribute the higher import precedence wins; `cdata-section-elements` is the exception, and
  /// is the union of them all.
  fn read_output(&mut self, module: usize, element: NodeId) -> Result<()> {
    let precedence = self.modules[module].precedence;
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);

    // Whether this declaration outranks whatever set an attribute before it. Equal precedence
    // means a later declaration wins, which is §16's recovery from what it calls an error.
    let wins = |set: &mut Option<i32>| {
      if set.is_some_and(|current| current > precedence) {
        return false;
      }
      *set = Some(precedence);
      true
    };

    if let Some(method) = document.attribute(element, "method") {
      if wins(&mut self.output_set.method) {
        self.output.method = match method {
          "xml" => OutputMethod::Xml,
          "html" => OutputMethod::Html,
          "text" => OutputMethod::Text,
          // A qualified name here names a method an implementation invented; XSLT says an
          // unknown one may be reported.
          other => return Err(xslt_error(format!("xsl:output method {other:?} is not one this can write"))),
        };
      }
    }
    for (attribute, slot, set) in [
      ("version", &mut self.output.version, &mut self.output_set.version),
      ("encoding", &mut self.output.encoding, &mut self.output_set.encoding),
      ("doctype-public", &mut self.output.doctype_public, &mut self.output_set.doctype_public),
      ("doctype-system", &mut self.output.doctype_system, &mut self.output_set.doctype_system),
      ("media-type", &mut self.output.media_type, &mut self.output_set.media_type),
    ] {
      if let Some(value) = document.attribute(element, attribute) {
        if wins(set) {
          *slot = Some(value.to_owned());
        }
      }
    }
    for (attribute, slot, set) in [
      ("omit-xml-declaration", &mut self.output.omit_xml_declaration, &mut self.output_set.omit_xml_declaration),
      ("indent", &mut self.output.indent, &mut self.output_set.indent),
    ] {
      if let Some(value) = document.attribute(element, attribute) {
        if wins(set) {
          *slot = value == "yes";
        }
      }
    }
    if let Some(value) = document.attribute(element, "standalone") {
      if wins(&mut self.output_set.standalone) {
        self.output.standalone = Some(value == "yes");
      }
    }
    // The union, whatever the precedences: every declaration names elements that are to be
    // written as CDATA, and none of them takes that back.
    if let Some(elements) = document.attribute(element, "cdata-section-elements").map(ToOwned::to_owned) {
      for name in elements.split_whitespace() {
        let expanded = match name.split_once(':') {
          None => (None, name.to_owned()),
          Some((prefix, local)) => match namespaces.get(prefix) {
            Some(namespace) => (Some(namespace.to_owned()), local.to_owned()),
            None => {
              return Err(xslt_error(format!("the prefix {prefix:?} of cdata-section-elements is not bound")));
            }
          },
        };
        if !self.output.cdata_section_elements.contains(&expanded) {
          self.output.cdata_section_elements.push(expanded);
        }
      }
    }
    Ok(())
  }

  /// Reads a top-level `xsl:strip-space` or `xsl:preserve-space` (§3.4).
  fn read_space(&mut self, module: usize, element: NodeId, strip: bool) -> Result<()> {
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);
    let what = if strip { "xsl:strip-space" } else { "xsl:preserve-space" };
    let Some(elements) = document.attribute(element, "elements") else {
      return Err(xslt_error(format!("{what} needs an elements")));
    };
    let precedence = self.modules[module].precedence;
    for name in elements.to_owned().split_whitespace() {
      let test = NameTest::parse(name, &namespaces)?;
      self.space.push(SpaceRule { test, strip, precedence });
    }
    Ok(())
  }

  /// Reads a top-level `xsl:namespace-alias` (§7.1.1).
  fn read_namespace_alias(&mut self, module: usize, element: NodeId) -> Result<()> {
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);
    // `#default` names the default namespace, which may be no namespace at all.
    let resolve = |attribute: &str| -> Result<Option<String>> {
      let Some(prefix) = document.attribute(element, attribute) else {
        return Err(xslt_error(format!("xsl:namespace-alias needs a {attribute}")));
      };
      if prefix == "#default" {
        return Ok(default_namespace(document, element));
      }
      match namespaces.get(prefix) {
        Some(namespace) => Ok(Some(namespace.to_owned())),
        None => Err(xslt_error(format!("the prefix {prefix:?} of xsl:namespace-alias is not bound"))),
      }
    };
    let alias = NamespaceAlias {
      stylesheet: resolve("stylesheet-prefix")?,
      result: resolve("result-prefix")?,
      precedence: self.modules[module].precedence,
    };
    self.aliases.push(alias);
    Ok(())
  }

  /// Reads a top-level `xsl:decimal-format` (§12.3).
  ///
  /// Each attribute renames one of the characters a pattern is written with, or one of those the
  /// result is written with; anything not named keeps the default §12.3 lists.
  fn read_decimal_format(&mut self, module: usize, element: NodeId) -> Result<()> {
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);
    let name = match document.attribute(element, "name") {
      None => None,
      Some(name) => Some(match name.split_once(':') {
        None => (None, name.to_owned()),
        Some((prefix, local)) => match namespaces.get(prefix) {
          Some(namespace) => (Some(namespace.to_owned()), local.to_owned()),
          None => {
            return Err(xslt_error(format!("the prefix {prefix:?} of the decimal format {name:?} is not bound")));
          }
        },
      }),
    };

    /// One attribute of an `xsl:decimal-format` that names a single character.
    fn character(document: &Document, element: NodeId, attribute: &str, current: char) -> Result<char> {
      let Some(value) = document.attribute(element, attribute) else { return Ok(current) };
      let mut characters = value.chars();
      match (characters.next(), characters.next()) {
        (Some(character), None) => Ok(character),
        _ => Err(xslt_error(format!("xsl:decimal-format {attribute} is one character, not {value:?}"))),
      }
    }

    let default = Symbols::default();
    let symbols = Symbols {
      decimal_separator: character(document, element, "decimal-separator", default.decimal_separator)?,
      grouping_separator: character(document, element, "grouping-separator", default.grouping_separator)?,
      minus_sign: character(document, element, "minus-sign", default.minus_sign)?,
      percent: character(document, element, "percent", default.percent)?,
      per_mille: character(document, element, "per-mille", default.per_mille)?,
      zero_digit: character(document, element, "zero-digit", default.zero_digit)?,
      digit: character(document, element, "digit", default.digit)?,
      pattern_separator: character(document, element, "pattern-separator", default.pattern_separator)?,
      infinity: document.attribute(element, "infinity").unwrap_or(&default.infinity).to_owned(),
      nan: document.attribute(element, "NaN").unwrap_or(&default.nan).to_owned(),
    };

    // §12.3: two declarations of one name must agree, and one that contradicts another is an
    // error rather than a last-one-wins.
    if let Some(existing) = self.decimal_formats.get(&name) {
      if *existing != symbols {
        let which = name.as_ref().map_or_else(|| "the default".to_owned(), |(_, local)| format!("{local:?}"));
        return Err(xslt_error(format!("two xsl:decimal-format declarations of {which} do not agree")));
      }
    }
    self.decimal_formats.insert(name, symbols);
    Ok(())
  }

  /// Reads a top-level `xsl:key`.
  ///
  /// The name is a QName, so the prefix is resolved here, against what is in scope where the
  /// key was written — the same reasoning as for a pattern's prefixes. A stylesheet and the
  /// expression that calls `key()` may spell the same namespace with different prefixes.
  fn read_key(&self, module: usize, element: NodeId) -> Result<Key> {
    let document = &self.modules[module].document;
    let namespaces = in_scope_namespaces(document, element);
    let Some(name) = document.attribute(element, "name") else {
      return Err(xslt_error("xsl:key needs a name"));
    };
    let Some(match_attribute) = document.attribute(element, "match") else {
      return Err(xslt_error("xsl:key needs a match"));
    };
    let Some(use_expression) = document.attribute(element, "use") else {
      return Err(xslt_error("xsl:key needs a use"));
    };
    let (namespace, local) = match name.split_once(':') {
      None => (None, name.to_owned()),
      Some((prefix, local)) => match namespaces.get(prefix) {
        Some(namespace) => (Some(namespace.to_owned()), local.to_owned()),
        None => return Err(xslt_error(format!("the prefix {prefix:?} of the key name {name:?} is not bound"))),
      },
    };
    Ok(Key {
      name: local,
      namespace,
      pattern: Pattern::compile(match_attribute)?,
      use_expression: use_expression.to_owned(),
      module,
      element,
      namespaces,
    })
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
pub(crate) fn in_scope_namespaces(document: &Document, element: NodeId) -> Namespaces {
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

/// The default namespace in scope on an element, which no prefix names.
///
/// [`in_scope_namespaces`] leaves this out on purpose: an XPath prefix can never stand for the
/// default namespace, so an expression has no use for it. `xsl:namespace-alias` does, through
/// `#default`, which is why it is looked up separately rather than by widening that.
fn default_namespace(document: &Document, element: NodeId) -> Option<String> {
  let mut current = Some(element);
  while let Some(node) = current {
    if document.node_type(node) == NodeType::Element {
      for attribute in document.attributes(node).iter() {
        if document.node_name(attribute) == "xmlns" {
          let value = document.node_value(attribute).unwrap_or_default();
          // An empty value undeclares it, putting the element in no namespace.
          return (!value.is_empty()).then(|| value.to_owned());
        }
      }
    }
    current = document.parent(node);
  }
  None
}

fn xslt_error(message: impl Into<String>) -> Error {
  Error::new(ErrorKind::Xslt, message.into())
}
