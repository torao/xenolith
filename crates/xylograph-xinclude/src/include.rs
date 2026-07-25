//! The XInclude processor.

use xylograph_core::name::XML_NS_URI;
use xylograph_core::{Error, ErrorKind, uri};
use xylograph_dom::{Document, NodeId, NodeType, build};

use crate::Loader;

/// The XInclude namespace.
const XI_NS: &str = "http://www.w3.org/2001/XInclude";

/// Expands `xi:include` elements in a document, in place.
///
/// See the [crate docs](crate) for the model. Build one with [`new`](Self::new), adjust it, then
/// [`expand`](Self::expand) a document.
#[derive(Clone, Debug)]
pub struct XInclude {
  base_fixup: bool,
  max_depth: usize,
  max_includes: usize,
}

impl Default for XInclude {
  fn default() -> Self {
    Self { base_fixup: true, max_depth: 40, max_includes: 65_536 }
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
    // A sub-resource selector is not handled yet, so such an include cannot be satisfied.
    if doc.attribute(include, "xpointer").is_some() {
      return Err(Fault::Recoverable(xinclude_error("xpointer is not yet supported")));
    }
    let Some(href) = doc.attribute(include, "href").map(ToOwned::to_owned) else {
      return Err(Fault::Fatal(xinclude_error("xi:include without href or a supported xpointer")));
    };
    if href.is_empty() {
      return Err(Fault::Recoverable(xinclude_error(
        "an empty href selects the including document, not yet supported",
      )));
    }

    let target = match doc.base_uri(include) {
      Some(base) => uri::resolve(&base, &href).map_err(Fault::Recoverable)?,
      None => href.clone(),
    };
    if state.chain.iter().any(|open| open == &target) {
      return Err(Fault::Fatal(xinclude_error(&format!("inclusion loop through {target:?}"))));
    }
    let bytes = state.loader.load(&target).map_err(Fault::Recoverable)?;

    if parse == "text" {
      let encoding = doc.attribute(include, "encoding").unwrap_or("UTF-8");
      let text = decode(&bytes, encoding).map_err(Fault::Fatal)?;
      return Ok(vec![doc.create_text_node(&text)]);
    }

    // parse == "xml": a malformed resource is fatal, not a fallback case.
    let mut included = build::parse_with_system_id(&bytes[..], &target).map_err(Fault::Fatal)?;
    let included_root = included.root();
    state.chain.push(target.clone());
    let result = self.process_children(&mut included, included_root, state, depth + 1);
    state.chain.pop();
    result.map_err(Fault::Fatal)?;

    let Some(source_root) = included.document_element() else {
      return Err(Fault::Fatal(xinclude_error(&format!("{target:?} has no document element to include"))));
    };
    let imported = doc.import_node(&included, source_root, true).map_err(dom_fault)?;
    if self.base_fixup && doc.base_uri(include).as_deref() != Some(target.as_str()) {
      self.fix_base(doc, imported, &target);
    }
    Ok(vec![imported])
  }

  /// Records the included element's own base URI as an `xml:base`, unless it already has one.
  fn fix_base(&self, doc: &mut Document, element: NodeId, base: &str) {
    if doc.attribute_ns(element, Some(XML_NS_URI), "base").is_none() {
      let _ = doc.set_attribute_ns(element, Some(XML_NS_URI), "xml:base", base);
    }
  }
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
