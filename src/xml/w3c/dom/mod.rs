//! This module provides *Document Object Model* (DOM) traits, supporting
//! "[Document Object Model Level 2 Core API](https://www.w3.org/TR/DOM-Level-2-Core/)"",
//! "[Document Object Model (DOM) Level 3 Core](https://www.w3.org/TR/DOM-Level-3-Core/)"",
//! and "[Document Object Model (DOM) Level 3 Load and Save](https://www.w3.org/TR/DOM-Level-3-LS/)"".
//!
//! `null` -> `None`
//!
//! ## W3C DOM2 Compatibility
//!
//! | W3C              | Rust               |
//! |:-----------------|:-------------------|
//! | UPPER_CASE consts | CamelCase         |
//! | CamelCase fields/methods | snake_case |
//! | nullable         | `Option`           |
//! | `DOMString`      | `&str` or `String` |
//! | `boolean`        | `bool`             |
//! | `unsigned short` | `u16`              |
//! | `unsigned long`  | `usize`            |
//!
//! Note that all indices in the W3C specification are **0-origin**.
//!

use std::rc::Rc;
use thiserror::Error;

#[cfg(test)]
mod test;

/// [Definition group *NodeType*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#ID-1841493061)
///
#[derive(PartialEq, Eq)]
pub enum NodeType {
  ElementNode = 1,
  AttributeNode = 2,
  TextNode = 3,
  CDATASectionNode = 4,
  EntityReferenceNode = 5,
  EntityNode = 6,
  ProcessingInstructionNode = 7,
  CommentNode = 8,
  DocumentNode = 9,
  DocumentTypeNode = 10,
  DocumentFragmentNode = 11,
  NotaionNode = 12,
}

/// [Definition group *DocumentPosition*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#DocumentPosition)
///
pub enum DocumentPosition {
  Disconnected = 0x01,
  Preceding = 0x02,
  Following = 0x04,
  Contains = 0x08,
  ContainedBy = 0x10,
  ImplementationSpecific = 0x20,
}

pub type DOMObject = ();

/// [Interface *DOMImplementation*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-102161490)
///
pub trait DOMImplementation: Sized {
  type Document: Document<Self>;
  type DocumentType: DocumentType<Self>;
  type Element: Element<Self>;
  type NamedNodeMap: NamedNodeMap<Self>;
  type NodeList: NodeList<Self>;
  type DocumentFragment: DocumentFragment<Self>;
  type Text: Text<Self>;
  type Comment: Comment<Self>;
  type CDATASection: CDATASection<Self>;
  type ProcessingInstruction: ProcessingInstruction<Self>;
  type Attr: Attr<Self>;
  type EntityReference: EntityReference<Self>;
  type Notation: Notation<Self>;
  type Entity: Entity<Self>;

  fn has_feature(&self, feature: &str, version: Option<&str>) -> bool;
  fn create_document_type(&self, qualified_name: &str, public_id: &str, system_id: &str) -> Result<Self::DocumentType>;
  fn create_document(&self, namespace_uri: &str, qualified_name: &str, doctype: Self::DocumentType) -> Self::Document;
  fn get_feature(&self, feature: &str, version: Option<&str>) -> Option<DOMObject>;
}

/// [Interface *DocumentFragment*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-B63ED1A3)
///
pub trait DocumentFragment<IMPL: DOMImplementation>: Node<IMPL> {
  fn clone_document_fragment(&self, deep: bool) -> Self;
}

