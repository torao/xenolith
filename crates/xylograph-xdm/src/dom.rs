//! The XPath data model over one or more [`xylograph_dom`] documents.
//!
//! The DOM is close to the XPath model but not the same, so this view adjusts three things as it
//! reads (never writing): a run of adjacent text and CDATA nodes reads as one text node; every
//! element gains namespace nodes for the declarations in scope; and every node — attributes and
//! namespace nodes included — has a place in one document order, computed once when the document
//! joins the model.
//!
//! # More than one document
//!
//! A model begins over one document, the one it was built from. XSLT's `document()` brings in
//! others while a transformation is already running, and they have to join the *same* node space:
//! the nodes it returns are processed by template rules, compared with nodes of the first
//! document, and sorted among them. So a node names its document as well as its place in it, and
//! [`Documents`] is the handle through which a document can be added to a model that is already
//! in use. A caller who never adds one pays a document index it never looks at.

use std::cell::{Ref, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use xylograph_core::name::{NameId, XML_NS_URI, XMLNS_NS_URI};
use xylograph_dom::{Document, NodeId, NodeType};

use crate::{ExpandedName, Model, NodeKind};

/// Which document of a model a node belongs to.
///
/// [`PRIMARY`](DocumentId::PRIMARY) is the one the model was built over; the rest are handed out
/// in the order they were added.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DocumentId(u32);

impl DocumentId {
  /// The document a model was built over.
  pub const PRIMARY: Self = Self(0);
}

/// A node in the XPath view of a document.
///
/// Most nodes are a DOM node ([`Tree`](DomNode::Tree)); a text node names the first DOM node of
/// its run ([`Text`](DomNode::Text)); a namespace node is synthesized from an element and the
/// prefix it binds ([`Namespace`](DomNode::Namespace)). Each carries the document it is in, so
/// that a handle means the same thing in a model holding several.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DomNode {
  /// A DOM node: the document (the XPath root), an element, an attribute, a comment or a PI.
  Tree {
    /// Which document it is in.
    document: DocumentId,
    /// The DOM node itself.
    node: NodeId,
  },
  /// A text node, named by the first DOM text or CDATA node of its run.
  Text {
    /// Which document it is in.
    document: DocumentId,
    /// The first DOM node of the run.
    node: NodeId,
  },
  /// A namespace node: a prefix bound on an element.
  Namespace {
    /// Which document it is in.
    document: DocumentId,
    /// The element the namespace node belongs to.
    element: NodeId,
    /// The prefix it binds, or `None` for the default namespace.
    prefix: Option<NameId>,
  },
}

impl DomNode {
  /// Which document the node is in.
  #[must_use]
  pub const fn document(&self) -> DocumentId {
    match self {
      Self::Tree { document, .. } | Self::Text { document, .. } | Self::Namespace { document, .. } => *document,
    }
  }
}

/// Documents a model gains after it was built.
///
/// A handle is shared: a clone names the same set, so whoever fetches a document and whoever
/// reads it need not be the same code. That is what lets XSLT's `document()` — a function
/// registered before the transformation starts, which cannot borrow the model — put a tree
/// where the model will find it.
///
/// A handle belongs to one model. Giving the same handle to two models over different documents
/// would leave [`DocumentId::PRIMARY`] meaning two different things.
#[derive(Clone, Debug, Default)]
pub struct Documents {
  shared: Rc<Shared>,
}

#[derive(Debug, Default)]
struct Shared {
  /// The documents added, in the order they were added; index `i` is `DocumentId(i + 1)`.
  documents: RefCell<Vec<Document>>,
  /// Where each of their nodes sits in its own document's order.
  order: RefCell<HashMap<DomNode, usize>>,
  /// The root of the document a URI was loaded into, so that one URI is fetched once.
  by_uri: RefCell<HashMap<String, DomNode>>,
}

