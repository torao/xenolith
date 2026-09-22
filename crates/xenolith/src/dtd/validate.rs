//! Checking a document against a DTD.
//!
//! [`DtdValidator`] checks a document against the DTD the parser read: that the root is the one the `DOCTYPE` declares,
//! that every element and attribute is declared and used as declared, that content follows the declared model, and that
//! IDs are unique and every reference resolves. The parser has already done the DTD's parsing-side work (entities,
//! defaults, tokenized normalization), so this is pure constraint checking.
//!
//! It is the first implementation of the [`Validator`] contract, and it reaches the events the same way any other does.
//! [`content`] compiles a declared content model into the automaton that matches a child sequence, and [`document`]
//! carries [`DocumentDtd`], the validator whose schema is whatever DTD the document itself declares.
//!

pub mod content;
pub mod document;

pub use document::DocumentDtd;

use std::collections::HashMap;

use crate::attr::Attributes;
use crate::chars;
use crate::dtd::{AttDef, AttType, ContentSpec, DefaultDecl, Dtd, GeneralEntity};
use crate::error::{Location, Result};
use crate::event::{EventHandler, EventRef};
use crate::name::{self, NameId, NamePool};

use crate::dtd::validate::content::ContentModel;
use crate::event::validate::{Validator, ValidityError};

/// A validator against a document's own DTD.
///
/// Built from the [`Dtd`] once the `DOCTYPE` is read, which [`DocumentDtd`] does when asked to validate against the
/// document's own DTD. It owns a copy of the DTD, and of the [`NamePool`] that DTD's names were interned in, so it can
/// read both while the parser continues emitting events.
///
/// Owning that pool is what lets the validator check a source that interned its names elsewhere, a tree walk over a
/// [`Document`](crate::dom::Document) among them. An event hands a name over as text, so the validator interns
/// the form it was written in into its own pool and looks that up. The ids the DTD is keyed by never leave this
/// validator.
///
#[derive(Debug)]
pub struct DtdValidator {
  dtd: Dtd,
  /// The pool the DTD's names are interned in. Every `NameId` this validator holds belongs to it, and a name arriving
  /// from a source is interned here before it is compared with anything from the DTD.
  pool: NamePool,
  /// The root element name to require, when there is one to require. A `DOCTYPE` declares one; a DTD read on its own
  /// declares no root, so a schema built from one leaves the check out unless the caller asks for it.
  root: Option<NameId>,
  root_seen: bool,
  /// Whether the once-per-document checks over the declarations have run.
  declarations_checked: bool,
  /// Compiled content models, by element lexical name, built on first use.
  models: HashMap<NameId, Option<ContentModel>>,
  /// The stack of open elements: lexical name and the children gathered so far.
  open: Vec<OpenElement>,
  /// Every `ID` value seen, to catch duplicates, with where it appeared.
  ids: HashMap<String, Location>,
  /// Every `IDREF` value seen and where, checked at the end against `ids`.
  idrefs: Vec<(String, Location)>,
  /// Whether `xml:id` attributes are checked as IDs, recorded in the same `ids` space.
  xml_id: bool,
  /// The validity errors found so far, in document order.
  errors: Vec<ValidityError>,
}

#[derive(Debug)]
struct OpenElement {
  lexical: NameId,
  children: Vec<NameId>,
  content: ContentKind,
}

/// What an open element's content model permits, as far as character data goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentKind {
  /// `EMPTY`: no content at all, not even whitespace.
  Empty,
  /// Element content: children, with only whitespace between them.
  ElementOnly,
  /// `ANY` or mixed content, or an undeclared element: character data is fine.
  CharacterData,
}

impl DtdValidator {
  /// Creates a validator for `dtd`, whose names are interned in `pool`.
  ///
  /// `root` is the element the document must have at its root, which a `DOCTYPE` declares. Pass `None` for a DTD
  /// that declares none, and the root goes unchecked.
  ///
  #[must_use]
  pub fn new(dtd: Dtd, pool: NamePool, root: Option<NameId>) -> Self {
    Self {
      dtd,
      pool,
      root,
      root_seen: false,
      declarations_checked: false,
      models: HashMap::new(),
      open: Vec::new(),
      ids: HashMap::new(),
      idrefs: Vec::new(),
      xml_id: false,
      errors: Vec::new(),
    }
  }

