//! This module is a implementation for [crate::w3c::com].
//!
//! ### Exceptions
//!
//! The following exceptions defined in the [W3C Recommendation](https://www.w3.org/TR/DOM-Level-3-Core/core.html#ID-17189187)
//! are not or rarely raised in Rust implementations.
//!
//! - [W3C::DOMException::NoModificationAllowed], because static check is self-explanatory as to whether it is
//!   read-only or not.
//! - [W3C::DOMException::WrongDocument], because DOMImplementation must match on static type checking.
//! - [W3C::DOMException::NotSupported], because this implementation supports namespaces ("XML" feature).
//!
use std::collections::{HashMap, HashSet};

use crate::xml::w3c::dom as W3C;

pub use document::*;
pub use document_type::*;

mod document;
mod document_type;

#[cfg(test)]
mod document_test;

pub struct DOMImplementation {
  features: Features,
}

impl W3C::DOMImplementation for DOMImplementation {
  type Document = Document;
  type DocumentType = DocumentType;
  type Element = Element;
  type NamedNodeMap = NamedNodeMap;
  type NodeList = NodeList;
  type DocumentFragment = DocumentFragment;
  type Text = Text;
  type Comment = Comment;
  type CDATASection = CDATASection;
  type ProcessingInstruction = ProcessingInstruction;
  type Attr = Attr;
  type EntityReference = EntityReference;
  type Notation = Notation;
  type Entity = Entity;

  fn has_feature(&self, feature: &str, version: Option<&str>) -> bool {
    self.features.has_feature(feature, version)
  }

  fn create_document_type(
    &self, qualified_name: &str, public_id: &str, system_id: &str,
  ) -> W3C::Result<Self::DocumentType> {
    todo!()
  }

  fn create_document(&self, namespace_uri: &str, qualified_name: &str, doctype: Self::DocumentType) -> Self::Document {
    todo!()
  }
  fn get_feature(&self, feature: &str, version: Option<&str>) -> Option<W3C::DOMObject> {
    todo!()
  }
}

pub struct NodeList {
  nodes: Vec<NodeRef>,
}

impl NodeList {
  fn new() -> Self {
    NodeList { nodes: Vec::new() }
  }
  fn first(&self) -> Option<&NodeRef> {
    self.nodes.first()
  }
  fn last(&self) -> Option<&NodeRef> {
    self.nodes.last()
  }
}

impl W3C::NodeList<DOMImplementation> for NodeList {
  fn item(&self, index: usize) -> Option<NodeRef> {
    if index < self.nodes.len() {
      Some(self.nodes[index].clone())
    } else {
      None
    }
  }
  fn length(&self) -> usize {
    self.nodes.len()
  }
}

pub struct NamedNodeMap {
  node_type: W3C::NodeType,
  named: HashMap<QName, NodeRef>,
  indexed: Vec<QName>,
}

impl NamedNodeMap {
  fn get(&self, qname: &QName) -> Option<NodeRef> {
    self.named.get(qname).map(|n| n.clone())
  }
  fn remove(&mut self, qname: &QName) -> W3C::Result<NodeRef> {
    match self.named.remove(&qname) {
      None => Err(W3C::DOMException::NotFound),
      Some(node) => {
        for i in 0..self.indexed.len() {
          if &self.indexed[i] == qname {
            self.indexed.remove(i);
            break;
          }
        }
        Ok(node)
      }
    }
  }
  fn set(&mut self, qname: QName, node: NodeRef) -> W3C::Result<Option<NodeRef>> {
    if let W3C::NodeRef::Attr(attr) = &node {
      if node.as_node().parent_node().is_some() {
        return Err(W3C::DOMException::InuseAttribute);
      }
    } else if self.node_type == W3C::NodeType::ElementNode {
      return Err(W3C::DOMException::HierarchyRequest);
    } else {
      // TODO: other combination for DOMException::HierarchyRequest
    }
    let sub = qname.clone();
    let result = self.named.insert(qname, node);
    if result.is_none() {
      self.indexed.push(sub);
    }
    Ok(result)
  }
}

impl W3C::NamedNodeMap<DOMImplementation> for NamedNodeMap {
  fn length(&self) -> usize {
    self.indexed.len()
  }
  fn get_named_item(&self, name: &str) -> Option<NodeRef> {
    self.get(&QName::new(None, Some(name.to_string())))
  }
  fn get_named_item_ns(&self, namespace_uri: &str, local_name: &str) -> W3C::Result<Option<NodeRef>> {
    Ok(self.get(&QName::new(Some(namespace_uri.to_string()), Some(local_name.to_string()))))
  }
  fn item(&self, index: usize) -> Option<NodeRef> {
    self.indexed.get(index).map(|qname| self.get(qname)).flatten()
  }
  fn remove_named_item(&mut self, name: &str) -> W3C::Result<NodeRef> {
    let qname = QName::new(None, Some(name.to_string()));
    self.remove(&qname)
  }
  fn remove_named_item_ns(&mut self, namespace_uri: &str, local_name: &str) -> W3C::Result<NodeRef> {
    let qname = QName::new(Some(namespace_uri.to_string()), Some(local_name.to_string()));
    self.remove(&qname)
  }
  fn set_named_item(&mut self, arg: NodeRef) -> W3C::Result<Option<NodeRef>> {
    let name = arg.as_node().node_name();
    let qname = QName::new(None, Some(name.to_string()));
    self.set(qname, arg)
  }
  fn set_named_item_ns(&mut self, arg: W3C::NodeRef<DOMImplementation>) -> W3C::Result<Option<NodeRef>> {
    let namespace_uri = arg.as_node().namespace_uri().map(|n| n.to_string());
    let local_name = arg.as_node().local_name().map(|n| n.to_string());
    let qname = QName::new(namespace_uri, local_name);
    self.set(qname, arg)
  }
}

pub struct Element {}
pub struct Attr {}
pub struct Text {}
pub struct EntityReference {}
pub struct ProcessingInstruction {}
pub struct DocumentFragment {}
pub struct Comment {}
pub struct CDATASection {}
pub struct Notation {}
pub struct Entity {}

pub type NodeRef = W3C::NodeRef<DOMImplementation>;

#[derive(Hash, Clone, PartialEq, Eq)]
struct QName {
  namespace_uri: Option<String>,
  local_name: Option<String>,
}

impl QName {
  pub fn new(namespace_uri: Option<String>, local_name: Option<String>) -> Self {
    Self { namespace_uri, local_name }
  }
}

struct Features {
  features: HashSet<(String, Option<String>)>,
}

impl Features {
  pub fn new() -> Self {
    Features { features: HashSet::new() }
  }
  pub fn has_feature(&self, feature: &str, version: Option<&str>) -> bool {
    let feature = feature.to_lowercase();
    let version = version.map(|version| if version.is_empty() { None } else { Some(version.to_string()) }).flatten();
    self.features.contains(&(feature, version)) || (version.is_some() && self.features.contains(&(feature, None)))
  }
  pub fn set_feature(&mut self, feature: &str, version: Option<&str>, enabled: bool) {
    let feature = feature.to_lowercase();
    let version = version.map(|version| if version.is_empty() { None } else { Some(version.to_string()) }).flatten();
    if enabled {
      self.features.insert((feature, version));
    } else {
      self.features.remove(&(feature, version));
    }
  }
}
