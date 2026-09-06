//! The document type definition model.
//!
//! The Document Type Definition (DTD) declares the grammar for an XML document: which elements may appear, what
//! content each may contain, which attributes an element may have with their types and defaults, the entities used
//! within the document, and the notations for unparsed data.
//!
//! This module is the data model only. A parsed DTD is a [`Dtd`], a set of declarations kept as read. Reading a DTD
//! from text, assembling its internal and external subsets, and interpreting it against a document to check
//! conformance are behavior that lives in higher crates; here a [`Dtd`] is built through its declaration methods and
//! read back through its queries.
//!
//! A name is lexical. `p:a` and `q:a` are different elements regardless of how the prefixes are bound, because DTDs
//! predate XML namespaces and match on the fully qualified name, prefix included.

use std::collections::{HashMap, HashSet};

use crate::name::NameId;

/// A general entity: an object referenced within the content by name or in an attribute value (XML 1.0 §4.2).
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneralEntity {
  /// Declared with replacement text specified inline.
  Internal {
    /// The replacement text in which character references have been expanded. This is expanded at the locations of
    /// general-entity references.
    ///
    value: String,
  },
  /// Declared as a separate, parsed resource.
  ///
  External {
    /// The public identifier, if specified.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
  },
  /// Declared as binary data using the specified notation. The name can be specified only via the `ENTITY` attribute.
  ///
  Unparsed {
    /// The public identifier, if specified.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
    /// The notation used to identify the data format.
    notation: NameId,
  },
}

/// A parameter entity: an entity referenced only within the DTD, in the form `%name;` (XML 1.0 §4.2).
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterEntity {
  /// Declared with replacement text specified inline.
  ///
  Internal {
    /// The replacement text.
    value: String,
  },
  /// Declared as a separate resource and is read when the DTD is processed.
  ///
  External {
    /// The public identifier, if given.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
  },
}

/// The declared type of an attribute (XML 1.0 §3.3.1).
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttType {
  /// Character data; the only type whose value is not whitespace-collapsed.
  Cdata,
  /// A unique identifier.
  Id,
  /// A reference to an `ID` elsewhere in the document.
  IdRef,
  /// Whitespace-separated `IDREF`s.
  IdRefs,
  /// The name of an unparsed entity.
  Entity,
  /// Whitespace-separated `ENTITY` names.
  Entities,
  /// A name token.
  Nmtoken,
  /// Whitespace-separated name tokens.
  Nmtokens,
  /// One of the named notations.
  Notation(Vec<NameId>),
  /// One of the enumerated tokens.
  Enumeration(Vec<NameId>),
}

impl AttType {
  /// True for every type but `CDATA`, all of which have their values whitespace-collapsed.
  pub fn is_tokenized(&self) -> bool {
    !matches!(self, Self::Cdata)
  }
}

/// What an attribute defaults to when a start tag omits it (XML 1.0 §3.3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultDecl {
  /// `#REQUIRED`: the start tag must give a value.
  Required,
  /// `#IMPLIED`: no value, and no default.
  Implied,
  /// `#FIXED`: the value is fixed and a start tag may only repeat it.
  Fixed(String),
  /// A default value supplied when the attribute is absent.
  Default(String),
}

impl DefaultDecl {
  /// The default value to supply for an absent attribute, if any.
  pub fn value(&self) -> Option<&str> {
    match self {
      Self::Fixed(value) | Self::Default(value) => Some(value),
      Self::Required | Self::Implied => None,
    }
  }
}

/// One attribute definition from an `ATTLIST`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttDef {
  /// The attribute's lexical name.
  pub name: NameId,
  /// Its declared type.
  pub att_type: AttType,
  /// Its default.
  pub default: DefaultDecl,
}

/// An external identifier, as on a notation or an external entity (XML 1.0 §4.2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalId {
  /// The public identifier, if given.
  pub public_id: Option<String>,
  /// The system identifier, absent for a notation declared `PUBLIC` alone.
  pub system_id: Option<String>,
}

