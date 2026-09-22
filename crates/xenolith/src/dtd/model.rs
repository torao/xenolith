//! DTD declarations stored as data.
//!
//! A DTD declares the elements that may appear in a document and their content, the attributes elements may possess
//! (including types and default values), entities the document can reference, and notations specifying the format of
//! unparsed data. [`Dtd`] stores these declarations in their raw form. [`Dtd`] is constructed via `declare_*` methods,
//! and information is retrieved through query methods.
//!
//! This module does not perform parsing or validation itself. [`read`](crate::dtd::read) constructs a [`Dtd`] from
//! text, while [`validate`](crate::dtd::validate) validates a document against a [`Dtd`].
//!
//! Names are treated as lexical entities. Since DTDs predate the concept of namespaces, matching is performed based on
//! qualified names (including prefixes). Consequently, `p:a` and `q:a` are considered distinct elements, regardless of
//! prefix bindings. All names are stored as [`NameId`]s, interned (deduplicated and assigned an ID) within a
//! [`NamePool`](crate::name::NamePool), and are meaningless without that pool.

use std::collections::{HashMap, HashSet};

use crate::name::NameId;

/// A general entity: an entity referenced as `&name;` in content or attribute values, or, in the case of unparsed
/// entities, one whose name is specified by an `ENTITY` or `ENTITIES` attribute (XML 1.0 §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneralEntity {
  /// Declared as replacement text within a literal.
  Internal {
    /// The replacement text: A literal in which character references and parameter entity references have been
    /// replaced, while general entity references remain as written (to be expanded at the point of reference).
    value: String,
  },

  /// Declared as a separate resource whose text is parsed as part of the referencing document.
  External {
    /// The public identifier (if specified).
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
    /// The system identifier of the resource the declaration was read from, which a relative `system_id` is resolved
    /// against (XML 1.0 §4.2.2). `None` if that resource has none.
    base: Option<String>,
  },

  /// Declared using `NDATA`: Data in the format indicated by the notation, which is not parsed by the processor. It
  /// can only be specified via `ENTITY` or `ENTITIES` attributes and is never referenced using the `&name;` syntax.
  Unparsed {
    /// The public identifier (if specified).
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
    /// The system identifier of the resource the declaration was read from, which a relative `system_id` is resolved
    /// against (XML 1.0 §4.2.2). `None` if that resource has none.
    base: Option<String>,
    /// The notation indicating the data format.
    notation: NameId,
  },
}

/// A Parameter entity: Referenced as `%name;` and used only within the DTD (XML 1.0 §4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterEntity {
  /// Declared with replacement text within a literal.
  Internal {
    /// The replacement text is provided in the same manner as [`GeneralEntity::Internal`].
    value: String,
  },

  /// Declared as a separate resource; retrieved and loaded at the point of reference.
  External {
    /// The public identifier (if specified).
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
    /// The system identifier of the resource the declaration was read from, which a relative `system_id` is resolved
    /// against (XML 1.0 §4.2.2). `None` if that resource has none.
    base: Option<String>,
  },
}

/// The attribute declared type (XML 1.0 §3.3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttType {
  /// `CDATA`: Character data. The only type where the value is not normalized as a token.
  Cdata,
  /// `ID`: A name that is unique among `ID` values in the document.
  Id,
  /// `IDREF`: A name that matches an `ID` value in the document.
  IdRef,
  /// `IDREFS`: A whitespace-separated list of `IDREF`s.
  IdRefs,
  /// `ENTITY`: The name of an unparsed entity.
  Entity,
  /// `ENTITIES`: A whitespace-separated list of `ENTITY` names.
  Entities,
  /// `NMTOKEN`: A name token.
  Nmtoken,
  /// `NMTOKENS`: A whitespace-separated list of name tokens.
  Nmtokens,
  /// `NOTATION (a | b | ...)`: One of the enumerated notation names.
  Notation(Vec<NameId>),
  /// `(a | b | ...)`: One of the enumerated name tokens.
  Enumeration(Vec<NameId>),
}

impl AttType {
  /// True for all types other than `CDATA`. For tokenized values, in addition to the white space to space replacement
  /// performed on all attribute values (XML 1.0 §3.3.3), leading and trailing spaces are removed, and consecutive
  /// spaces are collapsed into a single space.
  pub fn is_tokenized(&self) -> bool {
    !matches!(self, Self::Cdata)
  }
}

/// Rules regarding attribute omission and default values in start tags (XML 1.0 §3.3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultDecl {
  /// `#REQUIRED`: A value must be specified in every start-tag.
  Required,
  /// `#IMPLIED`: The attribute is optional, and there is no default value.
  Implied,
  /// `#FIXED "value"`: This value applies if the attribute is omitted; if specified in a start-tag, it must match this
  /// value.
  Fixed(String),
  /// `"value"`: The value applied if the attribute is omitted.
  Default(String),
}