impl Documents {
  /// A set with nothing in it.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Adds a document, giving the root node of it.
  ///
  /// `uri` is what it was loaded from; [`find`](Self::find) looks it up again by that, so a
  /// stylesheet asking for one URI twice gets one tree and not two.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_dom::build;
  /// use xylograph_xdm::{Documents, DomModel, Model};
  ///
  /// let primary = build::parse("<a/>".as_bytes())?;
  /// let documents = Documents::new();
  /// let model = DomModel::with_documents(&primary, &documents);
  ///
  /// let second = build::parse("<b>text</b>".as_bytes())?;
  /// let root = documents.add("urn:second", second);
  /// assert_eq!(model.string_value(root), "text");
  /// assert_eq!(documents.find("urn:second"), Some(root), "asked for again, it is the same tree");
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn add(&self, uri: &str, document: Document) -> DomNode {
    let id = {
      let mut documents = self.shared.documents.borrow_mut();
      documents.push(document);
      DocumentId(u32::try_from(documents.len()).expect("a model holds fewer than four billion documents"))
    };

    // Numbered here rather than on first use, so that document order is settled the moment the
    // tree can be seen.
    let (root, order) = {
      let documents = self.shared.documents.borrow();
      let view = View { doc: &documents[Self::index(id)], id };
      let root = view.root_node();
      let mut order = HashMap::new();
      let mut counter = 0;
      view.number(root, &mut order, &mut counter);
      (root, order)
    };

    self.shared.order.borrow_mut().extend(order);
    self.shared.by_uri.borrow_mut().insert(uri.to_owned(), root);
    root
  }

  /// The root node of the document added under a URI, if one was.
  #[must_use]
  pub fn find(&self, uri: &str) -> Option<DomNode> {
    self.shared.by_uri.borrow().get(uri).copied()
  }

  /// How many documents have been added.
  #[must_use]
  pub fn len(&self) -> usize {
    self.shared.documents.borrow().len()
  }

  /// Whether none have been added.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Where a document sits in the vector; `DocumentId(0)` is the model's own, not one of these.
  fn index(id: DocumentId) -> usize {
    (id.0 - 1) as usize
  }
}

/// One document seen as an XPath tree.
///
/// Every reading of the DOM goes through here, so a model over several documents is the same
/// code applied to each rather than a second implementation.
struct View<'d> {
  doc: &'d Document,
  id: DocumentId,
}

