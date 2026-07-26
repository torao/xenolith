//! Template match patterns.
//!
//! A pattern looks like a location path but asks the opposite question. A path is walked
//! *downwards* from a context node to find nodes; a pattern is a test on one node — does this
//! node match? XSLT 1.0 §5.2 defines the answer by the path's own semantics: a node matches a
//! pattern if the pattern, evaluated from some ancestor of the node (or from the node itself),
//! would select it.
//!
//! That definition is not a way to compute the answer, though; evaluating from every ancestor
//! would be ruinous. Matching here walks the pattern's steps **right to left** from the node
//! instead, moving to the parent at each `/` and searching the ancestors at each `//`, which
//! gives the same answer and touches only the nodes on the way up.
//!
//! Patterns are a strict subset of the path syntax — only the `child` and `attribute` axes, and
//! only `id()` or `key()` where an expression may start a path — so they are read with the
//! [XPath parser](xylograph_xpath::parse) and then checked against the subset, rather than given
//! a second grammar that could drift away from the first.

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_xdm::Model;
use xylograph_xpath::{Axis, BinaryOp, Expr, NameTest, Namespaces, NodeTest, Path, PathStart, Step, Variables};

/// A pattern, as `xsl:template`'s `match` attribute holds one.
///
/// A pattern written with `|` is several patterns in one; XSLT treats each alternative as its own
/// template rule, which is why [`alternatives`](Pattern::alternatives) hands them back separately
/// — each carries its own [default priority](Alternative::default_priority).
///
/// # Examples
///
/// ```
/// use xylograph_dom::build;
/// use xylograph_xdm::{DomModel, Model, NodeKind};
/// use xylograph_xslt::Pattern;
///
/// let doc = build::parse("<r><a><b/></a></r>".as_bytes())?;
/// let model = DomModel::new(&doc);
/// let b = model
///   .children(model.children(model.children(model.root_node())[0])[0])[0];
///
/// assert!(Pattern::compile("b")?.matches(&model, b)?, "an unanchored pattern matches at any depth");
/// assert!(Pattern::compile("a/b")?.matches(&model, b)?);
/// assert!(Pattern::compile("/r/a/b")?.matches(&model, b)?);
/// assert!(!Pattern::compile("r/b")?.matches(&model, b)?, "b's parent is a, not r");
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct Pattern {
  source: String,
  alternatives: Vec<Alternative>,
}

impl Pattern {
  /// Compiles a pattern.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::XPath`] if the text is not a path at all, and
  /// [`ErrorKind::Xslt`](xylograph_core::ErrorKind::Xslt) if it is a path but not one a pattern
  /// may be — an axis other than `child` or `attribute`, or an expression other than `id()` or
  /// `key()` at its head.
  pub fn compile(pattern: &str) -> Result<Self> {
    let expr = xylograph_xpath::parse(pattern)?;
    let mut alternatives = Vec::new();
    collect_alternatives(&expr, &mut alternatives, pattern)?;
    Ok(Self { source: pattern.to_owned(), alternatives })
  }

  /// The pattern as it was written.
  #[must_use]
  pub fn source(&self) -> &str {
    &self.source
  }

  /// The alternatives a `|` separated, each of which XSLT treats as its own template rule.
  #[must_use]
  pub fn alternatives(&self) -> &[Alternative] {
    &self.alternatives
  }

  /// Whether `node` matches any alternative, with nothing bound.
  ///
  /// # Errors
  ///
  /// As [`matches_with`](Self::matches_with).
  pub fn matches<M: Model>(&self, model: &M, node: M::Node) -> Result<bool> {
    self.matches_with(model, node, &Namespaces::new(), &Variables::new())
  }

  /// Whether `node` matches any alternative, with the given bindings in scope for the
  /// predicates.
  ///
  /// # Errors
  ///
  /// Whatever evaluating a predicate raises — an unbound variable or prefix, say.
  pub fn matches_with<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    namespaces: &Namespaces,
    variables: &Variables<M::Node>,
  ) -> Result<bool> {
    for alternative in &self.alternatives {
      if alternative.matches_with(model, node, namespaces, variables)? {
        return Ok(true);
      }
    }
    Ok(false)
  }
}