impl DefaultDecl {
  /// The value provided for an omitted attribute: the value declared as `#FIXED` or the ordinary default value;
  /// otherwise, `None`.
  pub fn value(&self) -> Option<&str> {
    match self {
      Self::Fixed(value) | Self::Default(value) => Some(value),
      Self::Required | Self::Implied => None,
    }
  }
}

/// One of the attribute definitions in an `<!ATTLIST>` declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttDef {
  /// TThe qualified name of the attribute (including the prefix).
  pub name: NameId,
  /// The declared type.
  pub att_type: AttType,
  /// The declared default value.
  pub default: DefaultDecl,
}

/// An external identifier — specifically, `SYSTEM "uri"` or `PUBLIC "id" "uri"` (XML 1.0 §4.2.2). [`Dtd`] holds one of
/// these for each notation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalId {
  /// The public identifier, if specified.
  pub public_id: Option<String>,
  /// The system identifier. This is `None` only for notations declared as `PUBLIC` that do not have a system literal.
  pub system_id: Option<String>,
}

/// Content particle occurrence frequency: Indicated in the content model by the following suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occurs {
  /// No suffix: exactly once.
  Once,
  /// `?`: zero or one.
  Optional,
  /// `*`: zero or more.
  ZeroOrMore,
  /// `+`: one or more.
  OneOrMore,
}

/// The particles of an element content model and their occurrence frequencies (XML 1.0 §3.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentParticle {
  /// A child element name.
  Name(NameId, Occurs),
  /// A choice, `(a | b | ...)`: one of the particles.
  Choice(Vec<ContentParticle>, Occurs),
  /// A sequence, `(a, b, ...)`: each particle in order.
  Seq(Vec<ContentParticle>, Occurs),
}

/// The content permitted by the `<!ELEMENT>` declaration (XML 1.0 §3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentSpec {
  /// `EMPTY`: no content at all.
  Empty,
  /// `ANY`: any mix of character data and child elements, each child of a declared element type.
  Any,
  /// Mixed content: `(#PCDATA)`, or `(#PCDATA | a | b)*` with the listed child names in any order and number,
  /// interleaved with character data.
  Mixed(Vec<NameId>),
  /// Element content: child elements only, as the particle allows, with nothing but white space between them.
  Children(ContentParticle),
}

/// The DTD declarations maintained in the order they were read.
///
/// The document parser generates this when reading the `DOCTYPE`, while [`DtdReader`](crate::dtd::DtdReader) generates
/// it directly from the DTD. Because [`Clone`] is implemented, a validator can hold a copy of this structure while the
/// parser that read it continues to generate events.
///
/// Each declaration is managed using a [`NameId`] as the key. To resolve an ID back to text or to look up a
/// declaration by interning (registering) a name, use the [`NamePool`](crate::name::NamePool) where the name was
/// interned.
///
/// If the same name is declared multiple times, the `declare_*` methods retain the initial declaration. It is up to
/// the caller to decide whether duplicates constitute an error (this behavior applies to elements and notations, but
/// not to entities or attributes).
///
/// # Examples
///
/// ```
/// use xenolith::dtd::model::{ContentSpec, Dtd};
/// use xenolith::name::NamePool;
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
  /// General entities declared in the external part of the DTD. A document with `standalone="yes"` may not reference
  /// one (XML 1.0 §2.9, §4.1).
  external_general: HashSet<NameId>,
  /// Elements with an attribute-list declaration in the external part of the DTD. A document with `standalone="yes"`
  /// may not take a default or a tokenized normalization from one (XML 1.0 §2.9).
  external_attlist: HashSet<NameId>,
}

impl Dtd {
  /// The declaration of a general entity, if it was declared.
  pub fn general_entity(&self, name: NameId) -> Option<&GeneralEntity> {
    self.general.get(&name)
  }

  /// The attribute definitions declared for an element, in declaration order, if it has any.
  pub fn attlist(&self, element: NameId) -> Option<&[AttDef]> {
    self.attlists.get(&element).map(Vec::as_slice)
  }

  /// True if the general entity is marked as declared outside the internal subset by
  /// [`mark_general_entity_external`](Self::mark_general_entity_external) (and thus cannot be referenced from a
  /// standalone document).
  pub fn general_entity_is_external(&self, name: NameId) -> bool {
    self.external_general.contains(&name)
  }

  /// True if any of the element's attribute list declarations are marked by
  /// [`mark_attlist_external`](Self::mark_attlist_external) as having been declared outside the internal subset. In
  /// this case, a standalone document cannot derive default values or tokenized normalization from that declaration.
  pub fn attlist_is_external(&self, element: NameId) -> bool {
    self.external_attlist.contains(&element)
  }

  /// The content specification declared for the element (if declared).
  pub fn content_spec(&self, element: NameId) -> Option<&ContentSpec> {
    self.elements.get(&element)
  }

  /// True if the element has a `<!ELEMENT>` declaration.
  pub fn has_element(&self, element: NameId) -> bool {
    self.elements.contains_key(&element)
  }