impl View<'_> {
  /// The root node — the document.
  fn root_node(&self) -> DomNode {
    DomNode::Tree { document: self.id, node: self.doc.root() }
  }

  /// The XPath node for a DOM node: a text or CDATA node maps to its run's text node.
  fn node(&self, id: NodeId) -> DomNode {
    if self.is_text_like(id) {
      DomNode::Text { document: self.id, node: self.run_start(id) }
    } else {
      DomNode::Tree { document: self.id, node: id }
    }
  }

  /// Assigns document-order indices: a node, then its namespace nodes, its attributes, and its
  /// children in turn.
  fn number(&self, node: DomNode, order: &mut HashMap<DomNode, usize>, counter: &mut usize) {
    order.insert(node, *counter);
    *counter += 1;
    for namespace in self.namespaces_of(node) {
      order.insert(namespace, *counter);
      *counter += 1;
    }
    for attribute in self.attributes_of(node) {
      order.insert(attribute, *counter);
      *counter += 1;
    }
    for child in self.children_of(node) {
      self.number(child, order, counter);
    }
  }

  fn is_text_like(&self, id: NodeId) -> bool {
    matches!(self.doc.node_type(id), NodeType::Text | NodeType::CdataSection)
  }

  /// The first DOM node of the text run that `id` belongs to.
  fn run_start(&self, id: NodeId) -> NodeId {
    let mut first = id;
    while let Some(previous) = self.doc.previous_sibling(first) {
      if self.is_text_like(previous) {
        first = previous;
      } else {
        break;
      }
    }
    first
  }

  fn children_of(&self, node: DomNode) -> Vec<DomNode> {
    let DomNode::Tree { node: id, .. } = node else { return Vec::new() };
    if !matches!(self.doc.node_type(id), NodeType::Document | NodeType::Element) {
      return Vec::new();
    }
    let mut children = Vec::new();
    let mut child = self.doc.first_child(id);
    while let Some(current) = child {
      match self.doc.node_type(current) {
        NodeType::Text | NodeType::CdataSection => {
          children.push(DomNode::Text { document: self.id, node: current });
          child = self.after_run(current);
        }
        NodeType::Element | NodeType::Comment | NodeType::ProcessingInstruction => {
          children.push(DomNode::Tree { document: self.id, node: current });
          child = self.doc.next_sibling(current);
        }
        // A document type or anything else is not part of the XPath tree.
        _ => child = self.doc.next_sibling(current),
      }
    }
    children
  }

  /// The sibling after the text run beginning at `start`.
  fn after_run(&self, start: NodeId) -> Option<NodeId> {
    let mut next = self.doc.next_sibling(start);
    while let Some(node) = next {
      if self.is_text_like(node) {
        next = self.doc.next_sibling(node);
      } else {
        break;
      }
    }
    next
  }

  fn attributes_of(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree { node: id, .. } if self.doc.node_type(id) == NodeType::Element => self
        .doc
        .attributes(id)
        .iter()
        // Namespace declarations are namespace nodes, not attributes.
        .filter(|&attr| self.declared_prefix(attr).is_none())
        .map(|id| DomNode::Tree { document: self.id, node: id })
        .collect(),
      _ => Vec::new(),
    }
  }

  fn namespaces_of(&self, node: DomNode) -> Vec<DomNode> {
    match node {
      DomNode::Tree { node: id, .. } if self.doc.node_type(id) == NodeType::Element => self
        .in_scope_prefixes(id)
        .into_iter()
        .map(|prefix| DomNode::Namespace { document: self.id, element: id, prefix })
        .collect(),
      _ => Vec::new(),
    }
  }

  /// The prefixes in scope on an element, in a stable order (default first, then by name), each
  /// with a non-empty binding, plus the implicit `xml`.
  fn in_scope_prefixes(&self, element: NodeId) -> Vec<Option<NameId>> {
    let mut seen: Vec<Option<NameId>> = Vec::new();
    let mut result: Vec<Option<NameId>> = Vec::new();
    let mut current = Some(element);
    while let Some(node) = current {
      if self.doc.node_type(node) == NodeType::Element {
        for attribute in self.doc.attributes(node).iter() {
          if let Some(prefix) = self.declared_prefix(attribute) {
            if !seen.contains(&prefix) {
              seen.push(prefix);
              // An empty value undeclares the prefix, so it shadows but adds no node.
              if !self.doc.node_value(attribute).unwrap_or_default().is_empty() {
                result.push(prefix);
              }
            }
          }
        }
      }
      current = self.doc.parent(node);
    }
    if !result.contains(&Some(NameId::XML)) {
      result.push(Some(NameId::XML));
    }
    result.sort_by_key(|&prefix| self.prefix_key(prefix));
    result
  }

  /// A sort key ordering the default namespace first, then prefixes by name.
  fn prefix_key(&self, prefix: Option<NameId>) -> (bool, String) {
    match prefix {
      None => (false, String::new()),
      Some(name) => (true, self.doc.pool().resolve(name).to_owned()),
    }
  }

  /// If `attribute` is a namespace declaration, the prefix it declares (`None` for the default).
  fn declared_prefix(&self, attribute: NodeId) -> Option<Option<NameId>> {
    if self.doc.namespace_uri(attribute) != Some(XMLNS_NS_URI) {
      return None;
    }
    match self.doc.prefix(attribute) {
      Some("xmlns") => Some(self.doc.local_name(attribute).and_then(|local| self.doc.pool().get(local))),
      _ => Some(None),
    }
  }

  /// The namespace URI a prefix is bound to on an element, if any.
  fn namespace_uri(&self, element: NodeId, prefix: Option<NameId>) -> Option<String> {
    if prefix == Some(NameId::XML) {
      return Some(XML_NS_URI.to_owned());
    }
    let mut current = Some(element);
    while let Some(node) = current {
      if self.doc.node_type(node) == NodeType::Element {
        for attribute in self.doc.attributes(node).iter() {
          if self.declared_prefix(attribute) == Some(prefix) {
            let value = self.doc.node_value(attribute).unwrap_or_default();
            return (!value.is_empty()).then(|| value.to_owned());
          }
        }
      }
      current = self.doc.parent(node);
    }
    None
  }

  fn kind(&self, node: DomNode) -> NodeKind {
    match node {
      DomNode::Text { .. } => NodeKind::Text,
      DomNode::Namespace { .. } => NodeKind::Namespace,
      DomNode::Tree { node: id, .. } => match self.doc.node_type(id) {
        NodeType::Document => NodeKind::Root,
        NodeType::Element => NodeKind::Element,
        NodeType::Attribute => NodeKind::Attribute,
        NodeType::Comment => NodeKind::Comment,
        NodeType::ProcessingInstruction => NodeKind::ProcessingInstruction,
        NodeType::Text | NodeType::CdataSection => NodeKind::Text,
        // A document type or fragment is not an XPath node; nothing reaches here in a tree.
        NodeType::DocumentType | NodeType::DocumentFragment => NodeKind::Root,
      },
    }
  }

  fn parent(&self, node: DomNode) -> Option<DomNode> {
    let tree = |id| DomNode::Tree { document: self.id, node: id };
    match node {
      DomNode::Tree { node: id, .. } if self.doc.node_type(id) == NodeType::Attribute => {
        self.doc.owner_element(id).map(tree)
      }
      DomNode::Tree { node: id, .. } => self.doc.parent(id).map(tree),
      DomNode::Text { node: first, .. } => self.doc.parent(first).map(tree),
      DomNode::Namespace { element, .. } => Some(tree(element)),
    }
  }

  fn expanded_name(&self, node: DomNode) -> Option<ExpandedName> {
    match node {
      DomNode::Tree { node: id, .. } => match self.doc.node_type(id) {
        NodeType::Element | NodeType::Attribute => Some(ExpandedName {
          namespace: self.doc.namespace_uri(id).map(ToOwned::to_owned),
          local: self.doc.local_name(id).unwrap_or_default().to_owned(),
        }),
        NodeType::ProcessingInstruction => Some(ExpandedName { namespace: None, local: self.doc.node_name(id) }),
        _ => None,
      },
      DomNode::Namespace { prefix, .. } => Some(ExpandedName {
        namespace: None,
        local: prefix.map(|name| self.doc.pool().resolve(name).to_owned()).unwrap_or_default(),
      }),
      DomNode::Text { .. } => None,
    }
  }

  fn qualified_name(&self, node: DomNode) -> Option<String> {
    match node {
      // The DOM's node name is already the lexical form, prefix included.
      DomNode::Tree { node: id, .. } => match self.doc.node_type(id) {
        NodeType::Element | NodeType::Attribute | NodeType::ProcessingInstruction => Some(self.doc.node_name(id)),
        _ => None,
      },
      // A namespace node's name is the prefix it binds, and nothing for the default namespace.
      DomNode::Namespace { prefix, .. } => {
        Some(prefix.map_or_else(String::new, |name| self.doc.pool().resolve(name).to_owned()))
      }
      DomNode::Text { .. } => None,
    }
  }

  fn string_value(&self, node: DomNode) -> String {
    match node {
      DomNode::Tree { node: id, .. } => match self.doc.node_type(id) {
        NodeType::Document | NodeType::Element => self.doc.text_content(id),
        NodeType::Attribute | NodeType::Comment | NodeType::ProcessingInstruction => {
          self.doc.node_value(id).unwrap_or_default().to_owned()
        }
        _ => String::new(),
      },
      DomNode::Text { node: first, .. } => {
        let mut value = String::new();
        let mut current = Some(first);
        while let Some(node) = current {
          if self.is_text_like(node) {
            value.push_str(self.doc.node_value(node).unwrap_or_default());
            current = self.doc.next_sibling(node);
          } else {
            break;
          }
        }
        value
      }
      DomNode::Namespace { element, prefix, .. } => self.namespace_uri(element, prefix).unwrap_or_default(),
    }
  }
}