/// How often a content particle may occur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occurs {
  /// Exactly once.
  Once,
  /// `?`: zero or one.
  Optional,
  /// `*`: zero or more.
  ZeroOrMore,
  /// `+`: one or more.
  OneOrMore,
}

/// A particle of an element content model (XML 1.0 §3.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentParticle {
  /// A child element name.
  Name(NameId, Occurs),
  /// A choice of alternatives, `(a | b | ...)`.
  Choice(Vec<ContentParticle>, Occurs),
  /// A sequence, `(a, b, ...)`.
  Seq(Vec<ContentParticle>, Occurs),
}

/// The content specification of an element (XML 1.0 §3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentSpec {
  /// `EMPTY`: no content.
  Empty,
  /// `ANY`: any well-formed content.
  Any,
  /// Mixed content: `#PCDATA`, optionally with a choice of child names.
  Mixed(Vec<NameId>),
  /// Element content: a single particle.
  Children(ContentParticle),
}

/// A parsed document type definition: the declarations of a DTD, kept as read.
///
/// A document parser hands one over once the `DOCTYPE` has been read; a DTD reader produces one from text. Interpreting
/// the declarations against a document, to check that it conforms, is a validator's work, not done here. It is
/// [`Clone`] so a validator can own a copy and read it while the parser goes on producing events.
///
/// Declarations are keyed by the interned [`NameId`] of a name; resolve one back to text, or intern one to look a
/// declaration up, through the [`NamePool`](crate::name::NamePool) its names were interned in.
///
/// # Examples
///
/// ```
/// use xenolith_core::model::dtd::{ContentSpec, Dtd};
/// use xenolith_core::name::NamePool;
///
/// let mut pool = NamePool::new();
/// let mut dtd = Dtd::default();
/// dtd.declare_element(pool.intern("note"), ContentSpec::Mixed(vec![])); // <!ELEMENT note (#PCDATA)>
///
/// let elements: Vec<_> = dtd.elements().map(|(name, _)| pool.resolve(name).to_owned()).collect();
/// assert_eq!(elements, ["note"]);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Dtd {
  general: HashMap<NameId, GeneralEntity>,
  parameter: HashMap<NameId, ParameterEntity>,
  elements: HashMap<NameId, ContentSpec>,
  attlists: HashMap<NameId, Vec<AttDef>>,
  notations: HashMap<NameId, ExternalId>,
  /// General entities and elements declared in an external subset (or external parameter entity). Since documents with
  /// `standalone="yes"` must not depend on these, referencing such entities or setting attributes declared in an
  /// external subset to their default values results in a fatal error.
  external_general: HashSet<NameId>,
  external_attlist: HashSet<NameId>,
}

impl Dtd {
  /// The declaration of a general entity, if it has one.
  pub fn general_entity(&self, name: NameId) -> Option<&GeneralEntity> {
    self.general.get(&name)
  }

  /// The attribute definitions for an element's lexical name.
  pub fn attlist(&self, element: NameId) -> Option<&[AttDef]> {
    self.attlists.get(&element).map(Vec::as_slice)
  }

  /// True if the general entity was declared in a location where a standalone document may not depend on it.
  /// Specifically, this refers to within an external subset or within an external parameter entity.
  ///
  pub fn general_entity_is_external(&self, name: NameId) -> bool {
    self.external_general.contains(&name)
  }

  /// True if any of an element's attribute declarations came from the external subset, so a
  /// default or a tokenized normalization it supplies is off-limits to a standalone document.
  pub fn attlist_is_external(&self, element: NameId) -> bool {
    self.external_attlist.contains(&element)
  }

  /// The content specification declared for an element, if it was declared.
  pub fn content_spec(&self, element: NameId) -> Option<&ContentSpec> {
    self.elements.get(&element)
  }

  /// True if the element was declared with an `<!ELEMENT>` declaration.
  pub fn has_element(&self, element: NameId) -> bool {
    self.elements.contains_key(&element)
  }

  /// True if a notation of this name was declared.
  pub fn has_notation(&self, name: NameId) -> bool {
    self.notations.contains_key(&name)
  }

  /// Every element with an attribute-list declaration, and its definitions.
  pub fn attlists(&self) -> impl Iterator<Item = (NameId, &[AttDef])> {
    self.attlists.iter().map(|(&name, defs)| (name, defs.as_slice()))
  }

