//! Writing a result tree out the way `xsl:output` asks (XSLT 1.0 §16).
//!
//! A result tree is a tree; what a caller wants is usually bytes. §16 is the stylesheet's say in
//! how that is done: which of the three methods, whether to write an XML declaration, what
//! doctype to put in front, which elements' text to wrap in CDATA, whether to indent.
//!
//! The XML method is XML. The HTML method is not — an empty element is written `<br>` rather
//! than `<br/>`, and a `script` or `style` holds text that must not be escaped, because an HTML
//! parser will not unescape it. The text method writes the characters and no markup at all.
//!
//! # Specifications
//!
//! - [Output (§16)], the [XML output method (§16.1)], the [HTML output method (§16.2)] and the
//!   [text output method (§16.3)]
//! - [Disabling output escaping (§16.4)]
//!
//! [Output (§16)]: https://www.w3.org/TR/1999/REC-xslt-19991116#output
//! [XML output method (§16.1)]: https://www.w3.org/TR/1999/REC-xslt-19991116#section-XML-Output-Method
//! [HTML output method (§16.2)]: https://www.w3.org/TR/1999/REC-xslt-19991116#section-HTML-Output-Method
//! [text output method (§16.3)]: https://www.w3.org/TR/1999/REC-xslt-19991116#section-Text-Output-Method
//! [Disabling output escaping (§16.4)]: https://www.w3.org/TR/1999/REC-xslt-19991116#disable-output-escaping

use std::collections::HashSet;

use xylograph_core::error::{Error, ErrorKind, Result};
use xylograph_dom::{Document, NodeId, NodeType};

use crate::stylesheet::OutputMethod;

/// The elements the HTML method writes without an end tag (§16.2).
const HTML_EMPTY: &[&str] =
  &["area", "base", "basefont", "br", "col", "frame", "hr", "img", "input", "isindex", "link", "meta", "param"];

/// The elements whose text the HTML method must not escape (§16.2).
const HTML_UNESCAPED: &[&str] = &["script", "style"];

/// What `xsl:output` asked for, with every declaration merged (XSLT 1.0 §16).
///
/// §16 merges the `xsl:output` declarations of a stylesheet attribute by attribute rather than
/// choosing one of them, so a module may set the encoding and an imported one the indentation.
/// Where two set the same attribute the higher import precedence wins.
#[derive(Clone, Debug, Default)]
pub struct Output {
  pub(crate) method: OutputMethod,
  pub(crate) version: Option<String>,
  pub(crate) encoding: Option<String>,
  pub(crate) omit_xml_declaration: bool,
  pub(crate) standalone: Option<bool>,
  pub(crate) doctype_public: Option<String>,
  pub(crate) doctype_system: Option<String>,
  pub(crate) cdata_section_elements: Vec<(Option<String>, String)>,
  pub(crate) indent: bool,
  pub(crate) media_type: Option<String>,
}

impl Output {
  /// The method the result is written by.
  #[must_use]
  pub const fn method(&self) -> OutputMethod {
    self.method
  }

  /// The `version` for the XML declaration, if one was asked for.
  #[must_use]
  pub fn version(&self) -> Option<&str> {
    self.version.as_deref()
  }

  /// The encoding the result is to be written in, if one was asked for.
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.encoding.as_deref()
  }

  /// Whether the XML declaration is left out.
  #[must_use]
  pub const fn omit_xml_declaration(&self) -> bool {
    self.omit_xml_declaration
  }

  /// What `standalone` was set to, if it was.
  #[must_use]
  pub const fn standalone(&self) -> Option<bool> {
    self.standalone
  }

  /// The public identifier of the document type declaration, if one was asked for.
  #[must_use]
  pub fn doctype_public(&self) -> Option<&str> {
    self.doctype_public.as_deref()
  }

  /// The system identifier of the document type declaration, if one was asked for.
  #[must_use]
  pub fn doctype_system(&self) -> Option<&str> {
    self.doctype_system.as_deref()
  }

  /// Whether the result is indented.
  #[must_use]
  pub const fn indent(&self) -> bool {
    self.indent
  }

  /// The media type given for the result, if one was.
  #[must_use]
  pub fn media_type(&self) -> Option<&str> {
    self.media_type.as_deref()
  }

  /// Whether an element's text is to be written in a CDATA section.
  fn is_cdata_section(&self, namespace: Option<&str>, local: &str) -> bool {
    self
      .cdata_section_elements
      .iter()
      .any(|(wanted_namespace, wanted_local)| wanted_namespace.as_deref() == namespace && wanted_local == local)
  }
}

