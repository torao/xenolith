//! The document type definition.
//!
//! A DTD is not validated here — that is phase 2b — but it is still needed to *parse*: it
//! declares the general entities a reference may name, the default and typed values of
//! attributes, and the notations an unparsed entity points at. Without it those are
//! guesswork, which is why even a non-validating processor reads the DTD.
//!
//! This module models a DTD and parses one from a buffer holding the internal subset followed
//! by the external subset. Names are lexical: the DTD predates Namespaces in XML and matches
//! on the qualified name as written, prefix and all, so `p:a` and `q:a` are different elements
//! to it however their prefixes are bound.
//!
//! Parameter entities are expanded by splicing their replacement text into the buffer and
//! reading on. An external one cannot be spliced synchronously, so a pass over the buffer stops
//! and asks the driver for it; the caller fetches it, splices it in, and parses the
//! buffer again from the start. Restarting each time keeps the whole pass free of state that
//! would have to survive a pause, at the cost of re-reading a small buffer.

use std::collections::HashMap;

use xylogue_core::chars;
use xylogue_core::error::{Error, Location, Result};
use xylogue_core::name::{NameId, NamePool};

/// A general entity: one that a reference in content or an attribute may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneralEntity {
  /// Declared with its replacement text inline.
  Internal {
    /// The replacement text, with character references expanded and general-entity
    /// references left in place to be expanded on use.
    value: String,
  },
  /// Declared as a separate parsed resource.
  External {
    /// The public identifier, if given.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
  },
  /// Declared as binary data of a given notation; may only be named by an `ENTITY` attribute.
  Unparsed {
    /// The public identifier, if given.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
    /// The notation naming the data's format.
    notation: NameId,
  },
}

/// A parameter entity: one referenced only inside the DTD, with `%name;`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterEntity {
  /// Declared with its replacement text inline.
  Internal {
    /// The replacement text.
    value: String,
  },
  /// Declared as a separate resource, read when the DTD is processed.
  External {
    /// The public identifier, if given.
    public_id: Option<String>,
    /// The system identifier.
    system_id: String,
  },
}

/// The declared type of an attribute (XML 1.0 §3.3.1).
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

/// An external identifier, as on a notation or an external entity.
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

/// A parsed document type definition.
///
/// The declarations are kept as read; interpreting them against a document is a validator's
/// work (`xylogue-validate`). It is [`Clone`] so a validator can own a copy and read it
/// while the parser goes on producing events.
#[derive(Clone, Debug, Default)]
pub struct Dtd {
  general: HashMap<NameId, GeneralEntity>,
  parameter: HashMap<NameId, ParameterEntity>,
  elements: HashMap<NameId, ContentSpec>,
  attlists: HashMap<NameId, Vec<AttDef>>,
  notations: HashMap<NameId, ExternalId>,
  /// General entities and elements declared in the external subset (or an external parameter
  /// entity). A `standalone="yes"` document may not depend on these, so referencing such an
  /// entity, or defaulting an attribute the external subset declared, is then a fatal error.
  external_general: std::collections::HashSet<NameId>,
  external_attlist: std::collections::HashSet<NameId>,
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

  /// True if the general entity was declared where a standalone document may not depend on it:
  /// in the external subset, or in an external parameter entity.
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
}

/// A request for an external parameter entity, raised while parsing the DTD.
///
/// The parser pauses on one of these; the driver fetches the entity, splices its content into
/// the DTD buffer at [`at`](Self::at)`..`[`end`](Self::end), and re-parses from the start.
#[derive(Clone, Debug)]
pub(crate) struct ExternalPe {
  pub name: String,
  pub public_id: Option<String>,
  pub system_id: String,
  /// Byte range of the `%name;` reference in the buffer, replaced by the fetched content.
  pub at: usize,
  pub end: usize,
}

/// The result of one pass over the DTD buffer.
pub(crate) enum DtdOutcome {
  /// The DTD is fully parsed. Boxed because a completed DTD is much larger than a request.
  Complete(Box<Dtd>),
  /// An external parameter entity is needed; fetch it, splice it in, and parse again.
  NeedExternalPe(ExternalPe),
}

/// Parses the DTD held in `buf`.
///
/// `buf` is the internal subset followed by the external subset (once fetched); `internal_len`
/// is the byte length of the internal part, which decides where the stricter internal-subset
/// rules apply. Internal parameter entities are spliced in as they are met and the pass
/// continues; an external one stops the pass with [`DtdOutcome::NeedExternalPe`], and the pass
/// is repeated after the driver splices its content in. Because each pass starts from the
/// beginning of the growing buffer, no state has to be carried across a pause.
pub(crate) fn parse_dtd(
  buf: &mut String,
  internal_len: &mut usize,
  pool: &mut NamePool,
  base: &Location,
) -> Result<DtdOutcome> {
  let mut parser = DtdParser {
    pool,
    base: base.clone(),
    buf,
    internal_len,
    pos: 0,
    dtd: Dtd::default(),
    pause: None,
    pe_regions: Vec::new(),
  };
  match parser.parse() {
    Ok(()) => Ok(DtdOutcome::Complete(Box::new(parser.dtd))),
    Err(_) if parser.pause.is_some() => Ok(DtdOutcome::NeedExternalPe(parser.pause.take().expect("just checked"))),
    Err(e) => Err(e),
  }
}

/// Parses a self-contained internal subset that references no external parameter entity.
///
/// A convenience over [`parse_dtd`] for tests and for a document whose DTD is wholly internal.
#[cfg(test)]
pub(crate) fn parse_internal_subset(subset: &str, pool: &mut NamePool, base: Location) -> Result<Dtd> {
  let mut buf = subset.to_owned();
  let mut internal_len = buf.len();
  match parse_dtd(&mut buf, &mut internal_len, pool, &base)? {
    DtdOutcome::Complete(dtd) => Ok(*dtd),
    DtdOutcome::NeedExternalPe(pe) => {
      Err(Error::UnsupportedFeature { message: format!("needs external parameter entity \"{}\"", pe.name) })
    }
  }
}

