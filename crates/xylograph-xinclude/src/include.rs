//! The XInclude processor.

use xylograph_core::name::XML_NS_URI;
use xylograph_core::{Error, ErrorKind, uri};
use xylograph_dom::{Document, NodeId, NodeType, build};

use crate::Loader;
use crate::xpointer;

/// The XInclude namespace.
const XI_NS: &str = "http://www.w3.org/2001/XInclude";

/// Expands `xi:include` elements in a document, in place.
///
/// See the [crate docs](crate) for the model. Build one with [`new`](Self::new), adjust it, then
/// [`expand`](Self::expand) a document.
#[derive(Clone, Debug)]
pub struct XInclude {
  base_fixup: bool,
  language_fixup: bool,
  max_depth: usize,
  max_includes: usize,
}

impl Default for XInclude {
  fn default() -> Self {
    Self { base_fixup: true, language_fixup: true, max_depth: 40, max_includes: 65_536 }
  }
}

impl XInclude {
  /// A processor with the defaults: base URI fixup on, and generous depth and count limits.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Whether to add an `xml:base` to an included element so its base URI is preserved (XInclude
  /// base URI fixup). On by default.
  #[must_use]
  pub fn with_base_fixup(mut self, on: bool) -> Self {
    self.base_fixup = on;
    self
  }

  /// Whether to add an `xml:lang` to an included element so its language is preserved (XInclude
  /// language fixup). On by default.
  #[must_use]
  pub fn with_language_fixup(mut self, on: bool) -> Self {
    self.language_fixup = on;
    self
  }

  /// The greatest depth of nested inclusion allowed before the expansion is refused.
  #[must_use]
  pub fn with_max_depth(mut self, depth: usize) -> Self {
    self.max_depth = depth;
    self
  }

  /// The greatest number of inclusions allowed before the expansion is refused.
  #[must_use]
  pub fn with_max_includes(mut self, includes: usize) -> Self {
    self.max_includes = includes;
    self
  }

  /// Expands every `xi:include` in `doc`, fetching resources through `loader`.
  ///
  /// # Errors
  ///
  /// [`ErrorKind::XInclude`] for an inclusion loop, a failed inclusion with no fallback, or a
  /// misplaced `xi:fallback`; [`ErrorKind::Limit`] if the depth or count bound is passed; and
  /// whatever a resource's own parsing or decoding raises.
  pub fn expand<L: Loader>(&self, doc: &mut Document, loader: &mut L) -> Result<(), Error> {
    let mut state = State { loader, count: 0, chain: Vec::new() };
    let root = doc.root();
    self.process_children(doc, root, &mut state, 0)
  }

  /// Walks the children of `parent`, expanding includes and recursing into other elements.
  fn process_children<L: Loader>(
    &self,
    doc: &mut Document,
    parent: NodeId,
    state: &mut State<'_, L>,
    depth: usize,
  ) -> Result<(), Error> {
    let mut child = doc.first_child(parent);
    while let Some(node) = child {
      let next = doc.next_sibling(node);
      if doc.node_type(node) == NodeType::Element {
        if is_xi(doc, node, "include") {
          self.process_include(doc, node, state, depth)?;
        } else if is_xi(doc, node, "fallback") {
          // A fallback reached here is not a child of an include: XInclude forbids it.
          return Err(xinclude_error("xi:fallback must be a child of xi:include"));
        } else {
          self.process_children(doc, node, state, depth)?;
        }
      }
      child = next;
    }
    Ok(())
  }

  /// Expands one `xi:include`, replacing it with the resource or its fallback.
  fn process_include<L: Loader>(
    &self,
    doc: &mut Document,
    include: NodeId,
    state: &mut State<'_, L>,
    depth: usize,
  ) -> Result<(), Error> {
    if depth >= self.max_depth {
      return Err(Error::new(ErrorKind::Limit, format!("XInclude nested more than {} deep", self.max_depth)));
    }
    state.count += 1;
    if state.count > self.max_includes {
      return Err(Error::new(ErrorKind::Limit, format!("more than {} inclusions", self.max_includes)));
    }

    let parent = doc.parent(include).expect("an xi:include always has a parent");
    match self.acquire(doc, include, state, depth) {
      Ok(nodes) => {
        for node in nodes {
          doc.insert_before(parent, node, Some(include)).map_err(dom_error)?;
        }
        doc.remove_child(parent, include).map_err(dom_error)?;
        Ok(())
      }
      Err(Fault::Fatal(error)) => Err(error),
      Err(Fault::Recoverable(error)) => match fallback(doc, include) {
        Some(fallback) => {
          // The fallback's own content is expanded, then moved in place of the include.
          self.process_children(doc, fallback, state, depth)?;
          for child in doc.children(fallback).collect::<Vec<_>>() {
            doc.insert_before(parent, child, Some(include)).map_err(dom_error)?;
          }
          doc.remove_child(parent, include).map_err(dom_error)?;
          Ok(())
        }
        None => Err(xinclude_error(&format!("inclusion failed and there is no fallback: {error}"))),
      },
    }
  }