/// [Interface *Document*](https://www.w3.org/TR/DOM-Level-2-Core/#core-i-Document)
///
pub trait Document<IMPL: DOMImplementation>: Node<IMPL> {
  fn doctype(&self) -> &IMPL::DocumentType;
  fn implementation(&self) -> &IMPL;
  fn document_element(&self) -> &IMPL::Element;
  fn create_element(&self, tag_name: &str) -> Result<IMPL::Element>;
  fn create_document_fragment(&self) -> IMPL::DocumentFragment;
  fn create_text_node(&self, data: &str) -> IMPL::Text;
  fn create_comment(data: &str) -> IMPL::Comment;
  fn create_cdata_section(data: &str) -> IMPL::CDATASection;
  fn create_processing_instruction(&self, target: &str, data: &str) -> IMPL::ProcessingInstruction;
  fn create_attribute(&self, name: &str) -> Result<IMPL::Attr>;
  fn create_entity_reference(&self, name: &str) -> Result<IMPL::EntityReference>;
  fn get_elements_by_tag_name(&self, tagname: &str) -> IMPL::NodeList;
  fn import_node(&mut self, imported_node: NodeRef<IMPL>, deep: bool) -> NodeRef<IMPL>;
  fn create_element_ns(&self, namespace_uri: &str, qualified_name: &str) -> Result<IMPL::Element>;
  fn create_attribute_ns(&self, namespace_uri: &str, qualified_name: &str) -> Result<IMPL::Attr>;
  fn get_elements_by_tag_name_ns(&self, namespace_uri: &str, local_name: &str) -> IMPL::NodeList;
  fn get_element_by_id(&self, element_id: &str) -> Option<IMPL::Element>;

  fn clone_document(&self, deep: bool) -> Self;
}

/// [Interface *Node*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-1950641247)
///
pub trait Node<IMPL: DOMImplementation> {
  fn attributes(&self) -> Option<IMPL::NamedNodeMap>;
  fn base_uri(&self) -> Option<String>;
  fn child_nodes(&self) -> &IMPL::NodeList;
  fn first_child(&self) -> Option<NodeRef<IMPL>>;
  fn last_child(&self) -> Option<NodeRef<IMPL>>;
  fn local_name(&self) -> Option<&str>;
  fn namespace_uri(&self) -> Option<&str>;
  fn next_sibling(&self) -> Option<NodeRef<IMPL>>;
  fn node_name(&self) -> &str;
  fn node_type(&self) -> NodeType;
  fn node_value(&self) -> Result<Option<&str>>;
  fn set_node_value(&mut self, value: &str) -> Result<()>;
  fn owner_document(&self) -> Option<&IMPL::Document>;
  fn parent_node(&self) -> Option<NodeRef<IMPL>>;
  fn prefix(&self) -> Option<String>;
  fn set_prefix(&self, prefix:Option<String>) -> Result<()>;
  fn previous_sibling(&self) -> Option<NodeRef<IMPL>>;
  fn text_content(&self) -> Option<String>;
  fn set_text_content(&self, text_content: &str) -> Result<()>;

  fn append_child(&mut self, new_child: NodeRef<IMPL>) -> Result<&NodeRef<IMPL>>;
  // fn clone_node(deep: bool) -> Self;
  fn compare_document_position(&self, other: &NodeRef<IMPL>) -> DocumentPosition {
    todo!()
  }
  fn get_feature(&self, feature: &str, version: Option<&str>) -> Option<DOMObject>;
  fn get_user_data(&self, key: &str) -> Option<DOMUserData>;
  fn has_attributes(&self) -> bool;
  fn has_child_nodes(&self) -> bool;
  fn insert_before(&mut self, new_child: NodeRef<IMPL>, ref_child: NodeRef<IMPL>) -> Result<NodeRef<IMPL>>;
  fn is_default_namespace(&self, namespace_uri: &str) -> bool;
  fn is_equal_node(&self, arg: &NodeRef<IMPL>) -> bool;
  fn is_same_node(&self, other: &NodeRef<IMPL>) -> bool;
  fn is_supported(&self, feature: &str, version: &str) -> bool;
  fn lookup_namespace_uri(&self, prefix: &str) -> Option<&str>;
  fn lookup_prefix(&self, namespace_uri: &str) -> Option<&str>;
  fn normalize(&mut self);
  fn remove_child(&mut self, old_child: &NodeRef<IMPL>) -> Result<NodeRef<IMPL>>;
  fn replace_child(&mut self, new_child: NodeRef<IMPL>, old_child: NodeRef<IMPL>) -> Result<NodeRef<IMPL>>;
  fn set_user_data(
    &mut self, key: &str, data: DOMUserData, handler: Box<dyn UserDataHandler<IMPL>>,
  ) -> Option<DOMUserData>;
}