struct DtdParser<'p> {
  pool: &'p mut NamePool,
  base: Location,
  /// The DTD text, rewritten in place as parameter entities are spliced in.
  buf: &'p mut String,
  /// Byte length of the internal subset; text at or after it obeys external-subset rules.
  internal_len: &'p mut usize,
  pos: usize,
  dtd: Dtd,
  /// Set when the pass must stop to have an external parameter entity fetched.
  pause: Option<ExternalPe>,
  /// Byte ranges of parameter-entity replacement text spliced into the buffer, so that a
  /// declaration straddling one boundary — its `<!` in the replacement and its `>` outside, or
  /// the reverse — can be rejected (WFC: PE Between Declarations, Proper PE Nesting).
  pe_regions: Vec<std::ops::Range<usize>>,
}

impl DtdParser<'_> {
  fn parse(&mut self) -> Result<()> {
    loop {
      self.skip_whitespace();
      self.expand_parameter_entity()?;
      self.skip_whitespace();
      let Some(c) = self.peek() else { break };
      match c {
        '<' => {
          let start = self.pos;
          self.markup_declaration()?;
          // The whole markup declaration must lie within one parameter-entity replacement, or
          // wholly outside them all.
          if !self.same_pe_region(start, self.pos) {
            return Err(self.error("a markup declaration begins in one parameter entity and ends in another"));
          }
        }
        // In the internal subset `%` is a reference between declarations; in the external
        // subset `expand_parameter_entity` above already dealt with it, so reaching one here
        // means it stood where a declaration should.
        _ => return Err(self.error(format!("{c:?} is not the start of a markup declaration"))),
      }
    }
    Ok(())
  }

  /// True if byte positions `a` and `b` lie in the same parameter-entity replacement, or both
  /// lie outside every one.
  fn same_pe_region(&self, a: usize, b: usize) -> bool {
    let region_of = |p: usize| self.pe_regions.iter().position(|r| r.start <= p && p < r.end);
    // `b` is one past the declaration's last character, so test the last character itself.
    region_of(a) == region_of(b.saturating_sub(1))
  }

  /// True when the cursor is in the external subset, where a parameter-entity reference may
  /// appear inside a declaration and conditional sections are allowed.
  fn external(&self) -> bool {
    self.pos >= *self.internal_len
  }

  /// Expands a parameter-entity reference at the cursor, if there is one and it is allowed
  /// here. An internal entity is spliced in and the cursor rewound to read it; an external one
  /// stops the pass.
  fn expand_parameter_entity(&mut self) -> Result<()> {
    self.expand_parameter_entities(true)
  }

  /// Expands the parameter-entity references at the cursor.
  ///
  /// `spaces` adds the leading and trailing space of §4.4.8, wanted everywhere the reference
  /// stands as markup but not inside an entity value, where §4.4.5 includes it literally.
  fn expand_parameter_entities(&mut self, spaces: bool) -> Result<()> {
    while self.peek_pe_start() {
      let start = self.pos;
      self.pos += 1;
      let name = self.raw_name("parameter entity")?;
      self.expect(';')?;
      match self.dtd.parameter.get(&name).cloned() {
        Some(ParameterEntity::Internal { value }) => {
          // With the §4.4.8 spaces the entity's own text is the middle; the region excludes
          // them so the boundary check sees exactly the replacement.
          let (replacement, content) =
            if spaces { (format!(" {value} "), 1..1 + value.len()) } else { (value.clone(), 0..value.len()) };
          self.splice(start..self.pos, &replacement, content);
          self.pos = start;
        }
        Some(ParameterEntity::External { public_id, system_id }) => {
          self.pause = Some(ExternalPe {
            name: self.pool.resolve(name).to_owned(),
            public_id,
            system_id,
            at: start,
            end: self.pos,
          });
          return Err(self.pause_error());
        }
        None => {
          let name = self.pool.resolve(name).to_owned();
          return Err(self.error(format!("parameter entity \"{name}\" is referenced before it is declared")));
        }
      }
      if spaces {
        self.skip_whitespace();
      }
    }
    Ok(())
  }

  /// True if a parameter-entity reference stands at the cursor.
  fn peek_pe_start(&self) -> bool {
    self.rest().starts_with('%') && self.rest()[1..].starts_with(|c: char| chars::is_name_start_char(c))
  }

  /// Replaces `range` in the buffer with `replacement`, keeping `internal_len` and the
  /// parameter-entity regions in step. `content` marks how much of `replacement` is the
  /// entity's own text (the rest is the spaces §4.4.8 adds), which is the part that becomes a
  /// region.
  fn splice(&mut self, range: std::ops::Range<usize>, replacement: &str, content: std::ops::Range<usize>) {
    let (start, old_len, new_len) = (range.start, range.len(), replacement.len());
    if start < *self.internal_len {
      let removed = range.end.min(*self.internal_len) - start;
      *self.internal_len = *self.internal_len - removed + new_len;
    }
    // Shift the regions that follow the splice, and drop any the splice overwrote.
    let delta = new_len as isize - old_len as isize;
    self.pe_regions.retain(|r| !(r.start >= start && r.start < range.end));
    for region in &mut self.pe_regions {
      if region.start >= range.end {
        region.start = (region.start as isize + delta) as usize;
        region.end = (region.end as isize + delta) as usize;
      }
    }
    if !content.is_empty() {
      self.pe_regions.push((start + content.start)..(start + content.end));
    }
    self.buf.replace_range(range, replacement);
  }

  /// The sentinel error that unwinds a pass paused for an external parameter entity. It is
  /// never surfaced: [`parse_dtd`] turns it into [`DtdOutcome::NeedExternalPe`].
  fn pause_error(&self) -> Error {
    Error::Internal { message: "<paused for an external parameter entity>".into() }
  }

  /// Handles one of the constructs that begin with `<` in the subset.
  fn markup_declaration(&mut self) -> Result<()> {
    if self.consume("<!--") {
      return self.comment();
    }
    // A conditional section is only allowed in the external subset.
    if self.rest().starts_with("<![") {
      if !self.external() {
        return Err(self.error("a conditional section may not appear in the internal subset"));
      }
      return self.conditional_section();
    }
    if self.consume("<?") {
      return self.processing_instruction();
    }
    if self.consume("<!ENTITY") {
      return self.entity_declaration();
    }
    if self.consume("<!ELEMENT") {
      return self.element_declaration();
    }
    if self.consume("<!ATTLIST") {
      return self.attlist_declaration();
    }
    if self.consume("<!NOTATION") {
      return self.notation_declaration();
    }
    Err(self.error("expected <!ELEMENT, <!ATTLIST, <!ENTITY, <!NOTATION, a comment or a processing instruction"))
  }

  /// `conditionalSect ::= includeSect | ignoreSect` (external subset only).
  ///
  /// `<![ INCLUDE [ ... ]]>` parses its contents as DTD; `<![ IGNORE [ ... ]]>` skips them,
  /// counting nested sections so an inner `]]>` does not close the outer one. The keyword may
  /// itself have come from a parameter entity, so it is read after PE expansion.
  fn conditional_section(&mut self) -> Result<()> {
    self.pos += 3; // "<!["
    self.skip_whitespace();
    self.expand_parameter_entity()?;
    self.skip_whitespace();
    let keyword = if self.consume("INCLUDE") {
      true
    } else if self.consume("IGNORE") {
      false
    } else {
      return Err(self.error("a conditional section must be INCLUDE or IGNORE"));
    };
    self.skip_whitespace();
    if !self.consume("[") {
      return Err(self.error("expected \"[\" after INCLUDE or IGNORE"));
    }
    if keyword {
      // INCLUDE: parse the contents, then require the closing "]]>".
      return self.include_section();
    }
    self.skip_ignored_section()
  }

  /// Parses the body of an `INCLUDE` section up to its `]]>`.
  fn include_section(&mut self) -> Result<()> {
    loop {
      self.skip_whitespace();
      self.expand_parameter_entity()?;
      self.skip_whitespace();
      if self.consume("]]>") {
        return Ok(());
      }
      if self.peek().is_none() {
        return Err(self.error("an INCLUDE section is not closed by \"]]>\""));
      }
      self.markup_declaration()?;
    }
  }

  /// Skips the body of an `IGNORE` section, honouring nested `<![ ... ]]>`.
  fn skip_ignored_section(&mut self) -> Result<()> {
    let mut depth = 1usize;
    while depth > 0 {
      let rest = self.rest();
      let open = rest.find("<![");
      let close = rest.find("]]>");
      match (open, close) {
        (Some(o), Some(c)) if o < c => {
          self.pos += o + 3;
          depth += 1;
        }
        (_, Some(c)) => {
          self.pos += c + 3;
          depth -= 1;
        }
        _ => return Err(self.error("an IGNORE section is not closed by \"]]>\"")),
      }
    }
    Ok(())
  }

  fn comment(&mut self) -> Result<()> {
    match self.rest().find("-->") {
      Some(i) => {
        self.pos += i + 3;
        Ok(())
      }
      None => Err(self.error("a comment in the DTD is not closed by \"-->\"")),
    }
  }

  fn processing_instruction(&mut self) -> Result<()> {
    let target_len = self.rest().find(|c: char| chars::is_whitespace(c) || c == '?').unwrap_or(self.rest().len());
    let target = self.rest()[..target_len].to_owned();
    if !chars::is_name(&target) {
      return Err(self.error(format!("{target:?} is not a valid processing instruction target")));
    }
    if target.eq_ignore_ascii_case("xml") {
      // An `<?xml ...?>` is a declaration, and one may not sit inside the DTD.
      return Err(self.error("an XML or text declaration may not appear in the internal subset"));
    }
    match self.rest().find("?>") {
      Some(i) => {
        self.pos += i + 2;
        Ok(())
      }
      None => Err(self.error("a processing instruction in the DTD is not closed by \"?>\"")),
    }
  }

  /// `EntityDecl ::= '<!ENTITY' S ('%' S)? Name S EntityDef S? '>'`
  fn entity_declaration(&mut self) -> Result<()> {
    // Where the declaration stands decides whether a standalone document may rely on it.
    let from_external = self.external();
    self.require_whitespace("<!ENTITY")?;
    let parameter = self.consume("%");
    if parameter {
      self.require_whitespace("%")?;
    }
    let name = self.name("entity")?;
    self.require_whitespace("an entity name")?;

    if parameter {
      let entity = if self.peek_external() {
        let id = self.external_id(false)?;
        ParameterEntity::External { public_id: id.public_id, system_id: id.system_id.unwrap_or_default() }
      } else {
        ParameterEntity::Internal { value: self.entity_value()? }
      };
      self.close_declaration()?;
      // A later declaration of the same parameter entity is not an error; the first wins.
      self.dtd.parameter.entry(name).or_insert(entity);
      return Ok(());
    }

    let entity = if self.peek_external() {
      let id = self.external_id(false)?;
      let system_id = id.system_id.unwrap_or_default();
      // `NDataDecl ::= S 'NDATA' S Name`: the whitespace before NDATA is required, so
      // `"foo.eps"NDATA` — no space — is malformed, not a plain external entity.
      let had_whitespace = self.peek().is_some_and(chars::is_whitespace);
      self.skip_whitespace();
      if self.rest().starts_with("NDATA") {
        if !had_whitespace {
          return Err(self.error("whitespace is required before NDATA"));
        }
        self.consume("NDATA");
        self.require_whitespace("NDATA")?;
        let notation = self.name("notation")?;
        GeneralEntity::Unparsed { public_id: id.public_id, system_id, notation }
      } else {
        GeneralEntity::External { public_id: id.public_id, system_id }
      }
    } else {
      GeneralEntity::Internal { value: self.entity_value()? }
    };
    self.close_declaration()?;
    if from_external && !self.dtd.general.contains_key(&name) {
      self.dtd.external_general.insert(name);
    }
    self.dtd.general.entry(name).or_insert(entity);
    Ok(())
  }

  /// `elementdecl ::= '<!ELEMENT' S Name S contentspec S? '>'`
  fn element_declaration(&mut self) -> Result<()> {
    self.require_whitespace("<!ELEMENT")?;
    let name = self.name("element")?;
    self.require_whitespace("an element name")?;
    let spec = self.content_spec()?;
    self.close_declaration()?;
    if self.dtd.elements.insert(name, spec).is_some() {
      let name = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("element \"{name}\" is declared more than once")));
    }
    Ok(())
  }

  /// `contentspec ::= 'EMPTY' | 'ANY' | Mixed | children`
  fn content_spec(&mut self) -> Result<ContentSpec> {
    // A whole content model may come from a parameter entity in the external subset.
    if self.external() {
      self.expand_parameter_entity()?;
    }
    if self.consume_keyword("EMPTY") {
      return Ok(ContentSpec::Empty);
    }
    if self.consume_keyword("ANY") {
      return Ok(ContentSpec::Any);
    }
    if self.peek() != Some('(') {
      return Err(self.error("expected EMPTY, ANY, or a content model in parentheses"));
    }
    // `(` `#PCDATA` marks mixed content; anything else is an element-content particle.
    let after_paren = self.rest()[1..].trim_start_matches(chars::is_whitespace);
    if after_paren.starts_with("#PCDATA") {
      self.mixed_content()
    } else {
      Ok(ContentSpec::Children(self.content_particle()?))
    }
  }

  /// Skips whitespace and any parameter-entity references, as allowed within a content model
  /// in the external subset.
  fn skip_separators(&mut self) -> Result<()> {
    self.skip_whitespace();
    if self.external() {
      self.expand_parameter_entity()?;
      self.skip_whitespace();
    }
    Ok(())
  }

  /// `Mixed ::= '(' S? '#PCDATA' (S? '|' S? Name)* S? ')*' | '(' S? '#PCDATA' S? ')'`
  fn mixed_content(&mut self) -> Result<ContentSpec> {
    self.expect('(')?;
    self.skip_separators()?;
    if !self.consume("#PCDATA") {
      return Err(self.error("mixed content must begin with #PCDATA"));
    }
    let mut names = Vec::new();
    self.skip_separators()?;
    loop {
      match self.peek() {
        Some(')') => break,
        Some('|') => {
          self.pos += 1;
          self.skip_separators()?;
          names.push(self.name("child element")?);
          self.skip_separators()?;
        }
        _ => return Err(self.error("expected \"|\" or \")\" in mixed content")),
      }
    }
    self.expect(')')?;
    // `(#PCDATA)` may stand alone, but with any names it must be `(#PCDATA | ...)*`.
    if names.is_empty() {
      let _ = self.consume("*"); // the '*' is optional on a bare (#PCDATA)
    } else if !self.consume("*") {
      return Err(self.error("mixed content with child elements must end with \")*\""));
    }
    Ok(ContentSpec::Mixed(names))
  }

  /// `cp ::= (Name | choice | seq) ('?' | '*' | '+')?`
  fn content_particle(&mut self) -> Result<ContentParticle> {
    if self.external() {
      self.expand_parameter_entity()?;
    }
    if self.peek() == Some('(') {
      return self.choice_or_seq();
    }
    let name = self.name("child element")?;
    Ok(ContentParticle::Name(name, self.occurrence()))
  }

  /// `choice ::= '(' S? cp ( S? '|' S? cp )+ S? ')'`,
  /// `seq ::= '(' S? cp ( S? ',' S? cp )* S? ')'`
  fn choice_or_seq(&mut self) -> Result<ContentParticle> {
    self.expect('(')?;
    self.skip_separators()?;
    let mut particles = vec![self.content_particle()?];
    self.skip_separators()?;

    // The first separator fixes whether this is a choice or a sequence.
    let separator = match self.peek() {
      Some(sep @ ('|' | ',')) => sep,
      Some(')') => {
        self.pos += 1;
        // A single particle in parentheses is a sequence of one.
        return Ok(ContentParticle::Seq(particles, self.occurrence()));
      }
      _ => return Err(self.error("expected \"|\", \",\" or \")\" in a content model")),
    };
    loop {
      match self.peek() {
        Some(c) if c == separator => {
          self.pos += 1;
          self.skip_separators()?;
          particles.push(self.content_particle()?);
          self.skip_separators()?;
        }
        Some(')') => {
          self.pos += 1;
          let occurs = self.occurrence();
          return Ok(if separator == '|' {
            ContentParticle::Choice(particles, occurs)
          } else {
            ContentParticle::Seq(particles, occurs)
          });
        }
        Some('|' | ',') => return Err(self.error("a content model may not mix \"|\" and \",\"")),
        _ => return Err(self.error("expected a separator or \")\" in a content model")),
      }
    }
  }

  /// An optional `?`, `*` or `+` after a particle.
  fn occurrence(&mut self) -> Occurs {
    match self.peek() {
      Some('?') => {
        self.pos += 1;
        Occurs::Optional
      }
      Some('*') => {
        self.pos += 1;
        Occurs::ZeroOrMore
      }
      Some('+') => {
        self.pos += 1;
        Occurs::OneOrMore
      }
      _ => Occurs::Once,
    }
  }

  /// `AttlistDecl ::= '<!ATTLIST' S Name AttDef* S? '>'`
  fn attlist_declaration(&mut self) -> Result<()> {
    let from_external = self.external();
    self.require_whitespace("<!ATTLIST")?;
    let element = self.name("element")?;
    if from_external {
      self.dtd.external_attlist.insert(element);
    }
    let mut defs = Vec::new();
    loop {
      // A whole attribute definition, or several, may come from a parameter entity.
      self.skip_separators()?;
      if self.consume(">") {
        break;
      }
      defs.push(self.attribute_definition()?);
    }
    // Per the spec the first declaration of an attribute binds; later ones are ignored, and
    // several ATTLISTs for one element accumulate.
    let list = self.dtd.attlists.entry(element).or_default();
    for def in defs {
      if !list.iter().any(|existing| existing.name == def.name) {
        list.push(def);
      }
    }
    Ok(())
  }

  /// `AttDef ::= S Name S AttType S DefaultDecl`
  fn attribute_definition(&mut self) -> Result<AttDef> {
    let name = self.name("attribute")?;
    self.require_whitespace("an attribute name")?;
    let att_type = self.attribute_type()?;
    self.require_whitespace("an attribute type")?;
    let default = self.default_declaration(&att_type)?;
    Ok(AttDef { name, att_type, default })
  }

  fn attribute_type(&mut self) -> Result<AttType> {
    if self.consume_keyword("CDATA") {
      return Ok(AttType::Cdata);
    }
    if self.consume_keyword("IDREFS") {
      return Ok(AttType::IdRefs);
    }
    if self.consume_keyword("IDREF") {
      return Ok(AttType::IdRef);
    }
    if self.consume_keyword("ID") {
      return Ok(AttType::Id);
    }
    if self.consume_keyword("ENTITIES") {
      return Ok(AttType::Entities);
    }
    if self.consume_keyword("ENTITY") {
      return Ok(AttType::Entity);
    }
    if self.consume_keyword("NMTOKENS") {
      return Ok(AttType::Nmtokens);
    }
    if self.consume_keyword("NMTOKEN") {
      return Ok(AttType::Nmtoken);
    }
    if self.consume_keyword("NOTATION") {
      self.require_whitespace("NOTATION")?;
      return Ok(AttType::Notation(self.name_group()?));
    }
    if self.peek() == Some('(') {
      return Ok(AttType::Enumeration(self.nmtoken_group()?));
    }
    Err(self.error("expected an attribute type such as CDATA, ID or an enumeration"))
  }

  /// `DefaultDecl ::= '#REQUIRED' | '#IMPLIED' | (('#FIXED' S)? AttValue)`
  fn default_declaration(&mut self, att_type: &AttType) -> Result<DefaultDecl> {
    if self.consume("#REQUIRED") {
      return Ok(DefaultDecl::Required);
    }
    if self.consume("#IMPLIED") {
      return Ok(DefaultDecl::Implied);
    }
    let fixed = self.consume("#FIXED");
    if fixed {
      self.require_whitespace("#FIXED")?;
    }
    let value = self.attribute_default_value(att_type)?;
    Ok(if fixed { DefaultDecl::Fixed(value) } else { DefaultDecl::Default(value) })
  }

  /// `NotationDecl ::= '<!NOTATION' S Name S (ExternalID | PublicID) S? '>'`
  fn notation_declaration(&mut self) -> Result<()> {
    self.require_whitespace("<!NOTATION")?;
    let name = self.name("notation")?;
    self.require_whitespace("a notation name")?;
    let id = self.external_id(true)?;
    self.close_declaration()?;
    if self.dtd.notations.insert(name, id).is_some() {
      let name = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("notation \"{name}\" is declared more than once")));
    }
    Ok(())
  }

  /// Reads an `ExternalID`, or a lone `PublicID` when `allow_public_only` (notations).
  fn external_id(&mut self, allow_public_only: bool) -> Result<ExternalId> {
    if self.consume_keyword("SYSTEM") {
      self.require_whitespace("SYSTEM")?;
      let system_id = self.system_literal()?;
      return Ok(ExternalId { public_id: None, system_id: Some(system_id) });
    }
    if self.consume_keyword("PUBLIC") {
      self.require_whitespace("PUBLIC")?;
      let public_id = self.pubid_literal()?;
      // A notation may stop after the public identifier; an entity may not. Either way, a
      // system literal must be separated from it by whitespace, so `"pub""sys"` is malformed.
      let had_whitespace = self.peek().is_some_and(chars::is_whitespace);
      self.skip_whitespace();
      if allow_public_only && matches!(self.peek(), Some('>') | None) {
        return Ok(ExternalId { public_id: Some(public_id), system_id: None });
      }
      if !had_whitespace {
        return Err(self.error("whitespace is required between the public and system identifiers"));
      }
      let system_id = self.system_literal()?;
      return Ok(ExternalId { public_id: Some(public_id), system_id: Some(system_id) });
    }
    Err(self.error("expected SYSTEM or PUBLIC"))
  }

  /// True if what follows begins an external identifier rather than a literal value.
  fn peek_external(&self) -> bool {
    self.rest().starts_with("SYSTEM") || self.rest().starts_with("PUBLIC")
  }

  /// `EntityValue`, with character references expanded and general references left in place.
  ///
  /// A parameter-entity reference is included literally (§4.4.5) in the external subset, and
  /// forbidden in the internal one (WFC: PEs in Internal Subset).
  fn entity_value(&mut self) -> Result<String> {
    let quote = self.expect_quote()?;
    let mut out = String::new();
    loop {
      // A parameter entity in an entity value is included literally: append its already-
      // processed value, so a quote inside it does not close the literal (splicing it into the
      // buffer would). An external one is still spliced and fetched, which is rarer.
      if self.external() && self.peek_pe_start() {
        let start = self.pos;
        self.pos += 1;
        let name = self.raw_name("parameter entity")?;
        self.expect(';')?;
        match self.dtd.parameter.get(&name).cloned() {
          Some(ParameterEntity::Internal { value }) => out.push_str(&value),
          Some(ParameterEntity::External { public_id, system_id }) => {
            self.pos = start;
            self.pause = Some(ExternalPe {
              name: self.pool.resolve(name).to_owned(),
              public_id,
              system_id,
              at: start,
              end: start + 1 + self.pool.resolve(name).len() + 1,
            });
            return Err(self.pause_error());
          }
          None => {
            let name = self.pool.resolve(name).to_owned();
            return Err(self.error(format!("parameter entity \"{name}\" is referenced before it is declared")));
          }
        }
        continue;
      }
      let Some(c) = self.peek() else {
        return Err(self.error("an entity value is not closed"));
      };
      self.pos += c.len_utf8();
      match c {
        _ if c == quote => return Ok(out),
        '%' => return Err(self.error("a parameter-entity reference may not appear in the internal subset here")),
        // In an entity value a general reference is bypassed, and need not be declared yet.
        '&' => self.reference_in_literal(&mut out, false)?,
        _ => out.push(c),
      }
    }
  }

  /// An `AttValue` used as an attribute default, normalized by the attribute's type.
  fn attribute_default_value(&mut self, att_type: &AttType) -> Result<String> {
    let quote = self.expect_quote()?;
    let mut out = String::new();
    loop {
      let Some(c) = self.peek() else {
        return Err(self.error("an attribute default value is not closed"));
      };
      self.pos += c.len_utf8();
      match c {
        _ if c == quote => break,
        '<' => return Err(self.error("\"<\" may not appear in an attribute value")),
        '\t' | '\n' | '\r' => out.push(' '), // literal whitespace normalizes to a space
        // An entity named in a default must already be declared (WFC: Entity Declared).
        '&' => self.reference_in_literal(&mut out, true)?,
        _ => out.push(c),
      }
    }
    Ok(normalize_tokenized(&out, att_type.is_tokenized()))
  }

  /// Expands the reference beginning after a consumed `&`, appending to `out`.
  ///
  /// A character reference is expanded; a general-entity reference is copied through, to be
  /// expanded when the value is used. With `require_declared`, the general entity must already
  /// have been declared — the constraint that applies inside an attribute default.
  fn reference_in_literal(&mut self, out: &mut String, require_declared: bool) -> Result<()> {
    if self.consume("#") {
      let (digits, radix) =
        if self.consume("x") { (self.reference_digits(), 16) } else { (self.reference_digits(), 10) };
      if digits.is_empty() {
        return Err(self.error("a character reference has no digits"));
      }
      let code = u32::from_str_radix(&digits, radix).ok();
      let Some(c) = code.and_then(char::from_u32).filter(|c| chars::is_char(*c)) else {
        return Err(self.error(format!("&#{digits}; is not a character XML permits")));
      };
      self.expect(';')?;
      out.push(c);
      return Ok(());
    }
    let name = self.name("entity")?;
    self.expect(';')?;
    if require_declared && !self.dtd.general.contains_key(&name) {
      let display = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("entity \"{display}\" is referenced in a default value before it is declared")));
    }
    out.push('&');
    out.push_str(self.pool.resolve(name));
    out.push(';');
    Ok(())
  }

  fn reference_digits(&mut self) -> String {
    let digits: String = self.rest().chars().take_while(|c| *c != ';').collect();
    self.pos += digits.len();
    digits
  }

  /// `(Name (S? '|' S? Name)*)` in parentheses, for `NOTATION` types.
  fn name_group(&mut self) -> Result<Vec<NameId>> {
    self.group(|p| p.name("notation"))
  }

  /// `(Nmtoken (S? '|' S? Nmtoken)*)` in parentheses, for enumerations.
  fn nmtoken_group(&mut self) -> Result<Vec<NameId>> {
    self.group(|p| p.nmtoken())
  }

  fn group(&mut self, mut item: impl FnMut(&mut Self) -> Result<NameId>) -> Result<Vec<NameId>> {
    self.expect('(')?;
    let mut names = Vec::new();
    loop {
      self.skip_whitespace();
      names.push(item(self)?);
      self.skip_whitespace();
      match self.peek() {
        Some('|') => self.pos += 1,
        Some(')') => {
          self.pos += 1;
          return Ok(names);
        }
        _ => return Err(self.error("expected \"|\" or \")\" in a parenthesized group")),
      }
    }
  }

  // --- low-level scanning ---

  fn name(&mut self, role: &str) -> Result<NameId> {
    // In the external subset a name may have been written as a parameter entity, e.g. the
    // element name in `<!ATTLIST %e; ...>`; expand it before reading.
    if self.external() {
      self.expand_parameter_entity()?;
    }
    self.raw_name(role)
  }

  /// Reads a name without first expanding a parameter entity, for reading the name *of* one.
  fn raw_name(&mut self, role: &str) -> Result<NameId> {
    let len = self.rest().find(|c: char| !chars::is_name_char(c)).unwrap_or(self.rest().len());
    // Copy off `buf` before touching the pool, which borrows `self` mutably.
    let name = self.rest()[..len].to_owned();
    if !chars::is_name(&name) {
      return Err(self.error(format!("expected a {role} name, found {:?}", self.clip())));
    }
    self.pos += len;
    Ok(self.pool.intern(&name))
  }

  fn nmtoken(&mut self) -> Result<NameId> {
    let len = self.rest().find(|c: char| !chars::is_name_char(c)).unwrap_or(self.rest().len());
    let token = self.rest()[..len].to_owned();
    if !chars::is_nmtoken(&token) {
      return Err(self.error(format!("expected a name token, found {:?}", self.clip())));
    }
    self.pos += len;
    Ok(self.pool.intern(&token))
  }

  fn system_literal(&mut self) -> Result<String> {
    let quote = self.expect_quote()?;
    let value = self.until(quote)?.to_owned();
    self.pos += quote.len_utf8();
    Ok(value)
  }

  fn pubid_literal(&mut self) -> Result<String> {
    let quote = self.expect_quote()?;
    let value = self.until(quote)?.to_owned();
    if !chars::is_pubid_literal(&value) {
      return Err(self.error("a public identifier contains a character it may not"));
    }
    self.pos += quote.len_utf8();
    Ok(value)
  }

  fn close_declaration(&mut self) -> Result<()> {
    self.skip_whitespace();
    self.expect('>')
  }

  /// Consumes a keyword only when it is followed by whitespace or a delimiter, so that
  /// `ID` does not match the start of `IDREF`.
  fn consume_keyword(&mut self, keyword: &str) -> bool {
    let rest = self.rest();
    if let Some(after) = rest.strip_prefix(keyword) {
      if after.starts_with(|c: char| chars::is_whitespace(c) || matches!(c, '(' | '>' | '%')) || after.is_empty() {
        self.pos += keyword.len();
        return true;
      }
    }
    false
  }

  fn consume(&mut self, literal: &str) -> bool {
    if self.rest().starts_with(literal) {
      self.pos += literal.len();
      true
    } else {
      false
    }
  }

  fn expect(&mut self, c: char) -> Result<()> {
    if self.peek() == Some(c) {
      self.pos += c.len_utf8();
      Ok(())
    } else {
      Err(self.error(format!("expected {c:?}, found {:?}", self.clip())))
    }
  }

  fn expect_quote(&mut self) -> Result<char> {
    match self.peek() {
      Some(q @ ('"' | '\'')) => {
        self.pos += 1;
        Ok(q)
      }
      _ => Err(self.error("expected a quoted value")),
    }
  }

  /// Everything up to `delimiter`, leaving the cursor on it. Errors if it is absent.
  fn until(&mut self, delimiter: char) -> Result<&str> {
    let rest = self.rest();
    match rest.find(delimiter) {
      Some(i) => {
        let value = &self.buf[self.pos..self.pos + i];
        self.pos += i;
        Ok(value)
      }
      None => Err(self.error(format!("expected {delimiter:?} before the end of the DTD"))),
    }
  }

  fn require_whitespace(&mut self, after: &str) -> Result<()> {
    // A parameter-entity reference may also stand where whitespace does, and its markup form
    // supplies the required separator, so it is accepted here in the external subset.
    if self.external() && self.peek_pe_start() {
      self.expand_parameter_entity()?;
      return Ok(());
    }
    if self.peek().is_some_and(chars::is_whitespace) {
      self.skip_whitespace();
      // The next token may itself be a parameter entity, e.g. the type or default in an
      // attribute definition.
      if self.external() {
        self.expand_parameter_entity()?;
      }
      Ok(())
    } else {
      Err(self.error(format!("expected whitespace after {after}")))
    }
  }

  fn skip_whitespace(&mut self) {
    let len = self.rest().len() - self.rest().trim_start_matches(chars::is_whitespace).len();
    self.pos += len;
  }

  fn peek(&self) -> Option<char> {
    self.rest().chars().next()
  }

  fn rest(&self) -> &str {
    &self.buf[self.pos..]
  }

  /// A short excerpt of what remains, for an error message.
  fn clip(&self) -> String {
    self.rest().chars().take(16).collect()
  }

  fn error(&self, message: impl Into<String>) -> Error {
    Error::well_formedness(message).at(self.base.clone())
  }
}