  /// Fetches and prepares the nodes an `xi:include` resolves to, or the reason it could not.
  fn acquire<L: Loader>(
    &self,
    doc: &mut Document,
    include: NodeId,
    state: &mut State<'_, L>,
    depth: usize,
  ) -> Result<Vec<NodeId>, Fault> {
    let parse = doc.attribute(include, "parse").unwrap_or("xml").to_owned();
    if parse != "xml" && parse != "text" {
      return Err(Fault::Fatal(xinclude_error(&format!("parse must be \"xml\" or \"text\", not {parse:?}"))));
    }
    let xpointer = doc.attribute(include, "xpointer").map(ToOwned::to_owned);
    let href = doc.attribute(include, "href").map(ToOwned::to_owned).filter(|href| !href.is_empty());

    if parse == "text" {
      if xpointer.is_some() {
        return Err(Fault::Fatal(xinclude_error("xpointer may not be used with parse=\"text\"")));
      }
      let Some(href) = href else {
        return Err(Fault::Fatal(xinclude_error("xi:include with parse=\"text\" needs an href")));
      };
      let target = self.resolve(doc, include, &href)?;
      let bytes = state.loader.load(&target).map_err(Fault::Recoverable)?;
      let encoding = doc.attribute(include, "encoding").unwrap_or("UTF-8");
      let text = decode(&bytes, encoding).map_err(Fault::Fatal)?;
      return Ok(vec![doc.create_text_node(&text)]);
    }

    match href {
      Some(href) => self.acquire_resource(doc, include, &href, xpointer.as_deref(), state, depth),
      None => match xpointer {
        Some(xpointer) => self.acquire_same_document(doc, include, &xpointer, state, depth),
        None => Err(Fault::Fatal(xinclude_error("xi:include needs an href or an xpointer"))),
      },
    }
  }

  /// Includes a fetched resource, optionally narrowed to the element an `xpointer` selects.
  fn acquire_resource<L: Loader>(
    &self,
    doc: &mut Document,
    include: NodeId,
    href: &str,
    xpointer: Option<&str>,
    state: &mut State<'_, L>,
    depth: usize,
  ) -> Result<Vec<NodeId>, Fault> {
    let target = self.resolve(doc, include, href)?;
    if state.chain.iter().any(|open| open == &target) {
      return Err(Fault::Fatal(xinclude_error(&format!("inclusion loop through {target:?}"))));
    }
    let bytes = state.loader.load(&target).map_err(Fault::Recoverable)?;

    // A malformed resource is fatal, not a fallback case.
    let mut included = build::parse_with_system_id(&bytes[..], &target).map_err(Fault::Fatal)?;
    let included_root = included.root();
    state.chain.push(target.clone());
    let result = self.process_children(&mut included, included_root, state, depth + 1);
    state.chain.pop();
    result.map_err(Fault::Fatal)?;

    let source = self.select(&included, xpointer, included.document_element())?;
    let source_base = included.base_uri(source);
    let source_language = effective_language(&included, source);
    let imported = doc.import_node(&included, source, true).map_err(dom_fault)?;
    self.fix_base(doc, include, imported, source_base.as_deref());
    self.fix_language(doc, include, imported, source_language.as_deref());
    Ok(vec![imported])
  }

  /// Includes part of the document that contains the `xi:include` (an `xpointer` with no href).
  fn acquire_same_document<L: Loader>(
    &self,
    doc: &mut Document,
    include: NodeId,
    xpointer: &str,
    state: &mut State<'_, L>,
    depth: usize,
  ) -> Result<Vec<NodeId>, Fault> {
    // A synthetic key guards against a same-document include selecting a region that holds it.
    let base = doc.base_uri(include).unwrap_or_default();
    let key = format!("{base}#{xpointer}");
    if state.chain.iter().any(|open| open == &key) {
      return Err(Fault::Fatal(xinclude_error(&format!("inclusion loop through {xpointer:?}"))));
    }
    let source = self.select(doc, Some(xpointer), None)?;
    let source_base = doc.base_uri(source);
    let source_language = effective_language(doc, source);
    let clone = doc.clone_node(source, true).map_err(dom_fault)?;
    // Expand any includes inside the copied region.
    state.chain.push(key);
    let result = self.process_children(doc, clone, state, depth + 1);
    state.chain.pop();
    result.map_err(Fault::Fatal)?;
    self.fix_base(doc, include, clone, source_base.as_deref());
    self.fix_language(doc, include, clone, source_language.as_deref());
    Ok(vec![clone])
  }