/// One alternative of a pattern: everything between two `|`.
#[derive(Clone, Debug)]
pub struct Alternative {
  anchor: Anchor,
  steps: Vec<PatternStep>,
  priority: f64,
}

/// Where an alternative is fixed, if it is fixed anywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Anchor {
  /// A relative pattern: the leftmost step may match anywhere.
  Anywhere,
  /// Written with a leading `/`: the leftmost step is a child of the root.
  Root,
  /// Written `id('...')/...`.
  Id(String),
  /// Written `key('name', 'value')/...`. Matching needs the stylesheet's key tables, which do
  /// not exist yet, so an alternative anchored this way never matches.
  Key { name: String, value: String },
}

/// One step of an alternative, always on the `child` or `attribute` axis.
#[derive(Clone, Debug)]
struct PatternStep {
  axis: Axis,
  node_test: NodeTest,
  predicates: Vec<Expr>,
  /// True when a `//` stood before this step, so the step to its left may match any ancestor
  /// rather than the immediate parent.
  after_any_ancestor: bool,
}

impl Alternative {
  /// The priority XSLT 1.0 §5.5 gives this alternative when the template does not set one.
  #[must_use]
  pub const fn default_priority(&self) -> f64 {
    self.priority
  }

  /// Whether `node` matches this alternative alone.
  ///
  /// A template rule is one alternative, not a whole pattern, so the engine asks each in turn
  /// rather than asking the pattern.
  ///
  /// # Errors
  ///
  /// Whatever evaluating a predicate raises.
  pub fn matches_with<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    namespaces: &Namespaces,
    variables: &Variables<M::Node>,
  ) -> Result<bool> {
    // A pattern with no steps is `/`, `id('x')` or `key(...)`, and asks a different question:
    // not whether the node hangs below the anchor, but whether it *is* the anchor.
    let Some(last) = self.steps.len().checked_sub(1) else {
      return Ok(self.is_anchor_target(model, node));
    };
    self.matches_from(model, node, last, namespaces, variables)
  }

  /// Whether `node` matches the step at `index`, and the steps to its left match its ancestors.
  fn matches_from<M: Model>(
    &self,
    model: &M,
    node: M::Node,
    index: usize,
    namespaces: &Namespaces,
    variables: &Variables<M::Node>,
  ) -> Result<bool> {
    let step = &self.steps[index];
    if !step_matches(model, node, step, namespaces, variables)? {
      return Ok(false);
    }
    let Some(next) = index.checked_sub(1) else {
      return Ok(self.anchor_holds(model, node, step.after_any_ancestor));
    };
    if step.after_any_ancestor {
      // A `//` stood here, so the step to the left may match any ancestor. Trying them in turn
      // is a search, but only up the one chain of ancestors.
      let mut current = model.parent(node);
      while let Some(ancestor) = current {
        if self.matches_from(model, ancestor, next, namespaces, variables)? {
          return Ok(true);
        }
        current = model.parent(ancestor);
      }
      return Ok(false);
    }
    match model.parent(node) {
      Some(parent) => self.matches_from(model, parent, next, namespaces, variables),
      None => Ok(false),
    }
  }

  /// Whether the node *is* what the anchor names, for a pattern that has no steps at all.
  fn is_anchor_target<M: Model>(&self, model: &M, node: M::Node) -> bool {
    match &self.anchor {
      // A relative pattern always has at least one step, so this cannot arise.
      Anchor::Anywhere => false,
      Anchor::Root => node == model.root(node),
      Anchor::Id(id) => model.element_by_id(id) == Some(node),
      // The key tables arrive with the stylesheet; until then nothing matches.
      Anchor::Key { .. } => false,
    }
  }

  /// Whether the node the leftmost step matched hangs where the anchor says it must.
  ///
  /// `loose` is true when a `//` stood before that step, which lets any ancestor satisfy the
  /// anchor rather than the immediate parent.
  fn anchor_holds<M: Model>(&self, model: &M, node: M::Node, loose: bool) -> bool {
    match &self.anchor {
      Anchor::Anywhere => true,
      // Everything descends from the root, so `//a` is satisfied by any `a` at all.
      Anchor::Root => loose || model.parent(node) == Some(model.root(node)),
      Anchor::Id(id) => {
        let Some(target) = model.element_by_id(id) else { return false };
        let mut current = model.parent(node);
        if !loose {
          return current == Some(target);
        }
        while let Some(ancestor) = current {
          if ancestor == target {
            return true;
          }
          current = model.parent(ancestor);
        }
        false
      }
      Anchor::Key { .. } => false,
    }
  }
}