/// Collapses whitespace as tokenized-attribute normalization requires (XML 1.0 §3.3.3):
/// leading and trailing spaces removed, runs of spaces reduced to one. `CDATA` skips this.
pub(crate) fn normalize_tokenized(value: &str, tokenized: bool) -> String {
  if !tokenized {
    return value.to_owned();
  }
  value.split(' ').filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(subset: &str) -> Result<(Dtd, NamePool)> {
    let mut pool = NamePool::new();
    let dtd = parse_internal_subset(subset, &mut pool, Location::unknown())?;
    Ok((dtd, pool))
  }

  fn general(subset: &str, name: &str) -> GeneralEntity {
    let (dtd, mut pool) = parse(subset).expect("parses");
    dtd.general_entity(pool.intern(name)).expect("declared").clone()
  }

  #[test]
  fn reads_internal_general_entities() {
    assert_eq!(
      general("<!ENTITY greeting \"hello\">", "greeting"),
      GeneralEntity::Internal { value: "hello".to_owned() }
    );
  }

  #[test]
  fn expands_character_references_but_keeps_general_ones_in_entity_values() {
    // &#60; becomes '<' now; &other; stays for expansion on use.
    assert_eq!(
      general("<!ENTITY e \"a&#60;b&other;c\">", "e"),
      GeneralEntity::Internal { value: "a<b&other;c".to_owned() }
    );
  }

  #[test]
  fn reads_external_and_unparsed_entities() {
    let (dtd, mut pool) = parse("<!ENTITY logo SYSTEM \"logo.png\" NDATA png>").unwrap();
    let logo = dtd.general_entity(pool.intern("logo")).unwrap().clone();
    let GeneralEntity::Unparsed { system_id, notation, .. } = logo else { panic!("expected an unparsed entity") };
    assert_eq!(system_id, "logo.png");
    assert_eq!(pool.resolve(notation), "png");

    assert!(matches!(
      general("<!ENTITY chap SYSTEM \"chap1.xml\">", "chap"),
      GeneralEntity::External { system_id, .. } if system_id == "chap1.xml"
    ));
    assert!(matches!(
      general("<!ENTITY chap PUBLIC \"-//x//y\" \"chap1.xml\">", "chap"),
      GeneralEntity::External { public_id: Some(p), .. } if p == "-//x//y"
    ));
  }

  #[test]
  fn expands_internal_parameter_entities_between_declarations() {
    let subset = "<!ENTITY % common \"<!ENTITY shared 'value'>\"> %common;";
    assert_eq!(general(subset, "shared"), GeneralEntity::Internal { value: "value".to_owned() });
  }

  #[test]
  fn reads_element_and_attlist_declarations() {
    let subset = "<!ELEMENT note (to, from, body)>\
                  <!ATTLIST note id ID #REQUIRED priority (high|low) \"low\" lang CDATA #IMPLIED>";
    let (dtd, mut pool) = parse(subset).unwrap();
    let note = pool.intern("note");
    let (to, from, body) = (pool.intern("to"), pool.intern("from"), pool.intern("body"));
    assert_eq!(
      dtd.elements.get(&note),
      Some(&ContentSpec::Children(ContentParticle::Seq(
        vec![
          ContentParticle::Name(to, Occurs::Once),
          ContentParticle::Name(from, Occurs::Once),
          ContentParticle::Name(body, Occurs::Once),
        ],
        Occurs::Once,
      )))
    );

    let attlist = dtd.attlist(note).expect("has an attlist");
    assert_eq!(attlist.len(), 3);
    assert_eq!(attlist[0].name, pool.intern("id"));
    assert_eq!(attlist[0].att_type, AttType::Id);
    assert_eq!(attlist[0].default, DefaultDecl::Required);
    assert_eq!(attlist[1].att_type, AttType::Enumeration(vec![pool.intern("high"), pool.intern("low")]));
    assert_eq!(attlist[1].default, DefaultDecl::Default("low".to_owned()));
    assert_eq!(attlist[2].default, DefaultDecl::Implied);
  }

  #[test]
  fn tokenized_defaults_are_whitespace_collapsed() {
    let (dtd, mut pool) = parse("<!ATTLIST e refs IDREFS \"  a   b  \">").unwrap();
    let attlist = dtd.attlist(pool.intern("e")).unwrap();
    assert_eq!(attlist[0].default, DefaultDecl::Default("a b".to_owned()));
  }

  #[test]
  fn a_cdata_default_keeps_its_whitespace() {
    let (dtd, mut pool) = parse("<!ATTLIST e note CDATA \"  spaced  out  \">").unwrap();
    let attlist = dtd.attlist(pool.intern("e")).unwrap();
    // Literal whitespace still normalizes to spaces, but runs are not collapsed for CDATA.
    assert_eq!(attlist[0].default, DefaultDecl::Default("  spaced  out  ".to_owned()));
  }

  #[test]
  fn reads_notation_types_and_declarations() {
    let subset = "<!NOTATION png SYSTEM \"image/png\"><!ATTLIST img type NOTATION (png) #IMPLIED>";
    let (dtd, mut pool) = parse(subset).unwrap();
    let attlist = dtd.attlist(pool.intern("img")).unwrap();
    assert_eq!(attlist[0].att_type, AttType::Notation(vec![pool.intern("png")]));
  }

  #[test]
  fn comments_and_processing_instructions_are_skipped() {
    let subset = "<!-- a comment --><?pi data?><!ENTITY e \"v\">";
    assert_eq!(general(subset, "e"), GeneralEntity::Internal { value: "v".to_owned() });
  }

  #[test]
  fn keyword_scanning_does_not_confuse_prefixes() {
    // ID must not swallow the start of IDREF, nor ENTITY the start of ENTITIES.
    let (dtd, mut pool) = parse("<!ATTLIST e a IDREF #IMPLIED b ENTITIES #IMPLIED>").unwrap();
    let attlist = dtd.attlist(pool.intern("e")).unwrap();
    assert_eq!(attlist[0].att_type, AttType::IdRef);
    assert_eq!(attlist[1].att_type, AttType::Entities);
  }

  #[test]
  fn parses_content_models_and_rejects_malformed_ones() {
    // Every well-formed shape is accepted.
    for spec in ["EMPTY", "ANY", "(#PCDATA)", "(#PCDATA|a|b)*", "(a)", "(a,b,c)", "(a|b|c)", "(a?,(b|c)+)*", "(a,b)?"] {
      assert!(parse(&format!("<!ELEMENT e {spec}>")).is_ok(), "{spec} should parse");
    }
    // Malformed content models are rejected, where phase 1 kept them as opaque text.
    for spec in ["(a,b|c)", "(a,)", "(|a)", "(a b)", "(#PCDATA|a)", "(a", "()", "(#PCDATA,a)*"] {
      assert!(parse(&format!("<!ELEMENT e {spec}>")).is_err(), "{spec} should be rejected");
    }
  }

  #[test]
  fn rejects_malformed_declarations() {
    assert!(parse("<!ENTITY>").is_err());
    assert!(parse("<!ELEMENT e>").is_err(), "no content spec");
    assert!(parse("<!ATTLIST e a>").is_err(), "no type");
    assert!(parse("<!WRONG e>").is_err());
    assert!(parse("<!ENTITY e \"unclosed>").is_err());
    assert!(parse("<!ENTITY % p \"v\"> %undeclared;").is_err());
  }

  #[test]
  fn rejects_a_parameter_entity_reference_inside_a_declaration_in_the_internal_subset() {
    // WFC: PEs in Internal Subset.
    assert!(parse("<!ENTITY % p \"CDATA\"><!ATTLIST e a %p; #IMPLIED>").is_err());
  }
}
