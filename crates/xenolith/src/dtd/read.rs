//! Reads DTD declarations into a [`Dtd`].
//!
//! A DTD consists of two parts: the "internal subset," defined between `[` and `]` within the `DOCTYPE` declaration,
//! and the "external subset," a separate resource referenced by the `DOCTYPE`. These are read from a single buffer,
//! with the internal subset processed first. There are three ways to perform this reading:
//!
//! - [`DtdAssembly`] collects the subsets and any external parameter entities retrieved by the caller, then parses
//!   them. The document parser reads the `DOCTYPE` through this mechanism.
//! - [`DtdReader`] reads a DTD stored in a separate resource as an external subset.
//! - [`parse_subset`] reads a subset provided directly, without referencing external parameter entities.
//!
//! This module verifies that the declarations are well-formed and records them. It does not validate whether the
//! document conforms to the DTD (validation is handled by [`validate`](crate::dtd::validate)). The recorded
//! information consists of data required for document parsing: entity replacement texts, default attribute values,
//! and attribute values normalized as tokens.
//!
//! A parameter entity reference, `%name;`, are expanded by inserting the entity's replacement text into the buffer and
//! continuing the read process. While internal entities are inserted immediately, external entities cannot be handled
//! this way. Since the parser does not perform I/O, processing pauses and returns [`DtdOutcome::NeedExternalPe`]. The
//! caller must then retrieve and insert the text, after which the next pass resumes from the beginning of the buffer.
//! This resumption model means the parser's state does not need to be preserved during the pause; only the buffer and
//! a record of the text's origin are maintained.

mod assemble;
mod reader;

#[cfg(test)]
mod test;

pub use assemble::DtdAssembly;
pub use reader::DtdReader;

use std::ops::Range;

use crate::chars;
use crate::error::{Error, Location, Result};
use crate::name::{NameId, NamePool, XML_PREFIX};

pub use crate::dtd::model::{
  AttDef, AttType, ContentParticle, ContentSpec, DefaultDecl, Dtd, ExternalId, GeneralEntity, Occurs, ParameterEntity,
};

/// The external parameter entity encountered during DTD scanning that caused the operation to be interrupted.
///
/// The caller obtains the replacement text from [`system_id`](Self::system_id) and passes it to
/// [`DtdAssembly::provide_parameter_entity`]. This method replaces the reference within the range
/// [`at`](Self::at)`..`[`end`](Self::end) with that text. Subsequently, the next pass begins from the start.
#[derive(Clone, Debug)]
pub struct ExternalPe {
  /// The entity name (excluding `%` and `;`).
  pub name: String,
  /// The public identifier specified in the declaration, if any.
  pub public_id: Option<String>,
  /// The system identifier used to retrieve the replacement text, as described in the declaration.
  pub system_id: String,
  /// The system identifier of the resource where the entity was declared. Relative
  /// [`system_id`](Self::system_id) values are resolved against this system identifier (XML 1.0 §4.2.2). `None` if
  /// the resource has no system identifier (e.g., for documents where the system identifier is unknown).
  pub base: Option<String>,
  /// The location of the `%name;` reference, in the event of an entity retrieval error.
  pub location: Location,
  /// The byte offset within the DTD buffer where the `%name;` reference begins.
  pub at: usize,
  /// The byte offset within the DTD buffer immediately following the `%name;` reference.
  pub end: usize,
  /// The reference occurs within an entity value literal, and the replacement text is included as literal data
  /// (§4.4.5): no extra spaces are added, and quotes that do not close the literal are included.
  pub(crate) in_literal: bool,
}

/// The result from a single pass over the DTD buffer.
///
/// At most one external parameter entity is reported per pass. Scanning stops when a reference within a declaration
/// is encountered, a common scenario in external subsets. This is because the replacement text might contain `>` or
/// `<!`, making it impossible to determine the end of the declaration or the subsequent tokens until the reference has
/// been expanded.
#[derive(Debug)]
pub enum DtdOutcome {
  /// The DTD is fully parsed; it is handled as a separate boxed unit since it is much larger than the request itself.
  Complete(Box<Dtd>),
  /// An external parameter entity is required. The caller retrieves and incorporates it, then performs the parsing
  /// again.
  NeedExternalPe(ExternalPe),
}

/// The reason why DTD scanning stopped before completion.
///
/// This is a type of error in recursive descent parsing; much like a fault, a pause triggers unwinding via `?`. In
/// this process, [`parse_dtd`] converts [`Pause`](Self::Pause) into [`DtdOutcome::NeedExternalPe`] and returns
/// [`Fatal`](Self::Fatal) as the error.
enum Break {
  /// The processing was interrupted because a reference to an external parameter entity was encountered. This is not
  /// an error.
  Pause(Box<ExternalPe>),
  /// A parse error.
  Fatal(Error),
}

impl From<Error> for Break {
  fn from(error: Error) -> Self {
    Self::Fatal(error)
  }
}

/// The result of a parsing step: a value or a [`Break`] (a pause for an external PE or a failure).
type Broken<T> = std::result::Result<T, Break>;