/// Writes a result tree as `output` asks.
pub(crate) struct Writer<'a> {
  document: &'a Document,
  output: &'a Output,
  /// The text nodes written with `disable-output-escaping`, which go out as they stand.
  raw: &'a HashSet<NodeId>,
  written: String,
  /// The prefix bindings in scope, innermost last, with `None` for the default namespace.
  ///
  /// A result tree carries names — a namespace URI and a local part — and not the declarations
  /// that would let those names be written down (see `engine.rs`, where a copy keeps its name
  /// and drops the declarations). Writing them is this writer's job, and this is what it needs
  /// to know which are already said.
  in_scope: Vec<(Option<String>, String)>,
  /// How many prefixes have had to be invented, so that each gets a name of its own.
  invented: usize,
}

impl<'a> Writer<'a> {
  pub(crate) fn new(document: &'a Document, output: &'a Output, raw: &'a HashSet<NodeId>) -> Self {
    Self { document, output, raw, written: String::new(), in_scope: Vec::new(), invented: 0 }
  }

  /// Writes everything below `root`.
  pub(crate) fn write(mut self, root: NodeId) -> String {
    if self.output.method == OutputMethod::Text {
      // §16.3: the character data of the result, and nothing else.
      self.written = self.document.text_content(root);
      return self.written;
    }
    self.prologue(root);
    let children: Vec<NodeId> = self.document.children(root).collect();
    for child in children {
      self.node(child, 0);
    }
    self.written
  }

  /// The XML declaration and document type declaration, as far as they were asked for.
  fn prologue(&mut self, root: NodeId) {
    // §16.1: the XML method writes a declaration unless told not to; §16.2 says the HTML method
    // does not write one at all.
    if self.output.method == OutputMethod::Xml && !self.output.omit_xml_declaration {
      let version = self.output.version.as_deref().unwrap_or("1.0");
      let encoding = self.output.encoding.as_deref().unwrap_or("UTF-8");
      self.written.push_str(&format!("<?xml version=\"{version}\" encoding=\"{encoding}\""));
      if let Some(standalone) = self.output.standalone {
        let value = if standalone { "yes" } else { "no" };
        self.written.push_str(&format!(" standalone=\"{value}\""));
      }
      self.written.push_str("?>");
      if self.output.indent {
        self.written.push('\n');
      }
    }
    self.doctype(root);
  }

  /// The document type declaration, whose name is that of the first element written.
  fn doctype(&mut self, root: NodeId) {
    if self.output.doctype_system.is_none() && self.output.doctype_public.is_none() {
      return;
    }
    // §16.1 takes the name from the document element; a result with none has nothing to declare.
    let Some(name) = self.first_element_name(root) else { return };
    self.written.push_str(&format!("<!DOCTYPE {name}"));
    match (&self.output.doctype_public, &self.output.doctype_system) {
      (Some(public), Some(system)) => {
        self.written.push_str(&format!(" PUBLIC \"{public}\" \"{system}\""));
      }
      // §16.1: a public identifier with no system identifier is written as though it were the
      // system one, since a declaration needs somewhere to point.
      (Some(public), None) => self.written.push_str(&format!(" PUBLIC \"{public}\"")),
      (None, Some(system)) => self.written.push_str(&format!(" SYSTEM \"{system}\"")),
      (None, None) => {}
    }
    self.written.push('>');
    if self.output.indent {
      self.written.push('\n');
    }
  }

  /// The name of the first element of the result, which the doctype declares.
  fn first_element_name(&self, root: NodeId) -> Option<String> {
    self
      .document
      .children(root)
      .find(|&child| self.document.node_type(child) == NodeType::Element)
      .map(|element| self.document.node_name(element))
  }

  /// Writes one node and everything below it.
  fn node(&mut self, node: NodeId, depth: usize) {
    match self.document.node_type(node) {
      NodeType::Element => self.element(node, depth),
      NodeType::Text | NodeType::CdataSection => self.text(node),
      NodeType::Comment => {
        let data = self.document.node_value(node).unwrap_or_default().to_owned();
        self.written.push_str(&format!("<!--{data}-->"));
      }
      NodeType::ProcessingInstruction => {
        let target = self.document.node_name(node);
        let data = self.document.node_value(node).unwrap_or_default().to_owned();
        // §16.2: the HTML method ends a processing instruction with `>` rather than `?>`.
        let close = if self.output.method == OutputMethod::Html { ">" } else { "?>" };
        if data.is_empty() {
          self.written.push_str(&format!("<?{target}{close}"));
        } else {
          self.written.push_str(&format!("<?{target} {data}{close}"));
        }
      }
      _ => {}
    }
  }

  /// What a prefix stands for here, or `None` if nothing in scope binds it.
  ///
  /// An empty URI is a binding: it is how a default namespace is cancelled.
  fn bound(&self, prefix: Option<&str>) -> Option<&str> {
    self.in_scope.iter().rev().find(|(bound, _)| bound.as_deref() == prefix).map(|(_, namespace)| namespace.as_str())
  }