/// [Interface *NodeList*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-536297177)
///
pub trait NodeList<IMPL: DOMImplementation> {
  fn item(&self, index: usize) -> Option<NodeRef<IMPL>>;
  fn length(&self) -> usize;
}

/// [Interface *NamedNodeMap*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#ID-1780488922)
///
pub trait NamedNodeMap<IMPL: DOMImplementation> {
  fn length(&self) -> usize;

  fn get_named_item(&self, name: &str) -> Option<NodeRef<IMPL>>;
  fn get_named_item_ns(&self, namespace_uri: &str, local_name: &str) -> Result<Option<NodeRef<IMPL>>>;
  fn item(&self, index: usize) -> Option<NodeRef<IMPL>>;
  fn remove_named_item(&mut self, name: &str) -> Result<NodeRef<IMPL>>;
  fn remove_named_item_ns(&mut self, namespace_uri: &str, local_name: &str) -> Result<NodeRef<IMPL>>;
  fn set_named_item(&mut self, arg: NodeRef<IMPL>) -> Result<Option<NodeRef<IMPL>>>;
  fn set_named_item_ns(&mut self, arg: NodeRef<IMPL>) -> Result<Option<NodeRef<IMPL>>>;
}

/// [Interface *CharacterData*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-FF21A306)
///
pub trait CharacterData<IMPL: DOMImplementation>: Node<IMPL> {
  fn data(&self) -> &str;
  fn substring_data(&self, offset: usize, count: usize) -> String;
  fn append_data(&mut self, arg: &str);
  fn insert_data(&mut self, offset: usize, arg: &str) -> Result<()>;
  fn delete_data(&mut self, offset: usize, count: usize) -> Result<()>;
  fn replace_data(&mut self, offset: usize, count: usize) -> Result<()>;
}

/// [Interface *Attr*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-637646024)
///
pub trait Attr<IMPL: DOMImplementation>: Node<IMPL> {
  fn name(&self) -> &str;
  fn specified(&self) -> bool;
  fn value(&self) -> &str;
  fn set_value(&mut self, value: &str) -> Result<()>;
  fn owner_element(&self) -> &IMPL::Element;

  fn clone_attr(&self, deep: bool) -> Self;
}

/// [Interface *Element*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-745549614)
///
pub trait Element<IMPL: DOMImplementation>: Node<IMPL> {
  fn tag_name(&self) -> &str;
  fn get_attribute(&self, name: &str) -> Option<&str>;
  fn set_attribute(&mut self, name: &str, value: &str) -> Result<()>;
  fn remove_attribute(&mut self, name: &str) -> Result<()>;
  fn get_attribute_node(&self, name: &str) -> Option<IMPL::Attr>;
  fn set_attribute_node(&mut self, new_attr: IMPL::Attr) -> Result<Option<IMPL::Attr>>;
  fn remove_attribute_node(&mut self, old_attr: IMPL::Attr) -> Result<Option<IMPL::Attr>>;
  fn get_elements_by_tag_name(&self, name: &str) -> IMPL::NodeList;
  fn get_attribute_ns(&self, namespace_uri: &str, local_name: &str) -> Option<&str>;
  fn set_attribute_ns(&self, namespace_uri: &str, local_name: &str, value: &str) -> Result<()>;
  fn remove_attribute_ns(&mut self, namespace_uri: &str, local_name: &str) -> Result<()>;
  fn get_attribute_node_ns(&self, namespace_uri: &str, local_name: &str) -> Option<&IMPL::Attr>;
  fn set_attribute_node_ns(&mut self, new_attr: IMPL::Attr) -> Result<Option<IMPL::Attr>>;
  fn get_elements_by_tag_name_ns(&self, namespace_uri: &str, local_name: &str) -> IMPL::NodeList;
  fn has_attribute(&self, name: &str) -> bool;
  fn has_attribute_ns(namespace_uri: &str, local_name: &str) -> bool;

  fn clone_element(&self, deep: bool) -> Self;
}

/// [Interface *Text*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-1312295772)
///
pub trait Text<IMPL: DOMImplementation>: CharacterData<IMPL> {
  fn split_text(&self, offset: usize) -> Result<IMPL::Text>;