/// The origin of the text within the DTD buffer.
///
/// This buffer holds the internal subset followed by the external subset; its contents are rewritten in place whenever
/// a parameter entity is incorporated into it. Layout information records the origin of each text fragment and is
/// shifted or updated whenever a splice occurs, ensuring that it consistently points to the same text throughout the
/// various processing stages. The parser uses this information to determine whether the text originates from an
/// external source, identify the parameter entity in which a declaration begins and ends, ascertain the declaration's
/// base URI, and track line and column numbers for specific positions.
///
/// For example, if the internal subset `<!ENTITY x "y">` and the external subset `<!ELEMENT a EMPTY>` are concatenated
/// with a newline, the buffer contents become `<!ENTITY x "y">\n<!ELEMENT a EMPTY>`, and `internal_len` becomes 15.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
  /// The byte length of the internal subset. Unless otherwise specified, the text following that position belongs to
  /// the external subset.
  internal_len: usize,
  /// Text fragments in the order they were added. Spliced fragments are located within the fragment into which they
  /// were spliced.
  pieces: Vec<Piece>,
}

/// The portion of the DTD buffer originating from a single location.
#[derive(Clone, Debug)]
struct Piece {
  /// The number of bytes occupied by that fragment (including whitespace added around the replacement text of the
  /// parameter entity).
  range: Range<usize>,
  /// The number of bytes of the fragment's text itself, excluding the added whitespace.
  content: Range<usize>,
  /// The number of characters in the `%name;` reference replaced by that fragment (0 in the case of a subset).
  replaced: usize,
  kind: PieceKind,
}

/// Where a [`Piece`] came from.
#[derive(Clone, Debug)]
enum PieceKind {
  /// A subset, read from the document or from a resource. `origin` is where its text begins.
  Subset { origin: Location, external: bool },
  /// The replacement text of an external parameter entity, read from a resource. `origin` is where its text begins.
  /// Like the external subset, it is read by the external subset's rules, and a standalone document may not depend on
  /// what it declares (XML 1.0 §2.9).
  ExternalEntity { origin: Location },
  /// The replacement text of an internal parameter entity. It has no position of its own, so a position in it is
  /// reported at the reference it replaced.
  InternalEntity,
}

impl Piece {
  /// Where the piece's text begins, for a piece read from the document or a resource.
  fn origin(&self) -> Option<&Location> {
    match &self.kind {
      PieceKind::Subset { origin, .. } | PieceKind::ExternalEntity { origin } => Some(origin),
      PieceKind::InternalEntity => None,
    }
  }

  /// True if `pos` lies in the piece's own text, or just past its end.
  fn holds(&self, pos: usize) -> bool {
    self.content.start <= pos && pos <= self.content.end
  }

  /// True if the piece's whole range lies within `outer`'s own text.
  fn within(&self, outer: &Piece) -> bool {
    outer.content.start <= self.range.start && self.range.end <= outer.content.end
  }
}

/// The innermost of `pieces`, which all hold one position. Pieces nest, so the innermost is the shortest, and among
/// pieces of one length the one added last, since a piece is added after the piece it lies within.
fn innermost<'p>(pieces: impl Iterator<Item = &'p Piece>) -> Option<&'p Piece> {
  pieces.fold(None, |best, piece| match best {
    Some(best) if best.content.len() < piece.content.len() => Some(best),
    _ => Some(piece),
  })
}

impl Layout {
  /// A layout for an internal subset `len` bytes long whose text begins at `origin`.
  pub(crate) fn with_internal_subset(len: usize, origin: Location) -> Self {
    let mut layout = Self { internal_len: len, pieces: Vec::new() };
    if len > 0 {
      let kind = PieceKind::Subset { origin, external: false };
      layout.pieces.push(Piece { range: 0..len, content: 0..len, replaced: 0, kind });
    }
    layout
  }

  /// Records the external subset at `range` of the buffer, whose text begins at `origin`.
  pub(crate) fn add_external_subset(&mut self, range: Range<usize>, origin: Location) {
    let kind = PieceKind::Subset { origin, external: true };
    self.pieces.push(Piece { range: range.clone(), content: range, replaced: 0, kind });
  }

  /// The innermost piece read from the document or a resource that holds `pos`.
  fn source_at(&self, pos: usize) -> Option<&Piece> {
    innermost(self.pieces.iter().filter(|p| p.origin().is_some() && p.holds(pos)))
  }

  /// True if the text at `pos` is external: in the external subset, or in an external parameter entity's replacement
  /// text, wherever it was referenced.
  fn is_external(&self, pos: usize) -> bool {
    match self.source_at(pos).map(|p| &p.kind) {
      Some(PieceKind::Subset { external, .. }) => *external,
      Some(PieceKind::ExternalEntity { .. }) => true,
      _ => pos >= self.internal_len,
    }
  }

  /// The own text of the innermost parameter entity that holds `pos`, if any.
  fn entity_at(&self, pos: usize) -> Option<&Range<usize>> {
    let entities = self.pieces.iter().filter(|p| !matches!(p.kind, PieceKind::Subset { .. }));
    innermost(entities.filter(|p| p.content.contains(&pos))).map(|p| &p.content)
  }

  /// The system identifier of the document or resource the text at `pos` was read from.
  fn base_at(&self, pos: usize) -> Option<String> {
    self.source_at(pos).and_then(Piece::origin).and_then(|o| o.system_id.as_deref()).map(ToOwned::to_owned)
  }