  /// A prefix for a namespace that has to have one: whichever is already bound to it, or a new
  /// one that nothing else is using.
  fn prefix_for(&mut self, namespace: &str) -> String {
    if let Some((Some(prefix), _)) =
      self.in_scope.iter().rev().find(|(prefix, bound)| prefix.is_some() && bound == namespace)
    {
      return prefix.clone();
    }
    loop {
      self.invented += 1;
      let prefix = format!("ns{}", self.invented);
      if self.bound(Some(&prefix)).is_none() {
        return prefix;
      }
    }
  }

  /// The name to write for an element or attribute, declaring the prefix it needs.
  ///
  /// The result tree holds expanded names; XML can only write a name through a prefix that is
  /// declared. So anything whose namespace is not already said here is said here — which is what
  /// makes a result carrying namespaces well-formed rather than merely nearly so.
  fn qualified(&mut self, node: NodeId, is_element: bool, declarations: &mut Vec<(String, String)>) -> String {
    let local = self.document.local_name(node).unwrap_or_default().to_owned();
    let prefix = self.document.prefix(node).map(ToOwned::to_owned);
    let Some(namespace) = self.document.namespace_uri(node).map(ToOwned::to_owned) else {
      // A name in no namespace. An unprefixed *element* would otherwise be read as being in
      // whatever default namespace is in scope, so that declaration is cancelled here;
      // an unprefixed attribute is in no namespace whatever is declared, so it needs nothing.
      if is_element && prefix.is_none() && self.bound(None).is_some_and(|namespace| !namespace.is_empty()) {
        declarations.push(("xmlns".to_owned(), String::new()));
        self.in_scope.push((None, String::new()));
      }
      return local;
    };
    // Namespaces §6.2: an unprefixed attribute name is in no namespace, whatever is declared. So
    // an attribute that is in one must be written through a prefix, invented if it has none.
    let prefix = match prefix {
      Some(prefix) => Some(prefix),
      None if is_element => None,
      None => Some(self.prefix_for(&namespace)),
    };
    if self.bound(prefix.as_deref()) != Some(namespace.as_str()) {
      let declaration = prefix.as_ref().map_or_else(|| "xmlns".to_owned(), |prefix| format!("xmlns:{prefix}"));
      declarations.push((declaration, namespace.clone()));
      self.in_scope.push((prefix.clone(), namespace));
    }
    prefix.map_or(local.clone(), |prefix| format!("{prefix}:{local}"))
  }

  /// Whether an attribute of the tree is itself a namespace declaration.
  fn is_declaration(name: &str) -> bool {
    name == "xmlns" || name.starts_with("xmlns:")
  }

  fn element(&mut self, node: NodeId, depth: usize) {
    let children: Vec<NodeId> = self.document.children(node).collect();
    let attributes: Vec<NodeId> = self.document.attributes(node).iter().collect();
    let outer = self.in_scope.len();

    // A declaration the tree already carries stays as it is, and is in scope for the names
    // below — so nothing is declared twice.
    let mut written: Vec<(String, String)> = Vec::new();
    for &attribute in &attributes {
      let name = self.document.node_name(attribute);
      if Self::is_declaration(&name) {
        let value = self.document.node_value(attribute).unwrap_or_default().to_owned();
        self.in_scope.push((name.strip_prefix("xmlns:").map(ToOwned::to_owned), value.clone()));
        written.push((name, value));
      }
    }

    let mut declarations: Vec<(String, String)> = Vec::new();
    let name = self.qualified(node, true, &mut declarations);
    for &attribute in &attributes {
      if Self::is_declaration(&self.document.node_name(attribute)) {
        continue;
      }
      let attribute_name = self.qualified(attribute, false, &mut declarations);
      let value = self.document.node_value(attribute).unwrap_or_default().to_owned();
      written.push((attribute_name, value));
    }
    let attributes = declarations.into_iter().chain(written).collect::<Vec<_>>();

    // Nothing is written before the very first thing, and nothing after a newline already put
    // there, so the result never begins with blank space it was not asked for.
    let opening = self.output.indent
      && self.among_elements(self.document.parent(node))
      && !self.written.is_empty()
      && !self.written.ends_with('\n');
    if opening {
      self.written.push('\n');
      for _ in 0..depth {
        self.written.push_str("  ");
      }
    }

    self.written.push('<');
    self.written.push_str(&name);
    for (attribute_name, value) in attributes {
      self.written.push(' ');
      self.written.push_str(&attribute_name);
      self.written.push_str("=\"");
      self.push_attribute(&value);
      self.written.push('"');
    }

    let html = self.output.method == OutputMethod::Html;
    let local = self.document.local_name(node).unwrap_or_default().to_lowercase();
    if html && HTML_EMPTY.contains(&local.as_str()) {
      // §16.2: such an element is written with no end tag and no self-closing slash, which is
      // what an HTML parser expects.
      self.written.push('>');
      self.in_scope.truncate(outer);
      return;
    }
    if children.is_empty() && !html {
      self.written.push_str("/>");
      self.in_scope.truncate(outer);
      return;
    }
    self.written.push('>');

    let unescaped = html && HTML_UNESCAPED.contains(&local.as_str());
    let cdata = self.output.is_cdata_section(self.document.namespace_uri(node), &local);
    for child in children {
      if unescaped && matches!(self.document.node_type(child), NodeType::Text | NodeType::CdataSection) {
        let text = self.document.node_value(child).unwrap_or_default().to_owned();
        self.written.push_str(&text);
        continue;
      }
      if cdata && matches!(self.document.node_type(child), NodeType::Text | NodeType::CdataSection) {
        let text = self.document.node_value(child).unwrap_or_default().to_owned();
        // A `]]>` inside would end the section early, so the section is split around it.
        self.written.push_str("<![CDATA[");
        self.written.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
        self.written.push_str("]]>");
        continue;
      }
      self.node(child, depth + 1);
    }

    if self.output.indent && self.among_elements(Some(node)) {
      self.written.push('\n');
      for _ in 0..depth {
        self.written.push_str("  ");
      }
    }
    self.written.push_str("</");
    self.written.push_str(&name);
    self.written.push('>');
    // What this element declared goes out of scope with it.
    self.in_scope.truncate(outer);
  }