  /// Also check `xml:id` attributes as IDs, sharing this validator's ID space so an `xml:id` and a declared `ID` with
  /// the same value collide. Off by default.
  #[must_use]
  pub fn with_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Keeps a validity error with `message` at `at`.
  ///
  /// The errors are gathered in a list the caller passes in rather than in the field they end up in, so a check may
  /// hold a borrow of this validator while it reports. [`handle`](EventHandler::handle) takes the field out for the
  /// length of one event and gives it back.
  ///
  fn report(&self, errors: &mut Vec<ValidityError>, at: &Location, message: String) {
    errors.push(ValidityError::new(message, at.clone()));
  }

  /// Checks the declarations themselves, once: the constraints that hold regardless of any document, for example, an
  /// ID attribute's default, one ID per element, the syntactic validity of each declared default value, and no name
  /// repeated in mixed content.
  fn check_declarations(&self, at: &Location, errors: &mut Vec<ValidityError>) {
    // Collect first so the borrow of the DTD does not outlive the reports.
    let attlists: Vec<(NameId, Vec<AttDef>)> = self.dtd.attlists().map(|(e, defs)| (e, defs.to_vec())).collect();

    for (element, defs) in attlists {
      let element_name = self.pool.resolve(element);
      let mut ids = 0;
      for def in &defs {
        if matches!(def.att_type, AttType::Id) {
          ids += 1;
          // VC: ID Attribute Default. An ID must be #IMPLIED or #REQUIRED.
          if matches!(def.default, DefaultDecl::Fixed(_) | DefaultDecl::Default(_)) {
            let name = self.pool.resolve(def.name);
            let message = format!("the ID attribute \"{name}\" of \"{element_name}\" must be #IMPLIED or #REQUIRED");
            errors.push(ValidityError::new(message, at.clone()));
          }
        }
        // VC: Notation Attributes. Every name in a NOTATION type must be a declared notation.
        if let AttType::Notation(notations) = &def.att_type {
          for &notation in notations {
            if !self.dtd.has_notation(notation) {
              let name = self.pool.resolve(notation);
              let attr = self.pool.resolve(def.name);
              let message = format!("attribute \"{attr}\" allows notation \"{name}\", which is not declared");
              errors.push(ValidityError::new(message, at.clone()));
            }
          }
        }
        // VC: Attribute Default Value Syntactically Correct. A declared default must be a legal
        // value for its type. Check it without recording IDs or references.
        if let Some(value) = def.default.value() {
          if !self.default_value_is_valid(&def.att_type, value) {
            let name = self.pool.resolve(def.name);
            let message = format!("the default value \"{value}\" of \"{name}\" is not valid for its declared type");
            errors.push(ValidityError::new(message, at.clone()));
          }
        }
      }
      // VC: One ID per Element Type.
      if ids > 1 {
        let message = format!("element \"{element_name}\" has more than one ID attribute");
        errors.push(ValidityError::new(message, at.clone()));
      }
    }

    // VC: No Duplicate Types. A name may not repeat in an element's mixed content.
    let mixed: Vec<(NameId, Vec<NameId>)> = self
      .dtd
      .elements()
      .filter_map(|(e, spec)| match spec {
        ContentSpec::Mixed(names) => Some((e, names.clone())),
        _ => None,
      })
      .collect();
    for (element, names) in mixed {
      let mut seen = Vec::new();
      for name in names {
        if seen.contains(&name) {
          let element_name = self.pool.resolve(element);
          let child = self.pool.resolve(name);
          let message =
            format!("element \"{child}\" appears more than once in the mixed content of \"{element_name}\"");
          errors.push(ValidityError::new(message, at.clone()));
        } else {
          seen.push(name);
        }
      }
    }
  }