  /// The location of `pos` in the document or resource its text was read from.
  ///
  /// A position in an internal parameter entity's replacement text, or in the spaces added around a parameter entity,
  /// is reported at the reference. A text included in an entity value literal had its quotes escaped, so a column after
  /// a quote in it is off by the length of the escape.
  pub(crate) fn location_at(&self, buf: &str, pos: usize) -> Location {
    let Some(source) = self.source_at(pos) else { return Location::unknown() };
    let mut at = source.origin().cloned().unwrap_or_else(Location::unknown);
    let mut from = source.content.start;
    for child in self.children_of(source) {
      if child.range.start >= pos {
        break;
      }
      advance_over(&mut at, &buf[from..child.range.start]);
      if pos < child.range.end {
        return at;
      }
      for _ in 0..child.replaced {
        at.advance(' ');
      }
      from = child.range.end;
    }
    advance_over(&mut at, &buf[from..pos]);
    at
  }

  /// The pieces spliced directly into `parent`'s own text, in the order they stand.
  fn children_of(&self, parent: &Piece) -> Vec<&Piece> {
    let inside = |p: &Piece| !std::ptr::eq(p, parent) && p.within(parent);
    let mut children: Vec<&Piece> = self
      .pieces
      .iter()
      .filter(|p| inside(p))
      .filter(|p| !self.pieces.iter().any(|q| !std::ptr::eq(q, *p) && inside(q) && p.within(q)))
      .collect();
    children.sort_by_key(|p| p.range.start);
    children
  }

  /// Records that `range` of the buffer, `replaced` characters long, is being replaced by `new_len` bytes of a
  /// parameter entity's replacement text, whose own text occupies `content` of the replacement (the rest is the spaces
  /// XML 1.0 §4.4.8 adds). `origin` is where an external entity's text begins, or `None` for an internal entity.
  pub(crate) fn splice(
    &mut self,
    range: Range<usize>,
    new_len: usize,
    content: Range<usize>,
    replaced: usize,
    origin: Option<Location>,
  ) {
    let start = range.start;
    if start < self.internal_len {
      let removed = range.end.min(self.internal_len) - start;
      self.internal_len = self.internal_len - removed + new_len;
    }
    for piece in &mut self.pieces {
      shift(&mut piece.range, &range, new_len);
      shift(&mut piece.content, &range, new_len);
    }
    let kind = origin.map_or(PieceKind::InternalEntity, |origin| PieceKind::ExternalEntity { origin });
    let content = start + content.start..start + content.end;
    self.pieces.push(Piece { range: start..start + new_len, content, replaced, kind });
  }
}

/// Advances `at` over the characters of `text`.
fn advance_over(at: &mut Location, text: &str) {
  for c in text.chars() {
    at.advance(c);
  }
}

/// Moves `region` for the replacement of `range` in the buffer by `new_len` bytes, so that it still covers the same
/// text. A region after the range moves by the change in length, and a region that encloses the range grows or shrinks
/// by it.
fn shift(region: &mut Range<usize>, range: &Range<usize>, new_len: usize) {
  let moved = |p: usize| p + new_len - range.len();
  if region.start >= range.end {
    *region = moved(region.start)..moved(region.end);
  } else if region.end > range.start {
    // The region overlaps the range: a reference inside it was replaced.
    region.start = region.start.min(range.start);
    region.end = moved(region.end.max(range.end));
  }
}

/// Parses the DTD in `buf` in one pass, as `layout` describes it.
///
/// Internal parameter entities are spliced into `buf` as they are met, and `layout` is moved with them. At an external
/// parameter entity the pass stops with [`DtdOutcome::NeedExternalPe`], since fetching is the caller's business. The
/// caller splices the fetched text in, records it through [`Layout::splice`], and parses again from the beginning.
/// [`DtdAssembly`] does all of that.
pub(crate) fn parse_dtd(buf: &mut String, layout: &mut Layout, pool: &mut NamePool) -> Result<DtdOutcome> {
  let parser = DtdParser { pool, buf, layout, pos: 0, dtd: Dtd::default() };
  match parser.parse() {
    Ok(dtd) => Ok(DtdOutcome::Complete(Box::new(dtd))),
    Err(Break::Pause(pe)) => Ok(DtdOutcome::NeedExternalPe(*pe)),
    Err(Break::Fatal(e)) => Err(e),
  }
}

/// Parses a self-contained internal subset that references no external parameter entity.
///
/// `base` is where the subset's text begins, which errors are located from. For a subset that may reference an
/// external parameter entity, use [`DtdAssembly`].
///
/// # Errors
///
/// The parser's error if the subset is malformed, or [`Error::UnsupportedFeature`] if it references an external
/// parameter entity, which only a caller that can fetch one may resolve.
///
pub fn parse_subset(subset: &str, pool: &mut NamePool, base: Location) -> Result<Dtd> {
  let mut buf = subset.to_owned();
  let mut layout = Layout::with_internal_subset(buf.len(), base);
  match parse_dtd(&mut buf, &mut layout, pool)? {
    DtdOutcome::Complete(dtd) => Ok(*dtd),
    DtdOutcome::NeedExternalPe(pe) => {
      Err(Error::UnsupportedFeature { message: format!("needs external parameter entity \"{}\"", pe.name) })
    }
  }
}

