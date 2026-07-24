//! The DTD validator.
//!
//! It checks a document against the DTD the parser read: that the root is the one the
//! `DOCTYPE` names, that every element and attribute is declared and used as declared, that
//! content follows the declared model, and that IDs are unique and every reference resolves.
//! The parser has already done the DTD's parsing-side work — entities, defaults, tokenized
//! normalization — so this is pure constraint checking, reported through an [`ErrorListener`].

use std::collections::HashMap;
use std::ops::ControlFlow;

use xylograph_core::error::Location;
use xylograph_core::name::{NameId, NamePool, QName};
use xylograph_parser::AttributeRef;
use xylograph_parser::dtd::{AttDef, AttType, ContentSpec, DefaultDecl, Dtd, GeneralEntity};

use crate::content::ContentModel;
use crate::{ErrorListener, Validator, ValidityError};

/// A validator against a document's own DTD.
///
/// Built from the [`Dtd`] once the `DOCTYPE` is read; see [`validate`](crate::validate). It
/// owns a copy of the DTD so it can read it while the parser goes on emitting events.
#[derive(Debug)]
pub struct DtdValidator {
  dtd: Dtd,
  /// The declared root element name, matched against the document's root.
  root: NameId,
  root_seen: bool,
  /// Whether the once-per-document checks over the declarations have run.
  declarations_checked: bool,
  /// Compiled content models, by element lexical name, built on first use.
  models: HashMap<NameId, Option<ContentModel>>,
  /// The stack of open elements: lexical name and the children gathered so far.
  open: Vec<OpenElement>,
  /// Every `ID` value seen, to catch duplicates, with where it was declared.
  ids: HashMap<String, Location>,
  /// Every `IDREF` value seen and where, checked at the end against `ids`.
  idrefs: Vec<(String, Location)>,
  /// Whether `xml:id` attributes are checked as IDs (xml:id), recorded in the same `ids` space.
  #[cfg(feature = "xml-id")]
  xml_id: bool,
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
  /// Creates a validator for `dtd`, whose declared root is `root` (the `DOCTYPE` name).
  #[must_use]
  pub fn new(dtd: Dtd, root: NameId) -> Self {
    Self {
      dtd,
      root,
      root_seen: false,
      declarations_checked: false,
      models: HashMap::new(),
      open: Vec::new(),
      ids: HashMap::new(),
      idrefs: Vec::new(),
      #[cfg(feature = "xml-id")]
      xml_id: false,
    }
  }

  /// Also check `xml:id` attributes as IDs (xml:id), sharing this validator's ID space so an
  /// `xml:id` and a declared `ID` with the same value collide. Off by default.
  #[cfg(feature = "xml-id")]
  #[must_use]
  pub fn with_xml_id(mut self, on: bool) -> Self {
    self.xml_id = on;
    self
  }

  /// Reports `error`, returning whether validation should continue.
  fn report(&self, errors: &mut dyn ErrorListener, at: &Location, message: String) -> ControlFlow<()> {
    errors.report(ValidityError::new(message, at.clone()))
  }

  /// The lexical name of an element or attribute, interned once, for matching against the DTD.
  fn lexical(pool: &NamePool, name: QName) -> String {
    name.to_lexical(pool)
  }
}