  /// Whether `value` is a legal value for `att_type`, for checking declared defaults.
  fn default_value_is_valid(&self, att_type: &AttType, value: &str) -> bool {
    let tokens = || value.split_whitespace();
    match att_type {
      AttType::Cdata => true,
      AttType::Id | AttType::IdRef => crate::chars::is_name(value),
      AttType::IdRefs | AttType::Entities => tokens().all(crate::chars::is_name),
      AttType::Entity => {
        matches!(self.pool.get(value).and_then(|id| self.dtd.general_entity(id)), Some(GeneralEntity::Unparsed { .. }))
      }
      AttType::Nmtoken => crate::chars::is_nmtoken(value),
      AttType::Nmtokens => tokens().all(crate::chars::is_nmtoken),
      AttType::Enumeration(allowed) | AttType::Notation(allowed) => {
        self.pool.get(value).is_some_and(|id| allowed.contains(&id))
      }
    }
  }

  /// Checks an element's children against its declared content model.
  fn check_content(&mut self, element: NameId, children: &[NameId], at: &Location, errors: &mut Vec<ValidityError>) {
    match self.dtd.content_spec(element) {
      None => {} // undeclared: already reported at the start tag
      Some(ContentSpec::Any) => {}
      Some(ContentSpec::Empty) => {
        if let Some(&child) = children.first() {
          let name = self.pool.resolve(child).to_owned();
          let element = self.pool.resolve(element).to_owned();
          self.report(errors, at, format!("element \"{element}\" is EMPTY but contains \"{name}\""));
        }
      }
      Some(ContentSpec::Mixed(allowed)) => {
        for &child in children {
          if !allowed.contains(&child) {
            let child = self.pool.resolve(child).to_owned();
            let element = self.pool.resolve(element).to_owned();
            self.report(
              errors,
              at,
              format!("element \"{child}\" may not appear in the mixed content of \"{element}\""),
            );
          }
        }
      }
      Some(ContentSpec::Children(particle)) => {
        // Compile the model once, reporting a non-deterministic one (VC: No Duplicate Types /
        // Appendix E) the first time it is used.
        let newly_compiled = !self.models.contains_key(&element);
        let model = self.models.entry(element).or_insert_with(|| Some(ContentModel::compile(particle)));
        let model = model.as_ref().expect("just inserted");
        if newly_compiled && !model.is_deterministic() {
          let name = self.pool.resolve(element).to_owned();
          self.report(errors, at, format!("the content model of \"{name}\" is not deterministic"));
        }
        let model = self.models.get(&element).and_then(Option::as_ref).expect("just inserted");
        match model.matches(children) {
          Ok(()) => {}
          Err(failure) => {
            let element = self.pool.resolve(element).to_owned();
            let allowed = names(&failure.allowed, &self.pool);
            let message = match failure.at {
              Some(child) => {
                format!(
                  "element \"{}\" is not allowed in \"{element}\" here; expected {allowed}",
                  self.pool.resolve(child)
                )
              }
              None => format!("the content of \"{element}\" is incomplete; expected {allowed}"),
            };
            self.report(errors, at, message);
          }
        }
      }
    }
  }