  /// True if a notation with this name is declared.
  pub fn has_notation(&self, name: NameId) -> bool {
    self.notations.contains_key(&name)
  }

  /// Every element with an attribute-list declaration, and its definitions, in no particular order of elements.
  pub fn attlists(&self) -> impl Iterator<Item = (NameId, &[AttDef])> {
    self.attlists.iter().map(|(&name, defs)| (name, defs.as_slice()))
  }

  /// Every element with an `<!ELEMENT>` declaration, and its content specification, in no particular order.
  pub fn elements(&self) -> impl Iterator<Item = (NameId, &ContentSpec)> {
    self.elements.iter().map(|(&name, spec)| (name, spec))
  }

  /// The external identifier declared for a notation, if it was declared.
  pub fn notation(&self, name: NameId) -> Option<&ExternalId> {
    self.notations.get(&name)
  }

  /// Every declared general entity, and its declaration, in no particular order.
  pub fn general_entities(&self) -> impl Iterator<Item = (NameId, &GeneralEntity)> {
    self.general.iter().map(|(&name, entity)| (name, entity))
  }

  /// Every declared parameter entity, and its declaration, in no particular order.
  pub fn parameter_entities(&self) -> impl Iterator<Item = (NameId, &ParameterEntity)> {
    self.parameter.iter().map(|(&name, entity)| (name, entity))
  }

  /// The declaration of a parameter entity, if it was declared.
  pub fn parameter_entity(&self, name: NameId) -> Option<&ParameterEntity> {
    self.parameter.get(&name)
  }

  /// Every declared notation, and its external identifier, in no particular order.
  pub fn notations(&self) -> impl Iterator<Item = (NameId, &ExternalId)> {
    self.notations.iter().map(|(&name, id)| (name, id))
  }

  // --- Building ---------------------------------------------------------------------------------

  /// Declares the content of an element. If the element has already been declared, it returns `false` and retains the
  /// initial declaration.
  ///
  /// Since the appearance of a second `<!ELEMENT>` with the same name results in a validity error (XML 1.0 §3.2),
  /// callers that are unsure whether the name is new must check the return value.
  pub fn declare_element(&mut self, name: NameId, content: ContentSpec) -> bool {
    !self.elements.contains_key(&name) && self.elements.insert(name, content).is_none()
  }

  /// Adds an attribute definition to an element (appended after existing definitions).
  ///
  /// In XML, multiple attribute-list declarations can be made for a single element, and they are cumulative. If there
  /// are multiple declarations defining the same attribute, the first definition takes precedence (XML 1.0 §3.3).
  /// Consequently, any definition with a name that has already been defined is discarded. Valid definitions are
  /// retained in the order in which they were added.
  pub fn declare_attributes(&mut self, element: NameId, definitions: impl IntoIterator<Item = AttDef>) {
    let list = self.attlists.entry(element).or_default();
    for def in definitions {
      if !list.iter().any(|existing| existing.name == def.name) {
        list.push(def);
      }
    }
  }

  /// Declares a general entity. If an entity with the same name has already been declared, it returns `false` and
  /// retains the initial declaration. In XML, the first binding takes effect, and subsequent declarations are
  /// permitted (XML 1.0 §4.2).
  pub fn declare_general_entity(&mut self, name: NameId, entity: GeneralEntity) -> bool {
    !self.general.contains_key(&name) && self.general.insert(name, entity).is_none()
  }

  /// Declares a parameter entity. If an entity with the same name has already been declared, it returns `false` and
  /// retains the initial declaration. In XML, the first definition takes effect, and subsequent definitions are
  /// permitted (XML 1.0 §4.2).
  pub fn declare_parameter_entity(&mut self, name: NameId, entity: ParameterEntity) -> bool {
    !self.parameter.contains_key(&name) && self.parameter.insert(name, entity).is_none()
  }

  /// Declares a notation. If the notation has already been declared, it returns `false` and retains the initial
  /// declaration.
  ///
  /// As with elements, the presence of a second `<!NOTATION>` declaration for the same name constitutes a validity
  /// error (XML 1.0 §4.7).
  pub fn declare_notation(&mut self, name: NameId, id: ExternalId) -> bool {
    !self.notations.contains_key(&name) && self.notations.insert(name, id).is_none()
  }

  /// Marks a general entity as declared outside the internal subset (i.e., within an external subset or an external
  /// parameter entity). This causes [`general_entity_is_external`](Self::general_entity_is_external) to report it as
  /// such.
  pub fn mark_general_entity_external(&mut self, name: NameId) {
    self.external_general.insert(name);
  }

  /// Marks the element's attribute-list declaration as being outside the internal subset (i.e., in an external subset
  /// or external parameter entity). This causes [`attlist_is_external`](Self::attlist_is_external) to report this status.
  pub fn mark_attlist_external(&mut self, element: NameId) {
    self.external_attlist.insert(element);
  }
}