impl Validator for DtdValidator {
  fn start_element(
    &mut self,
    name: QName,
    attributes: &[AttributeRef<'_>],
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    if !self.declarations_checked {
      self.declarations_checked = true;
      self.check_declarations(pool, at, errors)?;
    }
    let lexical = Self::lexical(pool, name);
    // The DTD interned its names in this same pool while the DOCTYPE was parsed, so a name it
    // never saw simply is not there.
    let element = match pool.get(&lexical) {
      Some(id) => id,
      None => {
        return self.report(errors, at, format!("element \"{lexical}\" is not declared"));
      }
    };

    // VC: Root Element Type.
    if !self.root_seen {
      self.root_seen = true;
      if element != self.root {
        let root = pool.resolve(self.root).to_owned();
        let flow = self.report(errors, at, format!("the root element is \"{lexical}\", but the DTD names \"{root}\""));
        flow?;
      }
    }

    // VC: Element Valid — the element must be declared.
    if !self.dtd.has_element(element) {
      self.report(errors, at, format!("element \"{lexical}\" is used but not declared"))?;
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
    self.check_attributes(element, &lexical, attributes, pool, at, errors)?;
    self.open.push(OpenElement { lexical: element, children: Vec::new(), content });
    ControlFlow::Continue(())
  }

  fn characters(
    &mut self,
    _text: &str,
    whitespace_only: bool,
    _pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    // VC: Element Valid. An EMPTY element admits no content at all; element content admits
    // whitespace between its children but no other character data.
    match self.open.last().map(|o| o.content) {
      Some(ContentKind::Empty) => self.report(errors, at, "an EMPTY element may not contain character data".to_owned()),
      Some(ContentKind::ElementOnly) if !whitespace_only => {
        self.report(errors, at, "character data may not appear in this element's content".to_owned())
      }
      _ => ControlFlow::Continue(()),
    }
  }

  fn end_element(
    &mut self,
    _name: QName,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    let open = self.open.pop().expect("an element was closed that was never opened");
    self.check_content(open.lexical, &open.children, pool, at, errors)
  }

  fn finish(&mut self, _pool: &NamePool, errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    // VC: IDREF — every referenced ID must have been declared somewhere in the document.
    for (value, at) in std::mem::take(&mut self.idrefs) {
      if !self.ids.contains_key(&value) {
        self.report(errors, &at, format!("IDREF \"{value}\" matches no ID in the document"))?;
      }
    }
    ControlFlow::Continue(())
  }
}

impl DtdValidator {
  /// Checks the declarations themselves, once: the constraints on an `ATTLIST` that hold
  /// regardless of any document — an ID attribute's default, one ID per element, and the
  /// syntactic validity of each declared default value.
  fn check_declarations(&self, pool: &NamePool, at: &Location, errors: &mut dyn ErrorListener) -> ControlFlow<()> {
    // Collect first so the borrow of the DTD does not outlive the reports.
    let attlists: Vec<(NameId, Vec<AttDef>)> = self.dtd.attlists().map(|(e, defs)| (e, defs.to_vec())).collect();

    for (element, defs) in attlists {
      let element_name = pool.resolve(element);
      let mut ids = 0;
      for def in &defs {
        if matches!(def.att_type, AttType::Id) {
          ids += 1;
          // VC: ID Attribute Default — an ID must be #IMPLIED or #REQUIRED.
          if matches!(def.default, DefaultDecl::Fixed(_) | DefaultDecl::Default(_)) {
            let name = pool.resolve(def.name);
            let message = format!("the ID attribute \"{name}\" of \"{element_name}\" must be #IMPLIED or #REQUIRED");
            errors.report(ValidityError::new(message, at.clone()))?;
          }
        }
        // VC: Notation Attributes — every name in a NOTATION type must be a declared notation.
        if let AttType::Notation(notations) = &def.att_type {
          for &notation in notations {
            if !self.dtd.has_notation(notation) {
              let name = pool.resolve(notation);
              let attr = pool.resolve(def.name);
              let message = format!("attribute \"{attr}\" allows notation \"{name}\", which is not declared");
              errors.report(ValidityError::new(message, at.clone()))?;
            }
          }
        }
        // VC: Attribute Default Value Syntactically Correct — a declared default must be a
        // legal value for its type. Check it without recording IDs or references.
        if let Some(value) = def.default.value() {
          if !self.default_value_is_valid(&def.att_type, value, pool) {
            let name = pool.resolve(def.name);
            let message = format!("the default value \"{value}\" of \"{name}\" is not valid for its declared type");
            errors.report(ValidityError::new(message, at.clone()))?;
          }
        }
      }
      // VC: One ID per Element Type.
      if ids > 1 {
        let message = format!("element \"{element_name}\" has more than one ID attribute");
        errors.report(ValidityError::new(message, at.clone()))?;
      }
    }

    // VC: No Duplicate Types — a name may not repeat in an element's mixed content.
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
          let element_name = pool.resolve(element);
          let child = pool.resolve(name);
          let message =
            format!("element \"{child}\" appears more than once in the mixed content of \"{element_name}\"");
          errors.report(ValidityError::new(message, at.clone()))?;
        } else {
          seen.push(name);
        }
      }
    }
    ControlFlow::Continue(())
  }

  /// Whether `value` is a legal value for `att_type`, for checking declared defaults.
  fn default_value_is_valid(&self, att_type: &AttType, value: &str, pool: &NamePool) -> bool {
    let tokens = || value.split_whitespace();
    match att_type {
      AttType::Cdata => true,
      AttType::Id | AttType::IdRef => xylograph_core::chars::is_name(value),
      AttType::IdRefs | AttType::Entities => tokens().all(xylograph_core::chars::is_name),
      AttType::Entity => {
        matches!(pool.get(value).and_then(|id| self.dtd.general_entity(id)), Some(GeneralEntity::Unparsed { .. }))
      }
      AttType::Nmtoken => xylograph_core::chars::is_nmtoken(value),
      AttType::Nmtokens => tokens().all(xylograph_core::chars::is_nmtoken),
      AttType::Enumeration(allowed) | AttType::Notation(allowed) => {
        pool.get(value).is_some_and(|id| allowed.contains(&id))
      }
    }
  }

  /// Checks an element's children against its declared content model.
  fn check_content(
    &mut self,
    element: NameId,
    children: &[NameId],
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    match self.dtd.content_spec(element) {
      None => ControlFlow::Continue(()), // undeclared: already reported at the start tag
      Some(ContentSpec::Any) => ControlFlow::Continue(()),
      Some(ContentSpec::Empty) => {
        if let Some(&child) = children.first() {
          let name = pool.resolve(child).to_owned();
          let element = pool.resolve(element).to_owned();
          return self.report(errors, at, format!("element \"{element}\" is EMPTY but contains \"{name}\""));
        }
        ControlFlow::Continue(())
      }
      Some(ContentSpec::Mixed(allowed)) => {
        for &child in children {
          if !allowed.contains(&child) {
            let child = pool.resolve(child).to_owned();
            let element = pool.resolve(element).to_owned();
            self.report(
              errors,
              at,
              format!("element \"{child}\" may not appear in the mixed content of \"{element}\""),
            )?;
          }
        }
        ControlFlow::Continue(())
      }
      Some(ContentSpec::Children(particle)) => {
        // Compile the model once, reporting a non-deterministic one (VC: No Duplicate Types /
        // Appendix E) the first time it is used.
        let newly_compiled = !self.models.contains_key(&element);
        let model = self.models.entry(element).or_insert_with(|| Some(ContentModel::compile(particle)));
        let model = model.as_ref().expect("just inserted");
        if newly_compiled && !model.is_deterministic() {
          let name = pool.resolve(element).to_owned();
          self.report(errors, at, format!("the content model of \"{name}\" is not deterministic"))?;
        }
        let model = self.models.get(&element).and_then(Option::as_ref).expect("just inserted");
        match model.matches(children) {
          Ok(()) => ControlFlow::Continue(()),
          Err(failure) => {
            let element = pool.resolve(element).to_owned();
            let allowed = names(&failure.allowed, pool);
            let message = match failure.at {
              Some(child) => {
                format!("element \"{}\" is not allowed in \"{element}\" here; expected {allowed}", pool.resolve(child))
              }
              None => format!("the content of \"{element}\" is incomplete; expected {allowed}"),
            };
            self.report(errors, at, message)
          }
        }
      }
    }
  }

  /// Checks the attributes of a start tag: each declared, `#REQUIRED` present, `#FIXED` kept,
  /// enumerations and ID/IDREF/ENTITY/NOTATION values valid.
  fn check_attributes(
    &mut self,
    element: NameId,
    element_lexical: &str,
    attributes: &[AttributeRef<'_>],
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    // Copy the definitions off the DTD so the checks below may borrow `self` mutably.
    let defs = self.dtd.attlist(element).unwrap_or(&[]).to_vec();

    for attribute in attributes {
      if attribute.declares_namespace {
        continue; // namespace declarations are not in the DTD
      }
      // xml:id §4: an xml:id is an ID whether or not the DTD declares it. When the DTD does not
      // declare it, check it here (NCName and uniqueness) and do not fault it as undeclared;
      // when the DTD does declare it, the declaration below is honoured as usual, so the ID is
      // not recorded twice.
      #[cfg(feature = "xml-id")]
      if self.xml_id && crate::ids::is_xml_id(attribute.name, pool) {
        let declared =
          pool.get(&Self::lexical(pool, attribute.name)).is_some_and(|id| defs.iter().any(|d| d.name == id));
        if !declared {
          crate::ids::check_xml_id(attribute.value, at, &mut self.ids, errors)?;
          continue;
        }
      }
      let lexical = Self::lexical(pool, attribute.name);
      let def = pool.get(&lexical).and_then(|id| defs.iter().find(|d| d.name == id));
      let Some(def) = def else {
        self.report(errors, at, format!("attribute \"{lexical}\" is not declared for \"{element_lexical}\""))?;
        continue;
      };

      if let DefaultDecl::Fixed(fixed) = &def.default {
        if attribute.value != fixed {
          self.report(
            errors,
            at,
            format!("attribute \"{lexical}\" is #FIXED as \"{fixed}\", but was given \"{}\"", attribute.value),
          )?;
        }
      }

      self.check_attribute_value(&lexical, &def.att_type, attribute.value, pool, at, errors)?;
    }

    // VC: Required Attribute.
    for def in &defs {
      if matches!(def.default, DefaultDecl::Required) {
        let present = attributes.iter().any(|a| pool.get(&Self::lexical(pool, a.name)) == Some(def.name));
        if !present {
          let name = pool.resolve(def.name).to_owned();
          self.report(errors, at, format!("required attribute \"{name}\" is missing from \"{element_lexical}\""))?;
        }
      }
    }
    ControlFlow::Continue(())
  }

  /// Checks one attribute value against its declared type.
  fn check_attribute_value(
    &mut self,
    lexical: &str,
    att_type: &AttType,
    value: &str,
    pool: &NamePool,
    at: &Location,
    errors: &mut dyn ErrorListener,
  ) -> ControlFlow<()> {
    match att_type {
      AttType::Cdata => ControlFlow::Continue(()),
      AttType::Id => {
        if !xylograph_core::chars::is_name(value) {
          return self.report(errors, at, format!("the ID value \"{value}\" is not a valid name"));
        }
        // VC: ID — an ID value must be unique.
        if self.ids.insert(value.to_owned(), at.clone()).is_some() {
          return self.report(errors, at, format!("the ID \"{value}\" is used more than once"));
        }
        ControlFlow::Continue(())
      }
      AttType::IdRef => {
        if !xylograph_core::chars::is_name(value) {
          return self.report(errors, at, format!("the IDREF value \"{value}\" is not a valid name"));
        }
        self.idrefs.push((value.to_owned(), at.clone()));
        ControlFlow::Continue(())
      }
      // `IDREFS ::= Name (S Name)*`: at least one name, each a valid name.
      AttType::IdRefs => {
        let tokens: Vec<&str> = value.split_whitespace().collect();
        if tokens.is_empty() {
          return self.report(errors, at, format!("attribute \"{lexical}\" is IDREFS but has no names"));
        }
        for token in tokens {
          if !xylograph_core::chars::is_name(token) {
            self.report(errors, at, format!("the IDREF \"{token}\" is not a valid name"))?;
          } else {
            self.idrefs.push((token.to_owned(), at.clone()));
          }
        }
        ControlFlow::Continue(())
      }
      AttType::Entity | AttType::Entities => {
        for token in value.split_whitespace() {
          // VC: Entity Name — the value must name an unparsed entity.
          let unparsed = pool.get(token).and_then(|id| self.dtd.general_entity(id));
          if !matches!(unparsed, Some(GeneralEntity::Unparsed { .. })) {
            self.report(
              errors,
              at,
              format!("attribute \"{lexical}\" names \"{token}\", which is not an unparsed entity"),
            )?;
          }
        }
        ControlFlow::Continue(())
      }
      // A single `NMTOKEN` is one token: whitespace inside it is not allowed.
      AttType::Nmtoken => {
        if !xylograph_core::chars::is_nmtoken(value) {
          return self.report(errors, at, format!("the value \"{value}\" of \"{lexical}\" is not a single name token"));
        }
        ControlFlow::Continue(())
      }
      // `NMTOKENS ::= Nmtoken (S Nmtoken)*`: at least one token, each valid.
      AttType::Nmtokens => {
        let tokens: Vec<&str> = value.split_whitespace().collect();
        if tokens.is_empty() {
          return self.report(errors, at, format!("attribute \"{lexical}\" is NMTOKENS but has no tokens"));
        }
        for token in tokens {
          if !xylograph_core::chars::is_nmtoken(token) {
            self.report(errors, at, format!("attribute \"{lexical}\" has \"{token}\", which is not a name token"))?;
          }
        }
        ControlFlow::Continue(())
      }
      AttType::Enumeration(allowed) | AttType::Notation(allowed) => {
        let ok = pool.get(value).is_some_and(|id| allowed.contains(&id));
        if !ok {
          let choices = names(allowed, pool);
          return self.report(errors, at, format!("attribute \"{lexical}\" is \"{value}\", not one of {choices}"));
        }
        ControlFlow::Continue(())
      }
    }
  }
}

/// Renders a list of names as `"a", "b", "c"` for a message.
fn names(names: &[NameId], pool: &NamePool) -> String {
  names.iter().map(|&n| format!("\"{}\"", pool.resolve(n))).collect::<Vec<_>>().join(", ")
}