  /// Checks the attributes of a start tag: each declared, `#REQUIRED` present, `#FIXED` kept,
  /// enumerations and ID/IDREF/ENTITY/NOTATION values valid.
  ///
  /// A namespace declaration is checked like any other attribute. XML 1.0 validity knows nothing of namespaces, so a
  /// valid document declares `xmlns` and `xmlns:p` in its ATTLIST, as the XHTML DTD declares `xmlns` `#FIXED`.
  fn check_attributes(
    &mut self,
    element: NameId,
    element_lexical: &str,
    attributes: Attributes<'_>,
    at: &Location,
    errors: &mut Vec<ValidityError>,
  ) {
    // Copy the definitions off the DTD so the checks below may borrow `self` mutably.
    let defs = self.dtd.attlist(element).unwrap_or(&[]).to_vec();

    for attribute in attributes.iter() {
      // xml:id §4: an xml:id is an ID whether or not the DTD declares it. When the DTD does not
      // declare it, check it here (NCName and uniqueness) and do not fault it as undeclared;
      // when the DTD does declare it, the declaration below is honoured as usual, so the ID is
      // not recorded twice.
      if self.xml_id && crate::event::validate::ids::is_xml_id(&attribute) {
        let declared = self.pool.get(&attribute.lexical()).is_some_and(|id| defs.iter().any(|d| d.name == id));
        if !declared {
          crate::event::validate::ids::check_xml_id(&attribute, &mut self.ids, errors);
          continue;
        }
      }
      let lexical = attribute.lexical();
      let def = self.pool.get(&lexical).and_then(|id| defs.iter().find(|d| d.name == id));
      let Some(def) = def else {
        self.report(errors, at, format!("attribute \"{lexical}\" is not declared for \"{element_lexical}\""));
        continue;
      };

      if let DefaultDecl::Fixed(fixed) = &def.default {
        if attribute.value != fixed {
          self.report(
            errors,
            at,
            format!("attribute \"{lexical}\" is #FIXED as \"{fixed}\", but was given \"{}\"", attribute.value),
          );
        }
      }

      self.check_attribute_value(&lexical, &def.att_type, attribute.value, at, errors);
    }

    // VC: Required Attribute.
    for def in &defs {
      if matches!(def.default, DefaultDecl::Required) {
        let present = attributes.iter().any(|a| self.pool.get(&a.lexical()) == Some(def.name));
        if !present {
          let name = self.pool.resolve(def.name).to_owned();
          self.report(errors, at, format!("required attribute \"{name}\" is missing from \"{element_lexical}\""));
        }
      }
    }
  }

  /// Checks one attribute value against its declared type.
  fn check_attribute_value(
    &mut self,
    lexical: &str,
    att_type: &AttType,
    value: &str,
    at: &Location,
    errors: &mut Vec<ValidityError>,
  ) {
    match att_type {
      AttType::Cdata => {}
      AttType::Id => {
        if !crate::chars::is_name(value) {
          self.report(errors, at, format!("the ID value \"{value}\" is not a valid name"));
          return;
        }
        // VC: ID. An ID value must be unique.
        if self.ids.insert(value.to_owned(), at.clone()).is_some() {
          self.report(errors, at, format!("the ID \"{value}\" is used more than once"));
        }
      }
      AttType::IdRef => {
        if !crate::chars::is_name(value) {
          self.report(errors, at, format!("the IDREF value \"{value}\" is not a valid name"));
          return;
        }
        self.idrefs.push((value.to_owned(), at.clone()));
      }
      // `IDREFS ::= Name (S Name)*`: at least one name, each a valid name.
      AttType::IdRefs => {
        let tokens: Vec<&str> = value.split_whitespace().collect();
        if tokens.is_empty() {
          self.report(errors, at, format!("attribute \"{lexical}\" is IDREFS but has no names"));
          return;
        }
        for token in tokens {
          if crate::chars::is_name(token) {
            self.idrefs.push((token.to_owned(), at.clone()));
          } else {
            self.report(errors, at, format!("the IDREF \"{token}\" is not a valid name"));
          }
        }
      }
      AttType::Entity | AttType::Entities => {
        for token in value.split_whitespace() {
          // VC: Entity Name. The value must name an unparsed entity.
          let unparsed = self.pool.get(token).and_then(|id| self.dtd.general_entity(id));
          if !matches!(unparsed, Some(GeneralEntity::Unparsed { .. })) {
            self.report(
              errors,
              at,
              format!("attribute \"{lexical}\" refers to \"{token}\", which is not an unparsed entity"),
            );
          }
        }
      }
      // A single `NMTOKEN` is one token: whitespace inside it is not allowed.
      AttType::Nmtoken => {
        if !crate::chars::is_nmtoken(value) {
          self.report(errors, at, format!("the value \"{value}\" of \"{lexical}\" is not a single name token"));
        }
      }
      // `NMTOKENS ::= Nmtoken (S Nmtoken)*`: at least one token, each valid.
      AttType::Nmtokens => {
        let tokens: Vec<&str> = value.split_whitespace().collect();
        if tokens.is_empty() {
          self.report(errors, at, format!("attribute \"{lexical}\" is NMTOKENS but has no tokens"));
          return;
        }
        for token in tokens {
          if !crate::chars::is_nmtoken(token) {
            self.report(errors, at, format!("attribute \"{lexical}\" has \"{token}\", which is not a name token"));
          }
        }
      }
      AttType::Enumeration(allowed) | AttType::Notation(allowed) => {
        let ok = self.pool.get(value).is_some_and(|id| allowed.contains(&id));
        if !ok {
          let choices = names(allowed, &self.pool);
          self.report(errors, at, format!("attribute \"{lexical}\" is \"{value}\", not one of {choices}"));
        }
      }
    }
  }
}