/// Whether one node passes one step of a pattern.
fn step_matches<M: Model>(
  model: &M,
  node: M::Node,
  step: &PatternStep,
  namespaces: &Namespaces,
  variables: &Variables<M::Node>,
) -> Result<bool> {
  // Without predicates the question is only about the node, so ask it directly.
  if step.predicates.is_empty() {
    return Ok(node_passes(model, node, step, namespaces));
  }
  // With them it is about the node's place among its siblings, which only the parent knows: run
  // the step from there and look for the node in what it selects.
  let Some(parent) = model.parent(node) else { return Ok(false) };
  let full = Step { axis: step.axis, node_test: step.node_test.clone(), predicates: step.predicates.clone() };
  let selected = xylograph_xpath::evaluate_step(&full, model, parent, namespaces, variables)?;
  Ok(selected.contains(&node))
}

/// Whether a node is on the step's axis from its parent and passes its node test.
fn node_passes<M: Model>(model: &M, node: M::Node, step: &PatternStep, namespaces: &Namespaces) -> bool {
  use xylograph_xdm::NodeKind;

  let kind = model.kind(node);
  // The axis says which side of the parent the node has to be on: an attribute is reached by
  // the attribute axis and never by the child axis, and the other way round.
  let on_axis = match step.axis {
    Axis::Attribute => kind == NodeKind::Attribute,
    _ => !matches!(kind, NodeKind::Attribute | NodeKind::Namespace | NodeKind::Root),
  };
  if !on_axis {
    return false;
  }
  let name = model.expanded_name(node);
  match &step.node_test {
    NodeTest::Node => true,
    NodeTest::Text => kind == NodeKind::Text,
    NodeTest::Comment => kind == NodeKind::Comment,
    NodeTest::ProcessingInstruction(None) => kind == NodeKind::ProcessingInstruction,
    NodeTest::ProcessingInstruction(Some(target)) => {
      kind == NodeKind::ProcessingInstruction && name.is_some_and(|name| name.local == *target)
    }
    // A name test also restricts to the axis's principal node type.
    NodeTest::Name(_) if kind != principal_kind(step.axis) => false,
    NodeTest::Name(NameTest::Any) => true,
    NodeTest::Name(NameTest::Namespace(prefix)) => match namespaces.get(prefix) {
      Some(namespace) => name.is_some_and(|name| name.namespace.as_deref() == Some(namespace)),
      None => false,
    },
    NodeTest::Name(NameTest::Name { prefix, local }) => {
      let expected = match prefix {
        Some(prefix) => match namespaces.get(prefix) {
          Some(namespace) => Some(namespace.to_owned()),
          None => return false,
        },
        None => None,
      };
      name.is_some_and(|name| name.namespace == expected && name.local == *local)
    }
  }
}

const fn principal_kind(axis: Axis) -> xylograph_xdm::NodeKind {
  match axis {
    Axis::Attribute => xylograph_xdm::NodeKind::Attribute,
    _ => xylograph_xdm::NodeKind::Element,
  }
}

// --- Reading a pattern out of a parsed expression ---------------------------------------------

/// Flattens the `|` alternatives of a pattern, checking each against the subset.
fn collect_alternatives(expr: &Expr, out: &mut Vec<Alternative>, source: &str) -> Result<()> {
  if let Expr::Binary { op: BinaryOp::Union, left, right } = expr {
    collect_alternatives(left, out, source)?;
    return collect_alternatives(right, out, source);
  }
  out.push(alternative(expr, source)?);
  Ok(())
}