/// The deepest a content model's parenthesized groups may nest. Real content models nest a few levels at most. The
/// parser descends one level of recursion per group, so without a bound an input like `((((...))))` could overflow the
/// stack.
///
const MAX_CONTENT_DEPTH: usize = 1024;

/// One pass of the recursive-descent parser over the DTD buffer.
struct DtdParser<'p> {
  /// The pool the declared names are interned in.
  pool: &'p mut NamePool,
  /// The DTD text, rewritten in place as parameter entities are spliced in.
  buf: &'p mut String,
  /// Where the text of `buf` came from, moved with every splice.
  layout: &'p mut Layout,
  /// The byte offset of the cursor in `buf`.
  pos: usize,
  /// The declarations read so far in this pass.
  dtd: Dtd,
}

impl DtdParser<'_> {
  /// Reads the buffer from the beginning and returns the declarations, or a [`Break`] for an external parameter entity
  /// or an error.
  ///
  fn parse(mut self) -> Broken<Dtd> {
    loop {
      self.skip_whitespace();
      self.expand_parameter_entity()?;
      self.skip_whitespace();
      let Some(c) = self.peek() else { break };
      match c {
        '<' => self.nested_markup_declaration()?,
        // A `%name;` reference between declarations was expanded above, in either subset, so any other character
        // here, a `%` not followed by a name among them, does not begin a declaration.
        _ => return Err(self.error(format!("{c:?} is not the start of a markup declaration"))),
      }
    }
    Ok(self.dtd)
  }

  /// True if byte positions `a` and `b` lie in the same innermost parameter-entity replacement, or both lie outside
  /// every one.
  fn same_pe_region(&self, a: usize, b: usize) -> bool {
    // `b` is one past the declaration's last character, so test the last character itself.
    self.layout.entity_at(a) == self.layout.entity_at(b.saturating_sub(1))
  }

  /// True when the cursor is in external text: the external subset, or the replacement text of an external parameter
  /// entity referenced from the internal subset. There, a parameter entity reference may appear within a declaration,
  /// conditional sections are allowed, and a declaration is one a standalone document may not depend on.
  ///
  fn external(&self) -> bool {
    self.layout.is_external(self.pos)
  }

  /// Expands the parameter entity references at the cursor, one after another while they follow each other.
  ///
  /// The replacement text is expanded as markup, with a space added on each side as XML 1.0 §4.4.8 requires, so
  /// `<!ELEMENT e (a%y;)>` cannot fuse `a` with the text of `y`. A reference in an entity value literal is included
  /// differently (§4.4.5), and [`entity_value`](Self::entity_value) handles it itself.
  ///
  /// It does not check whether a reference is allowed here; a caller inside a declaration calls it only in external
  /// text. An internal entity is spliced in and the cursor moved to its text; an external one stops the pass.
  ///
  fn expand_parameter_entity(&mut self) -> Broken<()> {
    while self.peek_pe_start() {
      let start = self.pos;
      self.pos += 1;
      let name = self.raw_name("parameter entity")?;
      self.expect(';')?;
      match self.dtd.parameter_entity(name).cloned() {
        Some(ParameterEntity::Internal { value }) => {
          self.splice(start..self.pos, &format!(" {value} "), 1..1 + value.len());
          self.pos = start + 1; // past the added space, to the start of the entity's text
        }
        Some(ParameterEntity::External { public_id, system_id, base }) => {
          return Err(self.pause(name, (public_id, system_id, base), start..self.pos, false));
        }
        None => {
          // A parameter entity must be declared before it is referenced (WFC: Entity Declared).
          let name = self.pool.resolve(name).to_owned();
          return Err(self.error(format!("parameter entity \"{name}\" is referenced before it is declared")));
        }
      }
    }
    Ok(())
  }

  /// True if a parameter entity reference, `%` followed by a name, begins at the cursor.
  ///
  fn peek_pe_start(&self) -> bool {
    self.rest().starts_with('%') && self.rest()[1..].starts_with(|c: char| chars::is_name_start_char(c))
  }

  /// Replaces `range` in the buffer with an internal parameter entity's `replacement`, moving the layout with it.
  /// `content` is the part of `replacement` occupied by the entity's own text (the rest is the spaces XML 1.0 §4.4.8
  /// adds).
  ///
  fn splice(&mut self, range: Range<usize>, replacement: &str, content: Range<usize>) {
    let replaced = self.buf[range.clone()].chars().count();
    self.layout.splice(range.clone(), replacement.len(), content, replaced, None);
    self.buf.replace_range(range, replacement);
  }

  /// The pause for the external parameter entity declared as `public_id`, `system_id`, and `base`, referenced as `name`
  /// at `range` of the buffer. `in_literal` says the reference stands in an entity value literal.
  fn pause(
    &self,
    name: NameId,
    entity: (Option<String>, String, Option<String>),
    range: Range<usize>,
    in_literal: bool,
  ) -> Break {
    let (public_id, system_id, base) = entity;
    Break::Pause(Box::new(ExternalPe {
      name: self.pool.resolve(name).to_owned(),
      public_id,
      system_id,
      base,
      location: self.layout.location_at(self.buf, range.start),
      at: range.start,
      end: range.end,
      in_literal,
    }))
  }

  /// Handles one of the constructs that begin with `<`, and requires it to begin and end in the same parameter entity
  /// replacement, or outside every one.
  fn nested_markup_declaration(&mut self) -> Broken<()> {
    let start = self.pos;
    self.markup_declaration()?;
    if !self.same_pe_region(start, self.pos) {
      return Err(self.error("a markup declaration begins in one parameter entity and ends in another"));
    }
    Ok(())
  }

  /// Reads one construct that begins with `<`: a declaration, a conditional section, a comment, or a processing
  /// instruction.
  fn markup_declaration(&mut self) -> Broken<()> {
    if self.consume("<!--") {
      return self.comment();
    }
    // A conditional section is allowed only in external text.
    if self.rest().starts_with("<![") {
      if !self.external() {
        return Err(self.error("a conditional section <![ may not appear in the internal subset"));
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

  /// `conditionalSect ::= includeSect | ignoreSect`, in external text only.
  ///
  /// `<![INCLUDE[ ... ]]>` reads its contents as declarations. `<![IGNORE[ ... ]]>` skips them, counting nested
  /// sections so that an inner `]]>` does not close the outer one. The keyword is often written as a parameter entity
  /// reference, `<![%draft;[`, so references are expanded before it is read.
  ///
  fn conditional_section(&mut self) -> Broken<()> {
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
      return self.include_section();
    }
    self.skip_ignored_section()
  }

  /// Reads the body of an `INCLUDE` section as declarations, through its closing `]]>`.
  ///
  fn include_section(&mut self) -> Broken<()> {
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
      self.nested_markup_declaration()?;
    }
  }

  /// Skips the body of an `IGNORE` section through its closing `]]>`, counting nested `<![ ... ]]>` sections. The body
  /// is not read otherwise, so parameter entity references in it are not expanded.
  ///
  fn skip_ignored_section(&mut self) -> Broken<()> {
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

  /// Skips a comment, after its `<!--`, through its `-->`.
  fn comment(&mut self) -> Broken<()> {
    match self.rest().find("-->") {
      Some(i) => {
        // XML 1.0 §2.5: a comment may not contain "--", nor end with "-" before its "-->".
        if self.rest()[..i].contains("--") || self.rest()[..i].ends_with('-') {
          return Err(self.error("a comment may not contain \"--\""));
        }
        self.pos += i + 3;
        Ok(())
      }
      None => Err(self.error("a comment in the DTD is not closed by \"-->\"")),
    }
  }

  /// Skips a processing instruction, after its `<?`, through its `?>`, checking its target.
  fn processing_instruction(&mut self) -> Broken<()> {
    let target_len = self.rest().find(|c: char| chars::is_whitespace(c) || c == '?').unwrap_or(self.rest().len());
    let target = self.rest()[..target_len].to_owned();
    if !chars::is_name(&target) {
      return Err(self.error(format!("{target:?} is not a valid processing instruction target")));
    }
    if target.eq_ignore_ascii_case(XML_PREFIX) {
      // `<?xml ...?>` is a declaration, not a processing instruction. A text declaration may only begin an external
      // entity, and is removed before the entity's text reaches the buffer, so one found here is misplaced.
      return Err(self.error("an XML or text declaration may not appear here"));
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
  fn entity_declaration(&mut self) -> Broken<()> {
    // Where the declaration stands decides whether a standalone document may depend on it, and what a relative system
    // identifier in it is resolved against.
    let from_external = self.external();
    let base = self.layout.base_at(self.pos);
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
        // `external_id(false)` requires a system identifier (only a notation may stop after PUBLIC).
        let system_id = id.system_id.expect("an entity's external id always carries a system identifier");
        ParameterEntity::External { public_id: id.public_id, system_id, base }
      } else {
        ParameterEntity::Internal { value: self.entity_value()? }
      };
      self.close_declaration()?;
      // A later declaration of the same parameter entity is not an error; the first is binding.
      self.dtd.declare_parameter_entity(name, entity);
      return Ok(());
    }

    let entity = if self.peek_external() {
      let id = self.external_id(false)?;
      // `external_id(false)` requires a system identifier (only a notation may stop after PUBLIC).
      let system_id = id.system_id.expect("an entity's external id always carries a system identifier");
      // `NDataDecl ::= S 'NDATA' S Name`: the whitespace before NDATA is required, so `"foo.eps"NDATA` is malformed,
      // not a parsed external entity.
      let had_whitespace = self.peek().is_some_and(chars::is_whitespace);
      self.skip_whitespace();
      if self.rest().starts_with("NDATA") {
        if !had_whitespace {
          return Err(self.error("whitespace is required before NDATA"));
        }
        self.consume("NDATA");
        self.require_whitespace("NDATA")?;
        let notation = self.name("notation")?;
        GeneralEntity::Unparsed { public_id: id.public_id, system_id, base, notation }
      } else {
        GeneralEntity::External { public_id: id.public_id, system_id, base }
      }
    } else {
      GeneralEntity::Internal { value: self.entity_value()? }
    };
    self.close_declaration()?;
    // Only the binding declaration counts: a later one in external text does not make an internal entity external.
    if from_external && self.dtd.general_entity(name).is_none() {
      self.dtd.mark_general_entity_external(name);
    }
    self.dtd.declare_general_entity(name, entity);
    Ok(())
  }

  /// `elementdecl ::= '<!ELEMENT' S Name S contentspec S? '>'`
  fn element_declaration(&mut self) -> Broken<()> {
    self.require_whitespace("<!ELEMENT")?;
    let name = self.name("element")?;
    self.require_whitespace("an element name")?;
    let spec = self.content_spec()?;
    self.close_declaration()?;
    if !self.dtd.declare_element(name, spec) {
      let name = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("element \"{name}\" is declared more than once")));
    }
    Ok(())
  }

  /// `contentspec ::= 'EMPTY' | 'ANY' | Mixed | children`
  fn content_spec(&mut self) -> Broken<ContentSpec> {
    // In external text, the whole content model may be written as a parameter entity reference.
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
    // `(` followed by `#PCDATA` is mixed content; anything else is element content.
    let after_paren = self.rest()[1..].trim_start_matches(chars::is_whitespace);
    if after_paren.starts_with("#PCDATA") {
      self.mixed_content()
    } else {
      Ok(ContentSpec::Children(self.content_particle(0)?))
    }
  }

  /// Skips whitespace between the tokens of a declaration and, in external text, expands a parameter entity reference
  /// standing there.
  ///
  fn skip_separators(&mut self) -> Broken<()> {
    self.skip_whitespace();
    if self.external() {
      self.expand_parameter_entity()?;
      self.skip_whitespace();
    }
    Ok(())
  }

  /// `Mixed ::= '(' S? '#PCDATA' (S? '|' S? Name)* S? ')*' | '(' S? '#PCDATA' S? ')'`
  fn mixed_content(&mut self) -> Broken<ContentSpec> {
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
      let _ = self.consume("*"); // optional on a bare `(#PCDATA)`
    } else if !self.consume("*") {
      return Err(self.error("mixed content with child elements must end with \")*\""));
    }
    Ok(ContentSpec::Mixed(names))
  }

  /// `cp ::= (Name | choice | seq) ('?' | '*' | '+')?`
  ///
  /// `depth` is the number of parenthesized groups enclosing this particle, bounded by [`MAX_CONTENT_DEPTH`].
  ///
  fn content_particle(&mut self, depth: usize) -> Broken<ContentParticle> {
    if depth > MAX_CONTENT_DEPTH {
      return Err(self.error(format!("a content model may not nest more than {MAX_CONTENT_DEPTH} groups deep")));
    }
    if self.external() {
      self.expand_parameter_entity()?;
    }
    if self.peek() == Some('(') {
      return self.choice_or_seq(depth);
    }
    let name = self.name("child element")?;
    Ok(ContentParticle::Name(name, self.occurrence()))
  }

  /// `choice ::= '(' S? cp ( S? '|' S? cp )+ S? ')'`,
  /// `seq ::= '(' S? cp ( S? ',' S? cp )* S? ')'`
  fn choice_or_seq(&mut self, depth: usize) -> Broken<ContentParticle> {
    self.expect('(')?;
    self.skip_separators()?;
    let mut particles = vec![self.content_particle(depth + 1)?];
    self.skip_separators()?;

    // The first separator decides whether this is a choice or a sequence, and every later one must match it.
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
          particles.push(self.content_particle(depth + 1)?);
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

  /// Reads the optional `?`, `*`, or `+` after a particle.
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
  fn attlist_declaration(&mut self) -> Broken<()> {
    let from_external = self.external();
    self.require_whitespace("<!ATTLIST")?;
    let element = self.name("element")?;
    if from_external {
      self.dtd.mark_attlist_external(element);
    }
    let mut defs = Vec::new();
    loop {
      // In external text, one or more whole attribute definitions may be written as a parameter entity reference.
      self.skip_separators()?;
      if self.consume(">") {
        break;
      }
      defs.push(self.attribute_definition()?);
    }
    // Several ATTLISTs for one element accumulate, and the first definition of an attribute is binding (XML 1.0
    // §3.3). `declare_attributes` keeps that rule.
    self.dtd.declare_attributes(element, defs);
    Ok(())
  }

  /// `AttDef ::= S Name S AttType S DefaultDecl`
  fn attribute_definition(&mut self) -> Broken<AttDef> {
    let name = self.name("attribute")?;
    self.require_whitespace("an attribute name")?;
    let att_type = self.attribute_type()?;
    self.require_whitespace("an attribute type")?;
    let default = self.default_declaration(&att_type)?;
    Ok(AttDef { name, att_type, default })
  }

  /// `AttType ::= StringType | TokenizedType | EnumeratedType`. The longer keywords are tried first, so that `IDREFS`
  /// is not read as `IDREF` followed by `S`.
  fn attribute_type(&mut self) -> Broken<AttType> {
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
  fn default_declaration(&mut self, att_type: &AttType) -> Broken<DefaultDecl> {
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
  fn notation_declaration(&mut self) -> Broken<()> {
    self.require_whitespace("<!NOTATION")?;
    let name = self.name("notation")?;
    self.require_whitespace("a notation name")?;
    let id = self.external_id(true)?;
    self.close_declaration()?;
    if !self.dtd.declare_notation(name, id) {
      let name = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("notation \"{name}\" is declared more than once")));
    }
    Ok(())
  }

  /// Reads an `ExternalID`, or a lone `PublicID` when `allow_public_only` (notations).
  ///
  fn external_id(&mut self, allow_public_only: bool) -> Broken<ExternalId> {
    if self.consume_keyword("SYSTEM") {
      self.require_whitespace("SYSTEM")?;
      let system_id = self.system_literal()?;
      return Ok(ExternalId { public_id: None, system_id: Some(system_id) });
    }
    if self.consume_keyword("PUBLIC") {
      self.require_whitespace("PUBLIC")?;
      let public_id = self.pubid_literal()?;
      // A notation may stop after the public identifier; an entity may not. Either way, a system literal must be
      // separated from it by whitespace, so `"pub""sys"` is malformed.
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

  /// Reads an `EntityValue` literal into the entity's replacement text.
  ///
  /// Character references are replaced, and general entity references are kept as written, to be expanded where the
  /// entity is used (XML 1.0 §4.5). A parameter entity reference is replaced by its replacement text, included as
  /// literal data (§4.4.5), in external text; in the internal subset it is an error (WFC: PEs in Internal Subset).
  ///
  fn entity_value(&mut self) -> Broken<String> {
    let quote = self.expect_quote()?;
    let mut out = String::new();
    loop {
      // An internal parameter entity's value is appended rather than spliced into the buffer, so a quote in it does
      // not close the literal. An external one stops the pass, and its text is spliced in as literal data: without
      // the added spaces, and with its quotes escaped.
      if self.external() && self.peek_pe_start() {
        let start = self.pos;
        self.pos += 1;
        let name = self.raw_name("parameter entity")?;
        self.expect(';')?;
        match self.dtd.parameter_entity(name).cloned() {
          Some(ParameterEntity::Internal { value }) => out.push_str(&value),
          Some(ParameterEntity::External { public_id, system_id, base }) => {
            let end = self.pos;
            self.pos = start;
            return Err(self.pause(name, (public_id, system_id, base), start..end, true));
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
        // A "%" in an entity value must begin a parameter entity reference. One in external text was handled above,
        // so a "%" here is either a reference in the internal subset or a "%" the grammar does not allow at all.
        '%' if self.external() => {
          return Err(self.error("a \"%\" in an entity value must begin a parameter-entity reference"));
        }
        '%' => return Err(self.error("a parameter-entity reference may not appear in the internal subset")),
        // A general entity reference in an entity value is bypassed (§4.4.7), and need not be declared yet.
        '&' => self.reference_in_literal(&mut out, false)?,
        _ => out.push(c),
      }
    }
  }

  /// Reads the `AttValue` literal of an attribute default, normalized as XML 1.0 §3.3.3 does for the attribute's type.
  ///
  /// White space characters written literally become spaces, and character references are replaced. General entity
  /// references are kept as written, and expanded where the default is supplied, where a tokenized value is
  /// normalized again.
  fn attribute_default_value(&mut self, att_type: &AttType) -> Broken<String> {
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
        '\t' | '\n' | '\r' => out.push(' '),
        // An entity referenced in a default must already be declared (WFC: Entity Declared).
        '&' => self.reference_in_literal(&mut out, true)?,
        _ => out.push(c),
      }
    }
    Ok(normalize_tokenized(&out, att_type.is_tokenized()))
  }

  /// Reads the reference after a consumed `&` in a literal, and appends it to `out`.
  ///
  /// A character reference is replaced by its character. A general entity reference is copied as written, to be
  /// expanded where the value is used. With `require_declared`, the entity must already be declared, as it must in an
  /// attribute default.
  ///
  fn reference_in_literal(&mut self, out: &mut String, require_declared: bool) -> Broken<()> {
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
    if require_declared && self.dtd.general_entity(name).is_none() {
      let display = self.pool.resolve(name).to_owned();
      return Err(self.error(format!("entity \"{display}\" is referenced in a default value before it is declared")));
    }
    out.push('&');
    out.push_str(self.pool.resolve(name));
    out.push(';');
    Ok(())
  }

  /// Reads the digits of a character reference, up to its `;`. They are checked by the caller.
  fn reference_digits(&mut self) -> String {
    let digits: String = self.rest().chars().take_while(|c| *c != ';').collect();
    self.pos += digits.len();
    digits
  }

  /// `'(' S? Name (S? '|' S? Name)* S? ')'`, the names of a `NOTATION` attribute type.
  fn name_group(&mut self) -> Broken<Vec<NameId>> {
    self.group(|p| p.name("notation"))
  }

  /// `'(' S? Nmtoken (S? '|' S? Nmtoken)* S? ')'`, the tokens of an enumerated attribute type.
  fn nmtoken_group(&mut self) -> Broken<Vec<NameId>> {
    self.group(|p| p.nmtoken())
  }

  /// A parenthesized, `|`-separated list of items, each read by `item`.
  fn group(&mut self, mut item: impl FnMut(&mut Self) -> Broken<NameId>) -> Broken<Vec<NameId>> {
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

  /// Reads a name, interning it. `role` names what the name is for, in an error message.
  fn name(&mut self, role: &str) -> Broken<NameId> {
    // In external text a name may be written as a parameter entity reference, such as the element name in
    // `<!ATTLIST %e; ...>`, so expand it first.
    if self.external() {
      self.expand_parameter_entity()?;
    }
    self.raw_name(role)
  }

  /// Reads a name without expanding a parameter entity reference first, as for the name inside `%name;` itself.
  ///
  fn raw_name(&mut self, role: &str) -> Broken<NameId> {
    let len = self.rest().find(|c: char| !chars::is_name_char(c)).unwrap_or(self.rest().len());
    // Copy the name out of `buf` before interning it, since the pool is borrowed through `self` mutably.
    let name = self.rest()[..len].to_owned();
    if !chars::is_name(&name) {
      return Err(self.error(format!("expected a {role} name, found {:?}", self.clip())));
    }
    self.pos += len;
    Ok(self.pool.intern(&name))
  }

  /// Reads a name token, interning it.
  fn nmtoken(&mut self) -> Broken<NameId> {
    let len = self.rest().find(|c: char| !chars::is_name_char(c)).unwrap_or(self.rest().len());
    let token = self.rest()[..len].to_owned();
    if !chars::is_nmtoken(&token) {
      return Err(self.error(format!("expected a name token, found {:?}", self.clip())));
    }
    self.pos += len;
    Ok(self.pool.intern(&token))
  }

  /// Reads a quoted `SystemLiteral`. Its content is not checked.
  fn system_literal(&mut self) -> Broken<String> {
    let quote = self.expect_quote()?;
    let value = self.until(quote)?.to_owned();
    self.pos += quote.len_utf8();
    Ok(value)
  }

  /// Reads a quoted `PubidLiteral`, whose characters are limited to `PubidChar`.
  fn pubid_literal(&mut self) -> Broken<String> {
    let quote = self.expect_quote()?;
    let value = self.until(quote)?.to_owned();
    if let Some(bad) = value.chars().find(|&c| !chars::is_pubid_char(c)) {
      return Err(self.error(format!("a public identifier may not contain {bad:?}")));
    }
    self.pos += quote.len_utf8();
    Ok(value)
  }

  /// Reads the optional whitespace and the `>` that end a declaration.
  fn close_declaration(&mut self) -> Broken<()> {
    self.skip_whitespace();
    self.expect('>')
  }

  /// Consumes `keyword` only when it is followed by whitespace, `(`, `>`, `%`, or the end of the buffer, so that `ID`
  /// does not match the start of `IDREF`.
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

  fn expect(&mut self, c: char) -> Broken<()> {
    if self.peek() == Some(c) {
      self.pos += c.len_utf8();
      Ok(())
    } else {
      Err(self.error(format!("expected {c:?}, found {:?}", self.clip())))
    }
  }

  fn expect_quote(&mut self) -> Broken<char> {
    match self.peek() {
      Some(q @ ('"' | '\'')) => {
        self.pos += 1;
        Ok(q)
      }
      _ => Err(self.error("expected a quoted value")),
    }
  }

  /// Reads everything up to `delimiter`, leaving the cursor on it. It is an error if `delimiter` does not follow.
  fn until(&mut self, delimiter: char) -> Broken<&str> {
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

  /// Requires whitespace after `after`, which names what came before it in an error message.
  fn require_whitespace(&mut self, after: &str) -> Broken<()> {
    // In external text a parameter entity reference may stand where the whitespace does: the spaces added around its
    // replacement text supply the separator.
    if self.external() && self.peek_pe_start() {
      self.expand_parameter_entity()?;
      return Ok(());
    }
    if self.peek().is_some_and(chars::is_whitespace) {
      self.skip_whitespace();
      // The next token may be written as a parameter entity reference, such as the type or default of an attribute
      // definition.
      if self.external() {
        self.expand_parameter_entity()?;
      }
      Ok(())
    } else {
      Err(self.error(format!("expected whitespace after {after}")))
    }
  }

  fn skip_whitespace(&mut self) {
    let rest = self.rest();
    self.pos += rest.find(|c: char| !chars::is_whitespace(c)).unwrap_or(rest.len());
  }

  fn peek(&self) -> Option<char> {
    self.rest().chars().next()
  }

  fn rest(&self) -> &str {
    &self.buf[self.pos..]
  }

  /// The next 16 characters, for an error message.
  fn clip(&self) -> String {
    self.rest().chars().take(16).collect()
  }

  /// A well-formedness error, located at the cursor.
  fn error(&self, message: impl Into<String>) -> Break {
    Break::Fatal(Error::well_formedness(message).at(self.layout.location_at(self.buf, self.pos)))
  }
}

/// Applies the extra normalization of a tokenized attribute value (XML 1.0 §3.3.3): leading and trailing spaces are
/// removed, and each run of spaces becomes one. With `tokenized` false, as for `CDATA`, `value` is returned unchanged.
///
/// Only spaces are collapsed: `value` is expected to have had its white space characters replaced by spaces already.
pub fn normalize_tokenized(value: &str, tokenized: bool) -> String {
  if !tokenized {
    return value.to_owned();
  }
  value.split(' ').filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}