/// The per-event checks. Each takes the error list rather than reaching for the field, so a check may hold a borrow of
/// this validator while it reports; [`handle`](EventHandler::handle) lends the field out and takes it back.
impl DtdValidator {
  fn start_element(
    &mut self,
    prefix: Option<&str>,
    local: &str,
    attributes: Attributes<'_>,
    at: &Location,
    errors: &mut Vec<ValidityError>,
  ) {
    if !self.declarations_checked {
      self.declarations_checked = true;
      self.check_declarations(at, errors);
    }
    // The source interned this name in its own pool, which the DTD's ids say nothing about. Interning the lexical form
    // here moves it into the one space every comparison below is made in. A name the DTD never declared still gets an
    // id, and `has_element` is what answers for that.
    let lexical = name::lexical(prefix, local);
    let element = self.pool.intern(&lexical);

    // VC: Root Element Type, when the DTD declares a root to check against.
    if !self.root_seen {
      self.root_seen = true;
      if let Some(declared) = self.root {
        if element != declared {
          let root = self.pool.resolve(declared).to_owned();
          self.report(errors, at, format!("the root element is \"{lexical}\", but the DTD declares \"{root}\""));
        }
      }
    }

    // VC: Element Valid. The element must be declared.
    if !self.dtd.has_element(element) {
      self.report(errors, at, format!("element \"{lexical}\" is used but not declared"));
    }

    // Record this element as a child of its parent, for the parent's content model.
    if let Some(parent) = self.open.last_mut() {
      parent.children.push(element);
    }

    let content = match self.dtd.content_spec(element) {
      Some(ContentSpec::Empty) => ContentKind::Empty,
      Some(ContentSpec::Children(_)) => ContentKind::ElementOnly,
      _ => ContentKind::CharacterData,
    };
    self.check_attributes(element, &lexical, attributes, at, errors);
    self.open.push(OpenElement { lexical: element, children: Vec::new(), content });
  }

  fn characters(&mut self, whitespace_only: bool, at: &Location, errors: &mut Vec<ValidityError>) {
    // VC: Element Valid. An EMPTY element admits no content at all; element content admits whitespace between its
    // children but no other character data.
    match self.open.last().map(|o| o.content) {
      Some(ContentKind::Empty) => self.report(errors, at, "an EMPTY element may not contain character data".to_owned()),
      Some(ContentKind::ElementOnly) if !whitespace_only => {
        self.report(errors, at, "character data may not appear in this element's content".to_owned());
      }
      _ => {}
    }
  }

  fn end_element(&mut self, at: &Location, errors: &mut Vec<ValidityError>) {
    // A well-formed document never closes what it did not open, so this is only reachable from a caller driving the
    // validator by hand with events of its own, or once from a parser that reported the `Doctype` event after the root
    // element's start tag, which built the validator too late to see it. Whichever it is, there is nothing to check
    // and no reason to bring the process down over it.
    let Some(open) = self.open.pop() else { return };
    self.check_content(open.lexical, &open.children, at, errors);
  }

  /// The checks that need the whole document, run from the end of it.
  fn end_document(&mut self, errors: &mut Vec<ValidityError>) {
    // VC: IDREF. Every referenced ID must have been declared somewhere in the document.
    for (value, at) in std::mem::take(&mut self.idrefs) {
      if !self.ids.contains_key(&value) {
        self.report(errors, &at, format!("IDREF \"{value}\" matches no ID in the document"));
      }
    }
  }
}