  fn clone_text(&self, deep: bool) -> Self;
}

/// [Interface *Text*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-1728279322)
///
pub trait Comment<IMPL: DOMImplementation>: CharacterData<IMPL> {
  fn clone_comment(&self, deep: bool) -> Self;
}

/// [Interface *CDATASection*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-667469212)
///
pub trait CDATASection<IMPL: DOMImplementation>: Text<IMPL> {
  fn clone_cdata_section(&self, deep: bool) -> Self;
}

/// [Interface *DocumentType*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-412266927)
///
pub trait DocumentType<IMPL: DOMImplementation>: Node<IMPL> {
  fn name(&self) -> &str;
  fn entities(&self) -> &IMPL::NamedNodeMap;
  fn notations(&self) -> IMPL::NamedNodeMap;
  fn public_id(&self) -> &str;
  fn system_id(&self) -> &str;
  fn internal_subset(&self) -> Option<&str>;

  fn clone_document_type(&self, deep: bool) -> Self;
}

/// [Interface *Notation*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-5431D1B9)
///
pub trait Notation<IMPL: DOMImplementation>: Node<IMPL> {
  fn public_id(&self) -> &str;
  fn system_id(&self) -> &str;

  fn clone_notation(&self, deep: bool) -> Self;
}

/// [Interface *Entity*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-527DCFF2)
///
pub trait Entity<IMPL: DOMImplementation>: Node<IMPL> {
  fn public_id(&self) -> &str;
  fn system_id(&self) -> &str;
  fn notation_name(&self) -> &str;

  fn clone_entity(&self, deep: bool) -> Self;
}

/// [Interface *EntityReference*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-11C98490)
///
pub trait EntityReference<IMPL: DOMImplementation>: Node<IMPL> {
  fn clone_entity_reference(&self, deep: bool) -> Self;
}

/// [Interface *ProcessingInstruction*](https://www.w3.org/TR/DOM-Level-2-Core/#core-ID-1004215813)
///
pub trait ProcessingInstruction<IMPL: DOMImplementation>: Node<IMPL> {
  fn target(&self) -> &str;
  fn data(&self) -> &str;
  fn set_data(&mut self, data: &str) -> Result<()>;

  fn clone_processing_instruction(&self, deep: bool) -> Self;
}

pub enum NodeRef<IMPL: DOMImplementation> {
  DocumentFragment(IMPL::DocumentFragment),
  Document(Rc<IMPL::Document>),
  Attr(Rc<IMPL::Attr>),
  Element(Rc<IMPL::Element>),
  Text(Rc<IMPL::Text>),
  Comment(Rc<IMPL::Comment>),
  CDATASection(Rc<IMPL::CDATASection>),
  DocumentType(Rc<IMPL::DocumentType>),
  Notation(Rc<IMPL::Notation>),
  Entity(Rc<IMPL::Entity>),
  EntityReference(Rc<IMPL::EntityReference>),
  ProcessingInstruction(Rc<IMPL::ProcessingInstruction>),
}

impl<IMPL: DOMImplementation> NodeRef<IMPL> {
  pub fn clone_node(&self, deep: bool) -> Self {
    match self {
      NodeRef::DocumentFragment(n) => NodeRef::DocumentFragment(n.clone_document_fragment(deep)),
      NodeRef::Document(n) => NodeRef::Document(Rc::new(n.clone_document(deep))),
      NodeRef::Attr(n) => NodeRef::Attr(Rc::new(n.clone_attr(deep))),
      NodeRef::Element(n) => NodeRef::Element(Rc::new(n.clone_element(deep))),
      NodeRef::Text(n) => NodeRef::Text(Rc::new(n.clone_text(deep))),
      NodeRef::Comment(n) => NodeRef::Comment(Rc::new(n.clone_comment(deep))),
      NodeRef::CDATASection(n) => NodeRef::CDATASection(Rc::new(n.clone_cdata_section(deep))),
      NodeRef::DocumentType(n) => NodeRef::DocumentType(Rc::new(n.clone_document_type(deep))),
      NodeRef::Notation(n) => NodeRef::Notation(Rc::new(n.clone_notation(deep))),
      NodeRef::Entity(n) => NodeRef::Entity(Rc::new(n.clone_entity(deep))),
      NodeRef::EntityReference(n) => NodeRef::EntityReference(Rc::new(n.clone_entity_reference(deep))),
      NodeRef::ProcessingInstruction(n) => {
        NodeRef::ProcessingInstruction(Rc::new(n.clone_processing_instruction(deep)))
      }
    }
  }