/// The XPath data model over a borrowed [`Document`], and any others added to it.
#[derive(Debug)]
pub struct DomModel<'a> {
  primary: &'a Document,
  /// Every node of the primary document's position in its document order, filled once.
  order: HashMap<DomNode, usize>,
  /// The documents `document()` and its like brought in later.
  extra: Documents,
}

impl<'a> DomModel<'a> {
  /// Builds the model over `doc`, computing document order for the whole tree.
  #[must_use]
  pub fn new(doc: &'a Document) -> Self {
    Self::with_documents(doc, &Documents::new())
  }

  /// Builds the model over `doc`, sharing a set of further documents with whoever else holds it.
  ///
  /// Anything added to `documents` afterwards becomes part of this model's node space.
  #[must_use]
  pub fn with_documents(doc: &'a Document, documents: &Documents) -> Self {
    let view = View { doc, id: DocumentId::PRIMARY };
    let mut order = HashMap::new();
    let mut counter = 0;
    view.number(view.root_node(), &mut order, &mut counter);
    Self { primary: doc, order, extra: documents.clone() }
  }

  /// The root node — the document the model was built over.
  #[must_use]
  pub fn root_node(&self) -> DomNode {
    DomNode::Tree { document: DocumentId::PRIMARY, node: self.primary.root() }
  }