impl EventHandler for DtdValidator {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    // The list is taken out for the length of this event, so a check may borrow the validator while it reports.
    let mut errors = std::mem::take(&mut self.errors);
    match event {
      EventRef::StartElement(event) => {
        self.start_element(event.prefix, event.local, event.attributes, &event.location, &mut errors);
      }
      EventRef::EndElement(event) => self.end_element(&event.location, &mut errors),
      EventRef::Characters(event) => {
        self.characters(event.text.chars().all(chars::is_whitespace), &event.location, &mut errors);
      }
      // A CDATA section is significant character data, never ignorable whitespace.
      EventRef::Cdata(event) => self.characters(false, &event.location, &mut errors),
      EventRef::EndDocument => self.end_document(&mut errors),
      // A comment, a processing instruction, and the document's own start and doctype are not content a DTD constrains.
      EventRef::StartDocument | EventRef::Comment(_) | EventRef::ProcessingInstruction(_) | EventRef::Doctype(_) => {}
    }
    self.errors = errors;
    // A validity error is recoverable, so the document is not refused over one; the errors are read back afterwards.
    Ok(())
  }
}

impl Validator for DtdValidator {
  fn errors(&self) -> std::borrow::Cow<'_, [ValidityError]> {
    std::borrow::Cow::Borrowed(&self.errors)
  }

  fn as_event_handler(&mut self) -> &mut dyn EventHandler {
    self
  }
}

/// Renders a list of names as `"a", "b", "c"` for a message.
fn names(names: &[NameId], pool: &NamePool) -> String {
  names.iter().map(|&n| format!("\"{}\"", pool.resolve(n))).collect::<Vec<_>>().join(", ")
}

/// A DTD held as a reusable schema, to check any number of documents against.
///
/// A [`DtdValidator`] runs over one document and keeps what it gathers along the way, so a schema hands out a fresh
/// one each time. Build it from a DTD read on its own, and check whatever source the events come from: a reader over
/// a document, or a walk over a tree already in memory.
///
/// A DTD read on its own declares no root element, since it is the `DOCTYPE` in a document that declares one. The root
/// therefore goes unchecked unless [`with_root`](Self::with_root) asks for it.
///
/// # Examples
///
/// ```
/// use xenolith::dtd::DtdReader;
/// use xenolith::dtd::validate::DtdSchema;
/// use xenolith::event::validate::ValidatorSet;
/// use xenolith::event::{EventCursor, EventSource};
/// use xenolith::io::StreamSource;
///
/// let (dtd, pool) = DtdReader::new("<!ELEMENT note (#PCDATA)>".as_bytes()).read()?;
/// let schema = DtdSchema::new(dtd, pool).with_root("note");
///
/// let mut validation = ValidatorSet::new().with_schema(&schema);
/// StreamSource::new("<note>hi</note>".as_bytes()).with_handler(&mut validation).emit()?;
/// assert!(validation.report().errors().is_empty());
///
/// let mut validation = ValidatorSet::new().with_schema(&schema);
/// StreamSource::new("<other/>".as_bytes()).with_handler(&mut validation).emit()?;
/// let report = validation.report();
/// assert!(!report.errors().is_empty(), "the root is not the one asked for, and the element is not declared");
/// # Ok::<(), xenolith::Error>(())
/// ```
///
#[derive(Debug)]
pub struct DtdSchema {
  dtd: Dtd,
  pool: NamePool,
  root: Option<NameId>,
}

impl DtdSchema {
  /// Creates a schema from `dtd`, whose names are interned in `pool`.
  #[must_use]
  pub fn new(dtd: Dtd, pool: NamePool) -> Self {
    Self { dtd, pool, root: None }
  }

  /// Requires the document's root element to be `name`, the check a `DOCTYPE` would supply.
  #[must_use]
  pub fn with_root(mut self, name: &str) -> Self {
    self.root = Some(self.pool.intern(name));
    self
  }
}

impl crate::event::validate::Schema for DtdSchema {
  fn validator(&self) -> Box<dyn Validator> {
    Box::new(DtdValidator::new(self.dtd.clone(), self.pool.fork(), self.root))
  }
}