  pub fn as_node(&self) -> &dyn Node<IMPL> {
    let node: &dyn Node<IMPL> = match self {
      NodeRef::DocumentFragment(n) => n,
      _ => todo!(),
    };
    node
  }

  pub fn as_mut_node(&mut self) -> &mut dyn Node<IMPL> {
    let node: &mut dyn Node<IMPL> = match self {
      NodeRef::DocumentFragment(n) => n,
      _ => todo!(),
    };
    node
  }
}

impl<IMPL: DOMImplementation> Clone for NodeRef<IMPL> {
  fn clone(&self) -> Self {
    self.clone_node(true)
  }
}

/// [Type Definition *DOMUserData*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#DOMUserData)
///
pub type DOMUserData = (); // FIXME: how to express `any`?

pub enum UserDataHandlerOperationType {
  Cloned,
  Imported,
  Deleted,
  Removed,
  Adopted,
}
pub trait UserDataHandler<IMPL: DOMImplementation> {
  fn handle(
    &self, operation: UserDataHandlerOperationType, key: &str, data: &DOMUserData, src: &NodeRef<IMPL>,
    dst: &NodeRef<IMPL>,
  );
}

pub type Result<T> = std::result::Result<T, DOMException>;

/// [Exception *DOMException*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#ID-17189187)
///
#[derive(Error, Debug)]
pub enum DOMException {
  #[error("INDEX_SIZE_ERROR")]
  IndexSize,
  #[error("DOMSTRING_SIZE_ERR")]
  DOMStringSize,
  #[error("HIERARCHY_REQUEST_ERR")]
  HierarchyRequest,
  #[error("WRONG_DOCUMENT_ERR")]
  WrongDocument,
  #[error("INVALID_CHARACTER_ERR")]
  InvalidCharacter,
  #[error("NO_DATA_ALLOWED_ERR")]
  NoDataAllowed,
  #[error("NO_MODIFICATION_ALLOWED_ERR")]
  NoModificationAllowed,
  #[error("NOT_FOUND_ERR")]
  NotFound,
  #[error("NOT_SUPPORTED_ERR")]
  NotSupported,
  #[error("INUSE_ATTRIBUTE_ERR")]
  InuseAttribute,
  #[error("INVALID_STATE_ERR")]
  InvalidState,
  #[error("SYNTAX_ERR")]
  Syntax,
  #[error("INVALID_MODIFICATION_ERR")]
  InvalidModification,
  #[error("NAMESPACE_ERR")]
  Namespace,
  #[error("INVALID_ACCESS_ERR")]
  InvalidAccess,
  #[error("VALIDATION_ERR")]
  Validation,
  #[error("TYPE_MISMATCH_ERR")]
  TypeMismatch,
}

impl DOMException {
  /// [Definition group *ExceptionCode*](https://www.w3.org/TR/DOM-Level-3-Core/core.html#ID-258A00AF)
  ///
  pub fn code(&self) -> u16 {
    match self {
      DOMException::IndexSize => 1,
      DOMException::DOMStringSize => 2,
      DOMException::HierarchyRequest => 3,
      DOMException::WrongDocument => 4,
      DOMException::InvalidCharacter => 5,
      DOMException::NoDataAllowed => 6,
      DOMException::NoModificationAllowed => 7,
      DOMException::NotFound => 8,
      DOMException::NotSupported => 9,
      DOMException::InuseAttribute => 10,
      DOMException::InvalidState => 11,
      DOMException::Syntax => 12,
      DOMException::InvalidModification => 13,
      DOMException::Namespace => 14,
      DOMException::InvalidAccess => 15,
      DOMException::Validation => 16,
      DOMException::TypeMismatch => 17,
    }
  }
}