  /// Whether an element's children are all elements, so whitespace may go among them.
  ///
  /// §16 lets the XML and HTML methods add whitespace where it is not significant, and the only
  /// place that is safe is among elements: a newline put beside text would become part of that
  /// text and change what the result says.
  fn among_elements(&self, parent: Option<NodeId>) -> bool {
    let Some(parent) = parent else { return false };
    let mut children = self.document.children(parent).peekable();
    if children.peek().is_none() {
      return false;
    }
    children.all(|child| self.document.node_type(child) == NodeType::Element)
  }

  fn text(&mut self, node: NodeId) {
    let text = self.document.node_value(node).unwrap_or_default().to_owned();
    // §16.4: text written with disable-output-escaping goes out as it stands, markup and all.
    if self.raw.contains(&node) {
      self.written.push_str(&text);
      return;
    }
    self.push_text(&text);
  }

  fn push_text(&mut self, text: &str) {
    for character in text.chars() {
      match character {
        '<' => self.written.push_str("&lt;"),
        '>' => self.written.push_str("&gt;"),
        '&' => self.written.push_str("&amp;"),
        other => self.written.push(other),
      }
    }
  }

  fn push_attribute(&mut self, value: &str) {
    for character in value.chars() {
      match character {
        '<' => self.written.push_str("&lt;"),
        '>' => self.written.push_str("&gt;"),
        '&' => self.written.push_str("&amp;"),
        '"' => self.written.push_str("&quot;"),
        '\n' => self.written.push_str("&#10;"),
        '\t' => self.written.push_str("&#9;"),
        '\r' => self.written.push_str("&#13;"),
        other => self.written.push(other),
      }
    }
  }
}

/// Turns written text into the bytes an `encoding` asks for.
///
/// UTF-8 needs nothing. Anything else needs the `encodings` feature, and without it this is an
/// error naming the feature rather than bytes in the wrong encoding under a declaration that
/// says otherwise — the same answer the parser gives on the way in.
pub(crate) fn encode(written: &str, encoding: Option<&str>) -> Result<Vec<u8>> {
  let Some(label) = encoding else { return Ok(written.as_bytes().to_vec()) };
  if label.eq_ignore_ascii_case("utf-8") || label.eq_ignore_ascii_case("utf8") {
    return Ok(written.as_bytes().to_vec());
  }
  transcode(written, label)
}

/// Writes text in an encoding other than UTF-8.
#[cfg(feature = "encodings")]
fn transcode(written: &str, label: &str) -> Result<Vec<u8>> {
  let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) else {
    return Err(Error::new(ErrorKind::Xslt, format!("no encoding is named {label:?}")));
  };
  // A character the encoding cannot hold becomes a character reference, which is what §16 asks
  // for and is still readable back.
  let (bytes, _, _) = encoding.encode(written);
  Ok(bytes.into_owned())
}

/// Refuses an encoding other than UTF-8, naming the feature that would provide it.
#[cfg(not(feature = "encodings"))]
fn transcode(written: &str, label: &str) -> Result<Vec<u8>> {
  let _ = written;
  let message =
    format!("writing the result in {label:?} needs the `encodings` feature; without it only UTF-8 can be written");
  Err(Error::new(ErrorKind::Xslt, message))
}
