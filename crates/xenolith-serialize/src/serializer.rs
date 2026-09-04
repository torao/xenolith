//! The serializer: a DOM subtree to XML text.

use std::io;

use xenolith_core::name::XMLNS_NS_URI;
use xenolith_dom::{Document, NodeId, NodeType};

use crate::escape::{push_attribute, push_cdata, push_text};

/// Serializes a DOM tree to XML text.
///
/// Output is UTF-8. Escaping, and repair of missing namespace declarations, are automatic; an
/// XML declaration and indentation are opt-in through the builder methods.
///
/// # Examples
///
/// ```
/// use xenolith_dom::Document;
/// use xenolith_serialize::Serializer;
///
/// let mut doc = Document::new();
/// let a = doc.create_element("a")?;
/// let b = doc.create_element("b")?;
/// doc.set_attribute(b, "x", "1 < 2")?;
/// let text = doc.create_text_node("t & u");
/// doc.append_child(b, text)?;
/// doc.append_child(a, b)?;
/// doc.append_child(doc.document_node(), a)?;
///
/// assert_eq!(Serializer::new().to_string(&doc, a), "<a><b x=\"1 &lt; 2\">t &amp; u</b></a>");
/// # Ok::<(), xenolith_dom::DomException>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct Serializer {
  xml_declaration: bool,
  standalone: Option<bool>,
  indent: Option<String>,
}

impl Serializer {
  /// A serializer with no XML declaration and no indentation: the most compact form.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Whether to begin with `<?xml version="1.0" encoding="UTF-8"?>`.
  #[must_use]
  pub fn with_xml_declaration(mut self, on: bool) -> Self {
    self.xml_declaration = on;
    self
  }

  /// Sets the `standalone` pseudo-attribute of the XML declaration. Only appears when the
  /// declaration itself is enabled.
  #[must_use]
  pub fn with_standalone(mut self, standalone: Option<bool>) -> Self {
    self.standalone = standalone;
    self
  }

  /// Pretty-prints with `unit` as one level of indentation (for example `"  "` or `"\t"`).
  ///
  /// Only element content is indented: an element that holds character data is written on one
  /// line, so no whitespace is added inside text.
  #[must_use]
  pub fn with_indent(mut self, unit: &str) -> Self {
    self.indent = Some(unit.to_owned());
    self
  }

  /// Serializes the subtree rooted at `node` to a `String`.
  #[must_use]
  pub fn to_string(&self, doc: &Document, node: NodeId) -> String {
    let mut writer = Writer { doc, indent: self.indent.as_deref(), out: String::new(), scope: Vec::new() };
    if self.xml_declaration {
      writer.write_declaration(self.standalone);
    }
    writer.write_node(node, 0);
    writer.out
  }

  /// Serializes the subtree rooted at `node` to a writer, as UTF-8.
  ///
  /// # Errors
  ///
  /// Propagates any error from `writer`.
  pub fn write<W: io::Write>(&self, mut writer: W, doc: &Document, node: NodeId) -> io::Result<()> {
    writer.write_all(self.to_string(doc, node).as_bytes())
  }
}

/// A namespace binding in scope during serialization: a prefix and the namespace it names, where
/// an empty namespace is an undeclaration.
struct Binding {
  prefix: Option<String>,
  namespace: String,
}

/// Carries the state of one serialization: the growing output and the namespace scope.
struct Writer<'a> {
  doc: &'a Document,
  indent: Option<&'a str>,
  out: String,
  scope: Vec<Binding>,
}

