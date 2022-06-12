use crate::xml::w3c::dom::{self as W3C, DOMObject, DOMUserData};

use super::{
  Attr, CDATASection, Comment, DOMImplementation as IMPL, DocumentFragment, DocumentType, Element, EntityReference,
  NamedNodeMap, NodeList, NodeRef, ProcessingInstruction, Text,
};

pub struct Document {
  base_uri: Option<String>,
  child_nodes: NodeList,
}

impl W3C::Node<IMPL> for Document {
  fn attributes(&self) -> Option<NamedNodeMap> {
    None
  }
  fn base_uri(&self) -> Option<String> {
    self.base_uri
  }
  fn child_nodes(&self) -> &NodeList {
    &self.child_nodes
  }
  fn first_child(&self) -> Option<NodeRef> {
    self.child_nodes.first().map(|n| n.clone())
  }
  fn last_child(&self) -> Option<NodeRef> {
    self.child_nodes.last().map(|n| n.clone())
  }
  fn local_name(&self) -> Option<&str> {
    None
  }
  fn namespace_uri(&self) -> Option<&str> {
    None
  }
  fn next_sibling(&self) -> Option<NodeRef> {
    None
  }
  fn node_name(&self) -> &str {
    "#document"
  }
  fn node_type(&self) -> W3C::NodeType {
    W3C::NodeType::DocumentNode
  }
  fn node_value(&self) -> W3C::Result<Option<&str>> {
    Ok(None)
  }
  fn set_node_value(&mut self, _value: &str) -> W3C::Result<()> {
    Ok(())
  }
  fn owner_document(&self) -> Option<&Document> {
    // NOTE: valid value is returned by [NodeRef::owned_document()].
    None
  }
  fn parent_node(&self) -> Option<W3C::NodeRef<IMPL>> {
    None
  }
  fn prefix(&self) -> Option<String> {
    None
  }
  fn set_prefix(&self, prefix: Option<String>) -> W3C::Result<()> {
    Err(W3C::DOMException::NoModificationAllowed)
  }
  fn previous_sibling(&self) -> Option<W3C::NodeRef<IMPL>> {
    None
  }
  fn text_content(&self) -> Option<String> {
    None
  }
  fn set_text_content(&self, text_content: &str) -> W3C::Result<()> {
    // noop
    Ok(())
  }

  fn append_child(&mut self, new_child: NodeRef) -> W3C::Result<&NodeRef> {
    self.child_nodes.nodes.push(new_child);
    Ok(self.child_nodes.nodes.last().unwrap())
  }
  fn get_feature(&self, feature: &str, version: Option<&str>) -> Option<DOMObject> {
    todo!()
  }
  fn get_user_data(&self, key: &str) -> Option<DOMUserData> {
    todo!()
  }
  fn has_attributes(&self) -> bool {
    todo!()
  }
  fn has_child_nodes(&self) -> bool {
    todo!()
  }
  fn insert_before(&mut self, new_child: NodeRef, ref_child: NodeRef) -> W3C::Result<NodeRef> {
    todo!()
  }
  fn is_default_namespace(&self, namespace_uri: &str) -> bool {
    todo!()
  }
  fn is_equal_node(&self, arg: &NodeRef) -> bool {
    todo!()
  }
  fn is_same_node(&self, other: &NodeRef) -> bool {
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
  fn remove_child(&mut self, old_child: &NodeRef) -> W3C::Result<NodeRef> {
    todo!()
  }
  fn replace_child(&mut self, new_child: NodeRef, old_child: NodeRef) -> W3C::Result<NodeRef> {
    todo!()
  }
  fn set_user_data(
    &mut self, key: &str, data: W3C::DOMUserData, handler: Box<dyn W3C::UserDataHandler<IMPL>>,
  ) -> Option<W3C::DOMUserData> {
    todo!()
  }
}

impl W3C::Document<IMPL> for Document {
  fn doctype(&self) -> &DocumentType {
    todo!()
  }

  fn implementation(&self) -> &IMPL {
    todo!()
  }

  fn document_element(&self) -> &Element {
    todo!()
  }

  fn create_element(&self, tag_name: &str) -> W3C::Result<Element> {
    todo!()
  }

  fn create_document_fragment(&self) -> DocumentFragment {
    todo!()
  }

  fn create_text_node(&self, data: &str) -> Text {
    todo!()
  }

  fn create_comment(data: &str) -> Comment {
    todo!()
  }

  fn create_cdata_section(data: &str) -> CDATASection {
    todo!()
  }

  fn create_processing_instruction(&self, target: &str, data: &str) -> ProcessingInstruction {
    todo!()
  }

  fn create_attribute(&self, name: &str) -> W3C::Result<Attr> {
    todo!()
  }

  fn create_entity_reference(&self, name: &str) -> W3C::Result<EntityReference> {
    todo!()
  }

  fn get_elements_by_tag_name(&self, tagname: &str) -> NodeList {
    todo!()
  }

  fn import_node(&mut self, imported_node: W3C::NodeRef<IMPL>, deep: bool) -> W3C::NodeRef<IMPL> {
    todo!()
  }

  fn create_element_ns(&self, namespace_uri: &str, qualified_name: &str) -> W3C::Result<Element> {
    todo!()
  }

  fn create_attribute_ns(&self, namespace_uri: &str, qualified_name: &str) -> W3C::Result<Attr> {
    todo!()
  }

  fn get_elements_by_tag_name_ns(&self, namespace_uri: &str, local_name: &str) -> NodeList {
    todo!()
  }

  fn get_element_by_id(&self, element_id: &str) -> Option<Element> {
    todo!()
  }

  fn clone_document(&self, deep: bool) -> Self {
    todo!()
  }
}