/// Converts one alternative, which must be a path within the pattern subset.
fn alternative(expr: &Expr, source: &str) -> Result<Alternative> {
  // `id('x')` on its own parses as a call rather than a path.
  if let Expr::Function { .. } = expr {
    let anchor = anchor_of(expr, source)?;
    return Ok(Alternative { anchor, steps: Vec::new(), priority: 0.5 });
  }
  let Expr::Path(path) = expr else {
    return Err(not_a_pattern(source, "a pattern is a path, not an expression"));
  };
  let anchor = match &path.start {
    PathStart::Context => Anchor::Anywhere,
    PathStart::Root => Anchor::Root,
    PathStart::Expr(expr) => anchor_of(expr, source)?,
  };
  let steps = pattern_steps(path, source)?;
  let priority = default_priority(&anchor, &steps);
  Ok(Alternative { anchor, steps, priority })
}

/// Reads the `id()` or `key()` an alternative may begin with.
fn anchor_of(expr: &Expr, source: &str) -> Result<Anchor> {
  let Expr::Function { prefix: None, local, arguments } = expr else {
    return Err(not_a_pattern(source, "only id() and key() may begin a pattern"));
  };
  let literal = |index: usize| match arguments.get(index) {
    Some(Expr::Literal(text)) => Ok(text.clone()),
    _ => Err(not_a_pattern(source, &format!("{local}() in a pattern takes string literals"))),
  };
  match (local.as_str(), arguments.len()) {
    ("id", 1) => Ok(Anchor::Id(literal(0)?)),
    ("key", 2) => Ok(Anchor::Key { name: literal(0)?, value: literal(1)? }),
    ("id" | "key", _) => Err(not_a_pattern(source, &format!("{local}() is given the wrong number of arguments"))),
    _ => Err(not_a_pattern(source, &format!("only id() and key() may begin a pattern, not {local}()"))),
  }
}

/// Converts a path's steps, checking each is one a pattern may hold, and folding the
/// `descendant-or-self::node()` that `//` expands to into a mark on the step after it.
fn pattern_steps(path: &Path, source: &str) -> Result<Vec<PatternStep>> {
  let mut steps: Vec<PatternStep> = Vec::new();
  let mut after_any_ancestor = false;
  for step in &path.steps {
    if is_any_ancestor_marker(step) {
      after_any_ancestor = true;
      continue;
    }
    if !matches!(step.axis, Axis::Child | Axis::Attribute) {
      let message = format!("a pattern may only walk the child and attribute axes, not {}", step.axis);
      return Err(not_a_pattern(source, &message));
    }
    steps.push(PatternStep {
      axis: step.axis,
      node_test: step.node_test.clone(),
      predicates: step.predicates.clone(),
      after_any_ancestor,
    });
    after_any_ancestor = false;
  }
  if after_any_ancestor {
    // Not reachable by writing `//`, which the path grammar already requires a step after, but
    // reachable by writing the `descendant-or-self::node()` it stands for.
    return Err(not_a_pattern(source, "a pattern may not end with a step onto the descendant-or-self axis"));
  }
  Ok(steps)
}

/// Whether a step is the `descendant-or-self::node()` the parser expands `//` into.
fn is_any_ancestor_marker(step: &Step) -> bool {
  step.axis == Axis::DescendantOrSelf && step.node_test == NodeTest::Node && step.predicates.is_empty()
}

/// The default priority of an alternative (XSLT 1.0 §5.5).
///
/// The three low priorities are for a pattern that is nothing but a node test on one of the two
/// axes: a name is 0, a namespace wildcard -0.25, and anything less specific -0.5. A pattern that
/// says more than that — a second step, a predicate, an anchor — is 0.5.
fn default_priority(anchor: &Anchor, steps: &[PatternStep]) -> f64 {
  let [step] = steps else { return 0.5 };
  if *anchor != Anchor::Anywhere || step.after_any_ancestor || !step.predicates.is_empty() {
    return 0.5;
  }
  match &step.node_test {
    NodeTest::Name(NameTest::Name { .. }) | NodeTest::ProcessingInstruction(Some(_)) => 0.0,
    NodeTest::Name(NameTest::Namespace(_)) => -0.25,
    _ => -0.5,
  }
}

fn not_a_pattern(source: &str, why: &str) -> Error {
  Error::new(ErrorKind::Xslt, format!("{source:?} is not a valid pattern: {why}"))
}