  /// The XPath node for a DOM node of the primary document.
  #[must_use]
  pub fn node(&self, id: NodeId) -> DomNode {
    View { doc: self.primary, id: DocumentId::PRIMARY }.node(id)
  }

  /// The further documents this model can see, for adding one or looking one up.
  #[must_use]
  pub fn documents(&self) -> &Documents {
    &self.extra
  }

  /// Reads a node through the view of whichever document it belongs to.
  fn view<T>(&self, node: DomNode, read: impl FnOnce(&View<'_>) -> T) -> T {
    if node.document() == DocumentId::PRIMARY {
      return read(&View { doc: self.primary, id: DocumentId::PRIMARY });
    }
    let documents: Ref<'_, Vec<Document>> = self.extra.shared.documents.borrow();
    let index = Documents::index(node.document());
    // A node naming a document this model has never held cannot be produced by the model, and
    // handing one in from elsewhere is the caller mixing two node spaces.
    let Some(doc) = documents.get(index) else {
      panic!("a node from document {index} was read by a model that does not hold it");
    };
    read(&View { doc, id: node.document() })
  }

  /// Where a node sits in its own document's order.
  fn position(&self, node: DomNode) -> usize {
    if node.document() == DocumentId::PRIMARY {
      return self.order.get(&node).copied().unwrap_or(usize::MAX);
    }
    self.extra.shared.order.borrow().get(&node).copied().unwrap_or(usize::MAX)
  }
}

impl Model for DomModel<'_> {
  type Node = DomNode;

  fn root(&self, node: DomNode) -> DomNode {
    // The root of the node's *own* document, not the model's first: a node brought in by
    // `document()` has a root of its own, and `/` inside a template applied to it means that.
    self.view(node, |view| view.root_node())
  }

  fn kind(&self, node: DomNode) -> NodeKind {
    self.view(node, |view| view.kind(node))
  }

  fn parent(&self, node: DomNode) -> Option<DomNode> {
    self.view(node, |view| view.parent(node))
  }

  fn children(&self, node: DomNode) -> Vec<DomNode> {
    self.view(node, |view| view.children_of(node))
  }

  fn attributes(&self, node: DomNode) -> Vec<DomNode> {
    self.view(node, |view| view.attributes_of(node))
  }

  fn namespaces(&self, node: DomNode) -> Vec<DomNode> {
    self.view(node, |view| view.namespaces_of(node))
  }

  fn expanded_name(&self, node: DomNode) -> Option<ExpandedName> {
    self.view(node, |view| view.expanded_name(node))
  }

  fn qualified_name(&self, node: DomNode) -> Option<String> {
    self.view(node, |view| view.qualified_name(node))
  }

  fn element_by_id(&self, id: &str) -> Option<DomNode> {
    self.primary.get_element_by_id(id).map(|node| DomNode::Tree { document: DocumentId::PRIMARY, node })
  }

  fn string_value(&self, node: DomNode) -> String {
    self.view(node, |view| view.string_value(node))
  }

  fn document_order(&self, a: DomNode, b: DomNode) -> Ordering {
    // XPath 1.0 §5 requires a total order over every node the model presents, but leaves the
    // order *between* documents to the implementation, asking only that it be consistent. The
    // document a node belongs to decides first, so a whole document sits before or after
    // another rather than interleaving with it, and the model it was built over comes first.
    match a.document().cmp(&b.document()) {
      Ordering::Equal => self.position(a).cmp(&self.position(b)),
      between => between,
    }
  }
}
