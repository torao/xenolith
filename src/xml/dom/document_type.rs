use std::collections::{HashMap, HashSet};

use crate::xml::w3c::dom as W3C;
use super::*;

#[derive(Clone, PartialEq, Eq)]
pub struct DocumentType {
  qualified_name: String,
  public_id: String,
  system_id: String,
  entities: NamedNodeMap,
  notations: NamedNodeMap,
  internal_subset: Option<String>
}

impl W3C::DocumentType<DOMImplementation> for DocumentType {
  fn name(&self) -> &str {
    &self.qualified_name
  }
  fn entities(&self) -> &NamedNodeMap {
    &self.entities
  }
  fn notations(&self) -> &NamedNodeMap {
    &self.notations
  }
  fn public_id(&self) -> &str {
    &self.public_id
  }
  fn system_id(&self) -> &str {
    &self.system_id
  }
  fn internal_subset(&self) -> Option<&str> {
    self.internal_subset
  }

  fn clone_document_type(&self, deep: bool) -> Self {
    self.clone()
  }
}

impl W3C::Node<DOMImplementation> for DocumentType{
    fn attributes(&self) -> Option<<DOMImplementation as W3C::DOMImplementation>::NamedNodeMap> {
        todo!()
    }

    fn base_uri(&self) -> Option<String> {
        todo!()
    }

    fn child_nodes(&self) -> &<DOMImplementation as W3C::DOMImplementation>::NodeList {
        todo!()
    }

    fn first_child(&self) -> Option<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn last_child(&self) -> Option<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn local_name(&self) -> Option<&str> {
        todo!()
    }

    fn namespace_uri(&self) -> Option<&str> {
        todo!()
    }

    fn next_sibling(&self) -> Option<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn node_name(&self) -> &str {
        todo!()
    }

    fn node_type(&self) -> W3C::NodeType {
        todo!()
    }

    fn node_value(&self) -> W3C::Result<Option<&str>> {
        todo!()
    }

    fn set_node_value(&mut self, value: &str) -> W3C::Result<()> {
        todo!()
    }

    fn owner_document(&self) -> Option<&<DOMImplementation as W3C::DOMImplementation>::Document> {
        todo!()
    }

    fn parent_node(&self) -> Option<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn prefix(&self) -> Option<String> {
        todo!()
    }

    fn set_prefix(&self, prefix: Option<String>) -> W3C::Result<()> {
        todo!()
    }

    fn previous_sibling(&self) -> Option<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn text_content(&self) -> Option<String> {
      None
    }

    fn set_text_content(&self, text_content: &str) -> W3C::Result<()> {
        todo!()
    }

    fn append_child(&mut self, new_child: W3C::NodeRef<DOMImplementation>) -> W3C::Result<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn compare_document_position(&self, other: &W3C::NodeRef<DOMImplementation>) -> W3C::DocumentPosition {
        todo!()
    }

    fn get_feature(&self, feature: &str, version: Option<&str>) -> Option<W3C::DOMObject> {
        todo!()
    }

    fn get_user_data(&self, key: &str) -> Option<W3C::DOMUserData> {
        todo!()
    }

    fn has_attributes(&self) -> bool {
        todo!()
    }

    fn has_child_nodes(&self) -> bool {
        todo!()
    }

    fn insert_before(&mut self, new_child: W3C::NodeRef<DOMImplementation>, ref_child: W3C::NodeRef<DOMImplementation>) -> W3C::Result<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn is_default_namespace(&self, namespace_uri: &str) -> bool {
        todo!()
    }

    fn is_equal_node(&self, arg: &W3C::NodeRef<DOMImplementation>) -> bool {
        todo!()
    }

    fn is_same_node(&self, other: &W3C::NodeRef<DOMImplementation>) -> bool {
        todo!()
    }

    fn is_supported(&self, feature: &str, version: &str) -> bool {
        todo!()
    }

    fn lookup_namespace_uri(&self, prefix: &str) -> Option<&str> {
        todo!()
    }

    fn lookup_prefix(&self, namespace_uri: &str) -> Option<&str> {
        todo!()
    }

    fn normalize(&mut self) {
        todo!()
    }

    fn remove_child(&mut self, old_child: &W3C::NodeRef<DOMImplementation>) -> W3C::Result<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn replace_child(&mut self, new_child: W3C::NodeRef<DOMImplementation>, old_child: W3C::NodeRef<DOMImplementation>) -> W3C::Result<W3C::NodeRef<DOMImplementation>> {
        todo!()
    }

    fn set_user_data(
    &mut self, key: &str, data: W3C::DOMUserData, handler: Box<dyn W3C::UserDataHandler<DOMImplementation>>,
  ) -> Option<W3C::DOMUserData> {
        todo!()
    }
}