  /// Every element with an `<!ELEMENT>` declaration, and its content specification.
  pub fn elements(&self) -> impl Iterator<Item = (NameId, &ContentSpec)> {
    self.elements.iter().map(|(&name, spec)| (name, spec))
  }

  /// The external identifier declared for a notation, if it was declared.
  pub fn notation(&self, name: NameId) -> Option<&ExternalId> {
    self.notations.get(&name)
  }

  /// Every general entity declared, and its declaration.
  pub fn general_entities(&self) -> impl Iterator<Item = (NameId, &GeneralEntity)> {
    self.general.iter().map(|(&name, entity)| (name, entity))
  }

  /// Every parameter entity declared, and its declaration.
  pub fn parameter_entities(&self) -> impl Iterator<Item = (NameId, &ParameterEntity)> {
    self.parameter.iter().map(|(&name, entity)| (name, entity))
  }

  /// The declaration of a parameter entity, if it has one.
  pub fn parameter_entity(&self, name: NameId) -> Option<&ParameterEntity> {
    self.parameter.get(&name)
  }

  /// Every notation declared, and its external identifier.
  pub fn notations(&self) -> impl Iterator<Item = (NameId, &ExternalId)> {
    self.notations.iter().map(|(&name, id)| (name, id))
  }

  // --- Building one by hand -------------------------------------------------------------------

  /// Declares an element's content, returning `false` if it was already declared, which leaves the first declaration
  /// standing.
  ///
  /// XML makes a second `<!ELEMENT>` for the same name an error, so a caller assembling a DTD checks the return where
  /// it does not already know the name is fresh.
  ///
  pub fn declare_element(&mut self, name: NameId, content: ContentSpec) -> bool {
    !self.elements.contains_key(&name) && self.elements.insert(name, content).is_none()
  }

  /// Adds attribute definitions for an element, after any it already has.
  ///
  /// Attribute-list declarations accumulate: XML allows several for one element, and where two define the same
  /// attribute the first one stands. This keeps that rule, so a definition whose name an earlier one already gave is
  /// dropped, and the order the kept definitions arrive in is the order they are held.
  ///
  pub fn declare_attributes(&mut self, element: NameId, definitions: impl IntoIterator<Item = AttDef>) {
    let list = self.attlists.entry(element).or_default();
    for def in definitions {
      if !list.iter().any(|existing| existing.name == def.name) {
        list.push(def);
      }
    }
  }

  /// Declares a general entity, returning `false` if one of that name was already declared, which leaves the first
  /// standing as XML requires.
  pub fn declare_general_entity(&mut self, name: NameId, entity: GeneralEntity) -> bool {
    !self.general.contains_key(&name) && self.general.insert(name, entity).is_none()
  }

  /// Declares a parameter entity, returning `false` if one of that name was already declared, which leaves the first
  /// standing as XML requires.
  pub fn declare_parameter_entity(&mut self, name: NameId, entity: ParameterEntity) -> bool {
    !self.parameter.contains_key(&name) && self.parameter.insert(name, entity).is_none()
  }

  /// Declares a notation, returning `false` if it was already declared, which leaves the first declaration standing.
  ///
  /// XML makes a second `<!NOTATION>` for the same name an error, as it does for an element.
  ///
  pub fn declare_notation(&mut self, name: NameId, id: ExternalId) -> bool {
    !self.notations.contains_key(&name) && self.notations.insert(name, id).is_none()
  }

  /// Marks a general entity as declared where a standalone document may not depend on it, an external subset or an
  /// external parameter entity, so [`general_entity_is_external`](Self::general_entity_is_external) reports it.
  ///
  pub fn mark_general_entity_external(&mut self, name: NameId) {
    self.external_general.insert(name);
  }

  /// Marks an element's attribute declarations as coming from the external subset, so
  /// [`attlist_is_external`](Self::attlist_is_external) reports it.
  ///
  pub fn mark_attlist_external(&mut self, element: NameId) {
    self.external_attlist.insert(element);
  }
}