  /// Resolves an `href` against the base URI in effect at the `xi:include`.
  fn resolve(&self, doc: &Document, include: NodeId, href: &str) -> Result<String, Fault> {
    match doc.base_uri(include) {
      Some(base) => uri::resolve(&base, href).map_err(Fault::Recoverable),
      None => Ok(href.to_owned()),
    }
  }

  /// Selects the element an `xpointer` identifies, or the default when there is none.
  fn select(&self, doc: &Document, xpointer: Option<&str>, default: Option<NodeId>) -> Result<NodeId, Fault> {
    match xpointer {
      Some(xpointer) => xpointer::select(doc, xpointer)
        .ok_or_else(|| Fault::Recoverable(xinclude_error(&format!("xpointer {xpointer:?} selected nothing")))),
      None => default.ok_or_else(|| Fault::Fatal(xinclude_error("the resource has no document element to include"))),
    }
  }

  /// Records an included element's own base URI as an `xml:base`, unless it already has one and
  /// unless it matches the base already in effect where the include sits.
  fn fix_base(&self, doc: &mut Document, include: NodeId, element: NodeId, source_base: Option<&str>) {
    if !self.base_fixup {
      return;
    }
    let Some(source_base) = source_base else { return };
    if doc.base_uri(include).as_deref() == Some(source_base) {
      return;
    }
    if doc.attribute_ns(element, Some(XML_NS_URI), "base").is_none() {
      let _ = doc.set_attribute_ns(element, Some(XML_NS_URI), "xml:base", source_base);
    }
  }

  /// Records an included element's own language as an `xml:lang`, when it differs from the
  /// language already in effect where the include sits and the element does not set it itself.
  fn fix_language(&self, doc: &mut Document, include: NodeId, element: NodeId, source_language: Option<&str>) {
    if !self.language_fixup {
      return;
    }
    // Only an element that actually has a language in scope carries one across.
    let Some(source_language) = source_language else { return };
    if effective_language(doc, include).as_deref() == Some(source_language) {
      return;
    }
    if doc.attribute_ns(element, Some(XML_NS_URI), "lang").is_none() {
      let _ = doc.set_attribute_ns(element, Some(XML_NS_URI), "xml:lang", source_language);
    }
  }
}

/// The language in effect at a node: the value of the nearest `xml:lang` at or above it, if any.
fn effective_language(doc: &Document, node: NodeId) -> Option<String> {
  let mut current = Some(node);
  while let Some(node) = current {
    if doc.node_type(node) == NodeType::Element {
      if let Some(language) = doc.attribute_ns(node, Some(XML_NS_URI), "lang") {
        return Some(language.to_owned());
      }
    }
    current = doc.parent(node);
  }
  None
}

/// The state threaded through an expansion: the loader, a running count, and the chain of
/// resources currently open, for loop detection.
struct State<'l, L> {
  loader: &'l mut L,
  count: usize,
  chain: Vec<String>,
}

/// Why an inclusion could not be performed.
enum Fault {
  /// A resource error: a fallback may be used instead.
  Recoverable(Error),
  /// An unrecoverable error: it stops the whole expansion.
  Fatal(Error),
}

/// Whether `node` is the XInclude element with local name `local`.
fn is_xi(doc: &Document, node: NodeId, local: &str) -> bool {
  doc.namespace_uri(node) == Some(XI_NS) && doc.local_name(node) == Some(local)
}

/// The `xi:fallback` child of an include, if it has one.
fn fallback(doc: &Document, include: NodeId) -> Option<NodeId> {
  doc.children(include).find(|&child| is_xi(doc, child, "fallback"))
}

/// Decodes bytes to text for `parse="text"`.
fn decode(bytes: &[u8], encoding: &str) -> Result<String, Error> {
  let mut decoder = xylograph_core::encoding::decoder_for(encoding)?;
  let mut text = String::new();
  decoder.decode(bytes, &mut text, true)?;
  Ok(text)
}

fn xinclude_error(message: &str) -> Error {
  Error::new(ErrorKind::XInclude, message.to_owned())
}

fn dom_error(error: xylograph_dom::DomException) -> Error {
  Error::internal(format!("XInclude tree edit: {error}"))
}

fn dom_fault(error: xylograph_dom::DomException) -> Fault {
  Fault::Fatal(dom_error(error))
}