impl Writer<'_> {
  fn write_declaration(&mut self, standalone: Option<bool>) {
    self.out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"");
    if let Some(standalone) = standalone {
      self.out.push_str(if standalone { " standalone=\"yes\"" } else { " standalone=\"no\"" });
    }
    self.out.push_str("?>");
    if self.indent.is_some() {
      self.out.push('\n');
    }
  }

  fn write_node(&mut self, node: NodeId, depth: usize) {
    match self.doc.node_type(node) {
      NodeType::DOCUMENT_NODE | NodeType::DOCUMENT_FRAGMENT_NODE => self.write_children(node, depth),
      NodeType::ELEMENT_NODE => self.write_element(node, depth),
      NodeType::TEXT_NODE => push_text(&mut self.out, self.doc.node_value(node).unwrap_or_default()),
      NodeType::CDATA_SECTION_NODE => push_cdata(&mut self.out, self.doc.node_value(node).unwrap_or_default()),
      NodeType::COMMENT_NODE => {
        self.out.push_str("<!--");
        self.out.push_str(self.doc.node_value(node).unwrap_or_default());
        self.out.push_str("-->");
      }
      NodeType::PROCESSING_INSTRUCTION_NODE => {
        self.out.push_str("<?");
        self.out.push_str(&self.doc.node_name(node));
        let data = self.doc.node_value(node).unwrap_or_default();
        if !data.is_empty() {
          self.out.push(' ');
          self.out.push_str(data);
        }
        self.out.push_str("?>");
      }
      NodeType::DOCUMENT_TYPE_NODE => self.write_doctype(node),
      NodeType::ATTRIBUTE_NODE => {}
    }
  }

  fn write_doctype(&mut self, node: NodeId) {
    self.out.push_str("<!DOCTYPE ");
    self.out.push_str(&self.doc.node_name(node));
    match (self.doc.public_id(node), self.doc.system_id(node)) {
      (Some(public), Some(system)) => {
        self.out.push_str(" PUBLIC \"");
        self.out.push_str(public);
        self.out.push_str("\" \"");
        self.out.push_str(system);
        self.out.push('"');
      }
      (None, Some(system)) => {
        self.out.push_str(" SYSTEM \"");
        self.out.push_str(system);
        self.out.push('"');
      }
      _ => {}
    }
    self.out.push('>');
  }

  fn write_element(&mut self, node: NodeId, depth: usize) {
    let mark = self.scope.len();
    let attributes: Vec<NodeId> = self.doc.attributes(node).iter().collect();

    // Existing namespace declarations on this element go into scope first, so repair does not
    // duplicate one the tree already carries.
    for &attr in &attributes {
      if let Some((prefix, namespace)) = self.namespace_declaration(attr) {
        self.scope.push(Binding { prefix, namespace });
      }
    }

    // Repair: make sure the element's own name, and each namespaced attribute, has a
    // declaration in scope. New ones are collected to write onto this start tag.
    let mut repairs: Vec<Binding> = Vec::new();
    let element_prefix = self.doc.prefix(node).map(ToOwned::to_owned);
    let element_ns = self.doc.namespace_uri(node).map(ToOwned::to_owned);
    self.ensure_binding(element_prefix.as_deref(), element_ns.as_deref(), &mut repairs);
    for &attr in &attributes {
      if self.namespace_declaration(attr).is_some() {
        continue;
      }
      if let Some(namespace) = self.doc.namespace_uri(attr) {
        let namespace = namespace.to_owned();
        let prefix = self.doc.prefix(attr).map(ToOwned::to_owned);
        self.ensure_binding(prefix.as_deref(), Some(&namespace), &mut repairs);
      }
    }

    self.out.push('<');
    self.out.push_str(&self.doc.node_name(node));
    for &attr in &attributes {
      self.write_attribute(attr);
    }
    for repair in &repairs {
      self.out.push_str(" xmlns");
      if let Some(prefix) = &repair.prefix {
        self.out.push(':');
        self.out.push_str(prefix);
      }
      self.out.push_str("=\"");
      push_attribute(&mut self.out, &repair.namespace);
      self.out.push('"');
    }

    let children: Vec<NodeId> = self.doc.children(node).collect();
    if children.is_empty() {
      self.out.push_str("/>");
    } else {
      self.out.push('>');
      self.write_child_list(&children, depth);
      self.out.push_str("</");
      self.out.push_str(&self.doc.node_name(node));
      self.out.push('>');
    }

    self.scope.truncate(mark);
  }

  fn write_attribute(&mut self, attr: NodeId) {
    self.out.push(' ');
    self.out.push_str(&self.doc.node_name(attr));
    self.out.push_str("=\"");
    push_attribute(&mut self.out, self.doc.node_value(attr).unwrap_or_default());
    self.out.push('"');
  }

  /// Writes the children of an element, indenting element content but leaving text content inline.
  fn write_child_list(&mut self, children: &[NodeId], depth: usize) {
    let inline = self.indent.is_none() || children.iter().any(|&c| self.is_character_data(c));
    if inline {
      for &child in children {
        self.write_node(child, depth);
      }
    } else {
      for &child in children {
        self.newline_indent(depth + 1);
        self.write_node(child, depth + 1);
      }
      self.newline_indent(depth);
    }
  }

  fn write_children(&mut self, node: NodeId, depth: usize) {
    let children: Vec<NodeId> = self.doc.children(node).collect();
    let mut first = true;
    for child in children {
      if self.indent.is_some() && !first {
        self.out.push('\n');
      }
      first = false;
      self.write_node(child, depth);
    }
  }

  fn newline_indent(&mut self, depth: usize) {
    if let Some(unit) = self.indent {
      self.out.push('\n');
      for _ in 0..depth {
        self.out.push_str(unit);
      }
    }
  }

  fn is_character_data(&self, node: NodeId) -> bool {
    matches!(self.doc.node_type(node), NodeType::TEXT_NODE | NodeType::CDATA_SECTION_NODE)
  }

  /// If `attr` is a namespace declaration, the prefix it declares (or `None` for the default
  /// namespace) and the namespace it names.
  fn namespace_declaration(&self, attr: NodeId) -> Option<(Option<String>, String)> {
    if self.doc.namespace_uri(attr) != Some(XMLNS_NS_URI) {
      return None;
    }
    let value = self.doc.node_value(attr).unwrap_or_default().to_owned();
    // `xmlns:p` declares the prefix `p`; a bare `xmlns` declares the default namespace.
    match self.doc.prefix(attr) {
      Some("xmlns") => Some((self.doc.local_name(attr).map(ToOwned::to_owned), value)),
      _ => Some((None, value)),
    }
  }

  /// Ensures a prefix is bound to a namespace, recording a repair declaration if it is not.
  fn ensure_binding(&mut self, prefix: Option<&str>, namespace: Option<&str>, repairs: &mut Vec<Binding>) {
    // The `xml` prefix is bound everywhere and never declared.
    if prefix == Some("xml") {
      return;
    }
    let namespace = namespace.unwrap_or("");
    if self.resolve(prefix) == namespace {
      return;
    }
    // An unprefixed name in no namespace needs no declaration unless a default namespace is in
    // scope, in which case it must be undeclared with xmlns="".
    if prefix.is_none() && namespace.is_empty() && self.resolve(None).is_empty() {
      return;
    }
    let binding = Binding { prefix: prefix.map(ToOwned::to_owned), namespace: namespace.to_owned() };
    self.scope.push(Binding { prefix: binding.prefix.clone(), namespace: binding.namespace.clone() });
    repairs.push(binding);
  }

  /// The namespace a prefix currently resolves to; `""` if it is unbound or undeclared.
  fn resolve(&self, prefix: Option<&str>) -> &str {
    self.scope.iter().rev().find(|b| b.prefix.as_deref() == prefix).map_or("", |b| b.namespace.as_str())
  }
}
