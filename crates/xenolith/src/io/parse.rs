//! The parser core with a sans-I/O.
//!
//! The [`Parser`] accepts a byte sequence via the [`feed`](Parser::feed) method and advances processing step-by-step
//! using the [`advance`](Parser::advance) method. `advance` returns a [`Progress`] value that instructs the caller on
//! the next action to take. Since the parser does not perform file reads or socket operations, the same core can be
//! used with blocking I/O, asynchronous I/O, or in-memory slices (byte sequences); it also allows processing to be
//! suspended between tokens without holding a thread.
//!
//! The caller accesses event data via an accessor borrowed from the parser, rather than through an event object that
//! owns the data itself. Consequently, once the buffer has been expanded to the necessary size, no new memory
//! allocations occur for individual events.
//!
//! As this is a low-level component, [`StreamSource`](crate::io::StreamSource) is provided as a wrapper that adds the
//! I/O handling and entity resolution loops typically required by users.

pub mod config;
pub mod entity;
mod namespace;
mod scan;

#[cfg(test)]
mod test;

pub use config::{DocumentLimits, EntityLimits, Extensions, Limits, ParserConfig, TokenLimits};
pub use entity::{Entity, EntityKind, EntityStack};

use std::borrow::Cow;
use std::ops::Range;

use crate::attr::{AttributeList, AttributeRef};
use crate::chars;
use crate::error::{Error, Location, Result};
use crate::name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XML_PREFIX, XMLNS_NS_URI, XMLNS_PREFIX};
use crate::uri::UriReference;

use crate::dtd::read::{self as dtd, Dtd, DtdAssembly, GeneralEntity};
use crate::io::decl::{self, TextDecl};
use crate::io::parse::namespace::NamespaceScope;
use crate::io::parse::scan::{Scan, scan};
use crate::io::resolve::{EntityRequest, RequestKind};
use crate::io::stream::CharStream;

/// What a call to [`Parser::advance`] achieved.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Progress {
  /// The parser produced a token. Read its data through [`token_ref`](Parser::token_ref), or through the accessors the
  /// [`TokenKind`] points to; the values belong to this token only, and the next [`advance`](Parser::advance) clears
  /// them.
  Token(TokenKind),

  /// The parser needs more bytes to decide the next step. The driver supplies them with [`feed`](Parser::feed), setting
  /// `last` on the final chunk, then calls again.
  NeedMoreInput,

  /// An external entity must be resolved before parsing can continue.
  ///
  /// Only external entities stop the parser here: an external general entity referenced in content, the external DTD
  /// subset, and an external parameter entity. Internal entities and character or predefined references (`&amp;`,
  /// `&#65;`) are resolved in place without an event.
  ///
  /// The driver reads the request (its name, identifiers, and [`RequestKind`]) with
  /// [`Parser::pending_entity`], fetches its bytes, gives the parser exactly one answer, then calls
  /// [`advance`](Parser::advance) again:
  ///
  /// - The driver streams a general entity with [`begin_entity`](Parser::begin_entity) then [`feed`](Parser::feed) in
  ///   chunks, which bounds memory, or hands it over whole with [`provide_entity`](Parser::provide_entity).
  /// - The external subset and an external parameter entity have no streaming form, so the driver supplies them through
  ///   [`provide_entity`](Parser::provide_entity) only.
  /// - The driver refuses an entity it cannot fetch with [`decline_entity`](Parser::decline_entity).
  ///
  /// A blocking driver does all this through a [`UriResolver`](crate::io::resolve::UriResolver); most callers use a
  /// [`StreamSource`](crate::io::StreamSource) and never see this variant.
  NeedEntity,

  /// The document is complete; no more events follow, and the driver stops.
  Eof,
}

/// The kind of token the parser is reporting, carried by [`Progress::Token`].
///
/// It carries only the kind, none of the token's data and so no borrow of the parser; that is what lets
/// [`advance`](Parser::advance) report it by value while the caller stays free to [`feed`](Parser::feed) more input.
/// The data is read separately through [`Parser::token_ref`], whose [`TokenRef`] variant matches the kind named here,
/// and each variant below points at that counterpart. Those borrowed values are current only until the next
/// [`advance`](Parser::advance).
///
/// The scanner that finds where each token ends reports these same kinds, with one exception: it never reports an
/// [`XmlDeclaration`](Self::XmlDeclaration), which is lexically a processing instruction that the parser tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TokenKind {
  /// The XML declaration: its version, encoding, and standalone flag are in [`TokenRef::XmlDeclaration`].
  XmlDeclaration,

  /// A document type declaration: its verbatim text is [`TokenRef::Doctype`], and its parsed root name and external
  /// identifiers are on the parser ([`doctype_name`](Parser::doctype_name),
  /// [`doctype_public_id`](Parser::doctype_public_id), [`doctype_system_id`](Parser::doctype_system_id)). The internal
  /// subset is parsed into the DTD rather than reported here.
  Doctype,

  /// The start of an element, or a whole empty element: its name and attributes are in [`TokenRef::StartElement`].
  StartElement,

  /// The end of an element, including the implied end of an empty element: its name is in [`TokenRef::EndElement`].
  EndElement,

  /// Character data, in [`TokenRef::Text`].
  ///
  /// One run of character data is not always one event: a long run is delivered as several adjacent `Text` events so
  /// it is not buffered without bound, and a reference or entity boundary within a run also splits it. A consumer that
  /// wants one maximal text node coalesces adjacent `Text` events, as the DOM tree builder does.
  Text,

  /// The content of a CDATA section, in [`TokenRef::CData`]. Reported separately from text because the DOM and the
  /// serializer both need to know where the section boundaries were.
  CData,

  /// A comment without its delimiters, in [`TokenRef::Comment`].
  Comment,

  /// A processing instruction: its target and data are in [`TokenRef::ProcessingInstruction`].
  ProcessingInstruction,
}

/// The current token's data, as an enum that borrows it from the parser. Each variant provides only the data specific
/// to that token type.
///
/// [`Parser::token_ref`] returns it after [`advance`](Parser::advance) reports [`Progress::Token`]. Match it for the
/// kind that was reported, or, to reach across kinds without a `match`, use [`name`](Self::name), [`text`](Self::text),
/// and [`attributes`](Self::attributes). The borrows are valid only until the next [`advance`](Parser::advance), so a
/// consumer that needs something after that copies the part it wants.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum TokenRef<'a> {
  /// The XML declaration.
  XmlDeclaration {
    /// The version, always `1.x`.
    version: &'a str,
    /// The encoding the declaration named, which need not be the one actually in use; it is the value as written, not
    /// normalized. [`Parser::encoding`] reports the encoding that actually decoded the bytes.
    encoding: Option<&'a str>,
    /// The standalone declaration, if there was one.
    standalone: Option<bool>,
  },
  /// A document type declaration, the whole `<!DOCTYPE ...>` text held verbatim; its parsed pieces are on the parser
  /// ([`doctype_name`](Parser::doctype_name) and the rest).
  Doctype(&'a str),
  /// The start of an element, or a whole empty element.
  StartElement {
    /// The element's name.
    name: QName,
    /// The attributes, in document order, namespace declarations included.
    attributes: Attributes<'a>,
    /// The `xml:space` in effect inside this element.
    xml_space: XmlSpace,
    /// The `xml:lang` in effect inside this element, if any.
    xml_lang: Option<&'a str>,
  },
  /// The end of an element, including the implied end of an empty one.
  EndElement {
    /// The element's name.
    name: QName,
  },
  /// Character data, with references expanded. One run may arrive as several adjacent `Text` events.
  Text(&'a str),
  /// The content of a CDATA section: everything between `<![CDATA[` and `]]>`, with no reference expansion and nothing
  /// trimmed.
  CData(&'a str),
  /// A comment's text: everything between `<!--` and `-->`, verbatim.
  Comment(&'a str),
  /// A processing instruction.
  ProcessingInstruction {
    /// The target: the name right after `<?`, ending at the first whitespace, or at `?>` when there is no data.
    target: &'a str,
    /// Everything after the target and the whitespace separating it, up to `?>`: the separating whitespace is dropped,
    /// nothing else is trimmed, and it is empty when the instruction is only a target.
    data: &'a str,
    /// Where `data` begins in the source, so a position inside foreign-language data maps back to the document. It is
    /// the anchor the separator whitespace would otherwise have hidden.
    data_location: &'a Location,
  },
}

impl<'a> TokenRef<'a> {
  /// Which kind of event this is.
  #[must_use]
  pub const fn kind(&self) -> TokenKind {
    match self {
      Self::XmlDeclaration { .. } => TokenKind::XmlDeclaration,
      Self::Doctype(_) => TokenKind::Doctype,
      Self::StartElement { .. } => TokenKind::StartElement,
      Self::EndElement { .. } => TokenKind::EndElement,
      Self::Text(_) => TokenKind::Text,
      Self::CData(_) => TokenKind::CData,
      Self::Comment(_) => TokenKind::Comment,
      Self::ProcessingInstruction { .. } => TokenKind::ProcessingInstruction,
    }
  }

  /// The element's name for a start or end element, or `None` for other kinds.
  ///
  /// For a start element it is also in the [`name`](Self::StartElement) field, and for an end element in the
  /// [`name`](Self::EndElement) field; this reaches whichever of the two applies without a `match`.
  #[must_use]
  pub const fn name(&self) -> Option<QName> {
    match self {
      Self::StartElement { name, .. } | Self::EndElement { name } => Some(*name),
      _ => None,
    }
  }

  /// The character data of a text, CDATA, or comment event, or `None` for other kinds.
  ///
  /// It does not cover a processing instruction's data or a `DOCTYPE`'s body, which are not character data; read those
  /// from the [`data`](Self::ProcessingInstruction) field and the [`Doctype`](Self::Doctype) variant.
  #[must_use]
  pub const fn text(&self) -> Option<&'a str> {
    match self {
      Self::Text(text) | Self::CData(text) | Self::Comment(text) => Some(text),
      _ => None,
    }
  }

  /// The attributes of a start element, in document order and namespace declarations included, or an empty view for
  /// other kinds.
  #[must_use]
  pub fn attributes(&self) -> Attributes<'a> {
    match self {
      Self::StartElement { attributes, .. } => *attributes,
      _ => Attributes { attributes: &[], text: "", pool: None },
    }
  }
}

pub use crate::event::XmlSpace;

/// The attributes of a start element, a borrowing view that yields [`AttributeRef`]. [`TokenRef::StartElement`]
/// carries one; iterate it with [`iter`](Self::iter) or index it with [`get`](Self::get).
///
/// It implements [`AttributeList`], so a source-independent consumer, a validator or a push handler, reads it through
/// an [`Attributes`](crate::attr::Attributes) view.
#[derive(Clone, Copy, Debug)]
pub struct Attributes<'a> {
  attributes: &'a [Attribute],
  text: &'a str,
  /// The pool the names are interned in, for lending each as its parts. An empty view has no names to lend and so
  /// carries none.
  pool: Option<&'a NamePool>,
}

/// The parser presents its current event's attributes, so an event built from it can borrow them for as long as the
/// parser is borrowed rather than for as long as some local view of them lives. That is what lets a cursor hand out an
/// event that outlives the call which built it.
impl AttributeList for Parser {
  fn len(&self) -> usize {
    self.attributes.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    self.attributes.get(index).map(|a| AttributeRef {
      prefix: a.name.prefix.map(|prefix| self.pool.resolve(prefix)),
      local: self.pool.resolve(a.name.local()),
      namespace: a.name.namespace().map(|namespace| self.pool.resolve(namespace)),
      value: &self.attribute_text[a.value.clone()],
      location: a.at.clone(),
      value_location: a.value_at.clone(),
    })
  }
}

impl AttributeList for Attributes<'_> {
  fn len(&self) -> usize {
    self.attributes.len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    Attributes::get(self, index)
  }
}

impl<'a> Attributes<'a> {
  /// How many attributes there are, namespace declarations included.
  #[must_use]
  pub const fn len(&self) -> usize {
    self.attributes.len()
  }

  /// Whether there are no attributes.
  #[must_use]
  pub const fn is_empty(&self) -> bool {
    self.attributes.is_empty()
  }

  /// The attribute at `index`, or `None` if `index` is out of range.
  #[must_use]
  pub fn get(&self, index: usize) -> Option<AttributeRef<'a>> {
    let pool = self.pool?;
    self.attributes.get(index).map(|a| AttributeRef {
      prefix: a.name.prefix.map(|prefix| pool.resolve(prefix)),
      local: pool.resolve(a.name.local()),
      namespace: a.name.namespace().map(|namespace| pool.resolve(namespace)),
      value: &self.text[a.value.clone()],
      location: a.at.clone(),
      value_location: a.value_at.clone(),
    })
  }

  /// Iterates the attributes in document order.
  pub fn iter(&self) -> impl Iterator<Item = AttributeRef<'a>> {
    let (attributes, text, pool) = (self.attributes, self.text, self.pool);
    (0..attributes.len()).filter_map(move |i| {
      // A view with an attribute in it carries the pool those names are interned in.
      let pool = pool.expect("the attributes were lent with their pool");
      attributes.get(i).map(|a| AttributeRef {
        prefix: a.name.prefix.map(|prefix| pool.resolve(prefix)),
        local: pool.resolve(a.name.local()),
        namespace: a.name.namespace().map(|namespace| pool.resolve(namespace)),
        value: &text[a.value.clone()],
        location: a.at.clone(),
        value_location: a.value_at.clone(),
      })
    })
  }
}

/// One attribute of the current start tag, stored internally; [`AttributeRef`] is the borrowed view handed to callers.
#[derive(Clone, Debug)]
struct Attribute {
  name: QName,
  /// The normalized value is a byte range into the parser's `attribute_text` buffer, so every value shares one
  /// allocation that the parser reuses across elements rather than each owning a `String`.
  value: Range<usize>,
  declares_namespace: bool,
  /// Where the name begins in the document, so a consumer reports a fault at the attribute rather than at its element.
  at: Location,
  /// Where the value begins, past the quotation mark, so a fault inside the value is reported where it is.
  value_at: Location,
}

/// An element whose start tag has been read but whose end tag has not: the state the parser keeps while it is open.
#[derive(Debug)]
struct OpenElement {
  /// The element's expanded name, reported when it closes.
  name: QName,
  /// The element's name as written, a range into `self.names`, compared against the end tag's raw name.
  lexical: Range<usize>,
  /// The namespace-scope position to roll back to when the element closes, dropping the bindings it declared.
  namespace_mark: usize,
  /// The `xml:space` in effect within the element.
  xml_space: XmlSpace,
  /// The `xml:lang` in effect within the element, if any.
  xml_lang: Option<NameId>,
  /// The base URI in effect within this element: the enclosing base, resolved with an `xml:base` attribute if the tag
  /// carried one (XML Base).
  base: Option<UriReference>,
  /// The entity depth at which the start tag was read. An end tag must be read at the same depth, so an element cannot
  /// start in one entity and end in another (XML 1.0 §4.3.2: logical and physical structures must nest properly).
  entity_depth: usize,
}

/// A markup token scanned but not yet interpreted, because pending text had to be emitted
/// first. Holding it lets a text run and the markup that ends it be reported in that order.
#[derive(Debug)]
struct Held {
  kind: TokenKind,
  text: String,
  at: Location,
}

/// Where in the document the parser is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
  /// Before the root element.
  Prolog,
  /// Inside the root element.
  Content,
  /// After the root element.
  Epilog,
}

/// An XML 1.0 parser that holds no I/O.
///
/// One parser drives one document: feed its bytes, drive [`advance`](Self::advance) to [`Progress::Eof`], then drop it.
/// It is not reset or reused for a second document.
///
/// # Examples
///
/// ```
/// use xenolith::io::{TokenKind, Parser, Progress};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<greeting xml:lang='en'>Hello</greeting>", true)?;
///
/// let mut kinds = Vec::new();
/// loop {
///   match parser.advance()? {
///     Progress::Token(kind) => kinds.push(kind),
///     Progress::Eof => break,
///     // `Progress` grows as later phases land, so a catch-all arm is required.
///     other => panic!("unexpected {other:?}: the whole document was fed at once"),
///   }
/// }
/// assert_eq!(kinds, [TokenKind::StartElement, TokenKind::Text, TokenKind::EndElement]);
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// Values are read through accessors while the event is current:
///
/// ```
/// use xenolith::io::{TokenKind, Parser, Progress};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<t:table xmlns:t='urn:t' rows='2'/>", true)?;
///
/// assert_eq!(parser.advance()?, Progress::Token(TokenKind::StartElement));
/// assert_eq!(parser.local_name(), "table");
/// assert_eq!(parser.prefix(), Some("t"));
/// assert_eq!(parser.namespace_uri(), Some("urn:t"));
/// assert_eq!(parser.attribute_value(None, "rows"), Some("2"));
///
/// // An empty element reports its end as a separate event.
/// assert_eq!(parser.advance()?, Progress::Token(TokenKind::EndElement));
/// assert_eq!(parser.advance()?, Progress::Eof);
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Debug)]
pub struct Parser {
  stack: EntityStack,
  pool: NamePool,
  config: ParserConfig,
  space_name: NameId,
  lang_name: NameId,
  /// The interned local name `base`, for spotting `xml:base` attributes.
  base_name: NameId,
  /// The interned local name `id`, for spotting `xml:id` attributes.
  id_name: NameId,
  scope: NamespaceScope,
  open: Vec<OpenElement>,
  phase: Phase,
  seen_doctype: bool,
  /// The root element name the `DOCTYPE` declared, interned; a validator matches the document
  /// root against it.
  doctype_name: Option<NameId>,
  /// The public and system identifiers of the `DOCTYPE`'s external subset, kept for the
  /// accessors after `dtd_external_id` has been consumed to fetch it.
  doctype_public_id: Option<String>,
  doctype_system_id: Option<String>,
  /// The document type definition, once a `DOCTYPE` has declared one.
  dtd: Option<Dtd>,
  /// True if the document declared an external DTD subset that was not read (no resolver), so
  /// an entity we cannot find might still be declared out there.
  external_subset_unread: bool,
  /// The DTD text gathered so far and the parse over it, which the `dtd` module owns.
  dtd_assembly: DtdAssembly,
  /// The external subset's identifier, until it has been fetched.
  dtd_external_id: Option<(Option<String>, String)>,
  /// True while the DTD is being parsed: `advance` drives it before the `Doctype` event.
  dtd_active: bool,
  /// Character data accumulated but not yet emitted, so a run split by a character reference
  /// or an entity boundary still surfaces as one text event where it can.
  pending_text: String,
  pending_text_at: Location,
  /// A markup token scanned while `pending_text` still had to be flushed first.
  held: Option<Held>,
  /// Entity references being expanded into an attribute value, to detect a repeated recursion and its depth.
  expanding: Vec<NameId>,
  /// An external entity the parser has stopped to have resolved, if any.
  pending_entity: Option<EntityRequest>,
  /// True while a just-begun external entity may still open with a text declaration that has to be stepped over before
  /// its content is read. Set by [`Parser::begin_entity`], cleared once the declaration is stripped or ruled out. At
  /// most one entity is in this state at a time, since an entity's declaration is handled before any reference within
  /// it opens another.
  ///
  entity_text_decl_pending: bool,
  /// The end of an empty element, owed to the caller on the next call.
  end_pending: bool,
  /// The kind of the current event, or `None` before the first one and after the last.
  kind: Option<TokenKind>,
  /// Scratch holding the token being interpreted; reused between tokens.
  token: String,
  /// Where the current token begins, so the event's location and an error's location both point at its start rather
  /// than at wherever reading has since reached.
  token_at: Location,
  /// Where the current processing instruction's data begins, past the target and the whitespace that separates it, so
  /// a handler can map a position inside foreign-language data back to the document.
  pi_data_at: Location,
  /// Lexical names of the open elements, so an end tag can be compared with its start tag.
  names: String,
  text: String,
  name: QName,
  attributes: Vec<Attribute>,
  attribute_text: String,
  version: String,
  /// The XML declaration gives the encoding. The stream layer sniffed the encoding from the bytes and is already
  /// decoding with it, so this copy is read only to report the declaration, never to pick a codec here.
  ///
  declared_encoding: Option<String>,
  standalone: Option<bool>,
  xml_space: XmlSpace,
  xml_lang: Option<NameId>,
  /// The base URI in effect for the current event (XML Base).
  base: Option<UriReference>,
}

impl Default for Parser {
  fn default() -> Self {
    Self::new()
  }
}

impl Parser {
  /// Creates a parser that sniffs the document's encoding from its bytes, with the default [`ParserConfig`] and no
  /// system identifier.
  ///
  /// Use [`with_document`](Self::with_document) to set the system identifier or pin the encoding, and
  /// [`set_config`](Self::set_config) to change the settings.
  #[must_use]
  pub fn new() -> Self {
    Self::with_document(Entity::document(CharStream::new()))
  }

  /// Creates a parser over a prepared document entity, with the default [`ParserConfig`].
  ///
  /// Use this when you know the system identifier or encoding in advance. The document stream's system identifier
  /// becomes the base URI and the origin of error locations, and an encoding set with [`CharStream::with_encoding`]
  /// pins decoding instead of sniffing it.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::io::{CharStream, Entity, Parser};
  ///
  /// let document = Entity::document(CharStream::with_encoding("UTF-8")?.with_system_id("file:///doc.xml"));
  /// let mut parser = Parser::with_document(document);
  /// parser.feed(b"<a/>", true)?;
  /// parser.advance()?;
  /// assert_eq!(parser.location().system_id.as_deref(), Some("file:///doc.xml"));
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  #[must_use]
  pub fn with_document(document: Entity) -> Self {
    let config = ParserConfig::default();
    let mut pool = NamePool::new();
    let space_name = pool.intern("space");
    let lang_name = pool.intern("lang");
    let base_name = pool.intern("base");
    let id_name = pool.intern("id");
    Self {
      stack: EntityStack::new(document, config.limits.entities),
      pool,
      config,
      space_name,
      lang_name,
      base_name,
      id_name,
      scope: NamespaceScope::new(),
      open: Vec::new(),
      phase: Phase::Prolog,
      seen_doctype: false,
      doctype_name: None,
      doctype_public_id: None,
      doctype_system_id: None,
      dtd: None,
      external_subset_unread: false,
      dtd_assembly: DtdAssembly::new(),
      dtd_external_id: None,
      dtd_active: false,
      pending_text: String::new(),
      pending_text_at: Location::unknown(),
      held: None,
      expanding: Vec::new(),
      pending_entity: None,
      entity_text_decl_pending: false,
      end_pending: false,
      kind: None,
      token: String::new(),
      token_at: Location::unknown(),
      pi_data_at: Location::unknown(),
      names: String::new(),
      text: String::new(),
      name: QName::new(None, None, NameId::EMPTY),
      attributes: Vec::new(),
      attribute_text: String::new(),
      version: String::new(),
      declared_encoding: None,
      standalone: None,
      xml_space: XmlSpace::Default,
      xml_lang: None,
      base: None,
    }
  }

  /// Replaces the parser's settings: the extensions it applies, its limits, and the length of text fragments. Set them
  /// before parsing begins.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::io::{Extensions, Parser, ParserConfig};
  ///
  /// let mut config = ParserConfig::default();
  /// config.extensions = Extensions::none();
  /// let mut parser = Parser::new();
  /// parser.set_config(config);
  /// ```
  pub fn set_config(&mut self, config: ParserConfig) {
    self.stack.set_limits(config.limits.entities);
    self.config = config;
  }

  /// The configuration in effect; change it with [`set_config`](Self::set_config).
  #[must_use]
  pub const fn config(&self) -> &ParserConfig {
    &self.config
  }

  /// Fixes the encoding of the document, skipping detection.
  ///
  /// Use this when the encoding is dictated from outside the document, such as a transport header, instead of the
  /// byte-order mark and declaration the document would otherwise be sniffed for. The caller must call it before the
  /// first [`feed`](Self::feed); [`CharStream::use_encoding`] covers the detail, the byte-order mark caveat included.
  ///
  /// # Errors
  ///
  /// See [`CharStream::use_encoding`]: an unknown or unavailable encoding, or a call made after
  /// bytes have already been fed.
  pub fn set_encoding(&mut self, encoding: &str) -> Result<()> {
    self.stack.current_mut().stream_mut().use_encoding(encoding)
  }

  /// Supplies bytes of the document, or of whatever entity is innermost.
  ///
  /// The parser appends the bytes to the entity now being read: the document until an entity is streamed, then that
  /// entity's own bytes between [`begin_entity`](Self::begin_entity) and its exhaustion. The caller sets `last` on the
  /// final chunk of that entity; feeding again after `last` is a usage error.
  ///
  /// # Errors
  ///
  /// See [`EntityStack::feed`].
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    self.stack.feed(bytes, last)
  }

  /// Clears the per-event output so no accessor reports a value an earlier event left behind; each event's handler
  /// then sets what it reports. Document-level state (the XML declaration and `DOCTYPE` metadata) is not per-event
  /// and stays in place.
  fn reset_event_fields(&mut self) {
    self.kind = None;
    self.name = QName::new(None, None, NameId::EMPTY);
    self.attributes.clear();
    self.attribute_text.clear();
    self.text.clear();
  }

  /// Advances parsing by one step and reports what it achieved.
  ///
  /// Each call returns a [`Progress`] that says what to do before calling `advance` again; the loop ends at
  /// [`Eof`](Progress::Eof).
  ///
  /// - [`Token`](Progress::Token) carries the [`TokenKind`]. Read the token's data through
  ///   [`token_ref`](Self::token_ref), whose [`TokenRef`] variant matches that kind (for example,
  ///   [`TokenRef::StartElement`] carries a start element's name and attributes); its values belong to this token only.
  /// - [`NeedMoreInput`](Progress::NeedMoreInput): supply more bytes with [`feed`](Self::feed), setting its `last`
  ///   flag on the final chunk, then call again.
  /// - [`NeedEntity`](Progress::NeedEntity): an external entity must be resolved; that variant documents the full
  ///   read-fetch-answer protocol and how to choose among [`begin_entity`](Self::begin_entity),
  ///   [`provide_entity`](Self::provide_entity), and [`decline_entity`](Self::decline_entity), then call again.
  /// - [`Eof`](Progress::Eof): the document is complete; stop.
  ///
  /// Every call first clears the previous event's fields (name, attributes, text), so an accessor never reports a
  /// value left by an earlier event; the accessors that do not belong to the current event read empty. The
  /// document-level accessors (the XML declaration and `DOCTYPE` metadata) are not per-event and stay readable
  /// throughout.
  ///
  /// A [`StreamSource`](crate::io::StreamSource) runs this loop and resolves entities through a
  /// [`UriResolver`](crate::io::resolve::UriResolver), so most callers never call `advance` directly.
  ///
  /// # Errors
  ///
  /// Returns [`Error::WellFormedness`] or [`Error::Namespace`] for a document that breaks the rules, and passes on
  /// decoding and limit errors.
  pub fn advance(&mut self) -> Result<Progress> {
    if self.dtd_active {
      return self.drive_dtd();
    }
    // Each event reports only through its own accessors, so clear the last event's before producing this one; a value
    // left over must not be read as this event's. This runs after the DTD-driving return above, whose `Doctype` text
    // was set on an earlier call and has to survive.
    self.reset_event_fields();
    if self.end_pending {
      self.end_pending = false;
      let open = self.open.pop().expect("an empty element was left open");
      self.close(open);
      self.kind = Some(TokenKind::EndElement);
      return Ok(Progress::Token(TokenKind::EndElement));
    }
    loop {
      // A markup token held back while the text before it was flushed.
      if let Some(held) = self.held.take() {
        self.token_at = held.at;
        if let Some(kind) = self.interpret(held.kind, &held.text)? {
          self.kind = Some(kind);
          return Ok(Progress::Token(kind));
        }
        if let Some(progress) = self.outstanding() {
          return progress;
        }
        continue;
      }

      // A freshly-begun external entity may open with a text declaration, which is stepped over
      // before its content is read. It can straddle feeds, so this may request more input.
      if self.entity_text_decl_pending {
        let last = !self.stack.current().stream().can_be_fed();
        let rem = self.stack.current().stream().remainder();
        match decl::text_declaration_span(rem, last).map_err(|e| e.at(self.stack.location()))? {
          TextDecl::NeedMore => return Ok(Progress::NeedMoreInput),
          TextDecl::None => self.entity_text_decl_pending = false,
          TextDecl::Present(len) => {
            self.entity_text_decl_pending = false;
            self.stack.current_mut().stream_mut().advance(len);
          }
        }
        continue;
      }

      let scanned = {
        let stream = self.stack.current().stream();
        let rest = stream.remainder();
        let no_more_input = !stream.can_be_fed();
        if rest.is_empty() {
          if !no_more_input {
            return Ok(Progress::NeedMoreInput);
          }
          None
        } else {
          match scan(rest, no_more_input, &self.config.limits.tokens, self.config.text_fragment_len)
            .map_err(|e| e.at(self.stack.location()))?
          {
            Scan::Found(kind, len) => Some((Some(kind), len)),
            // A reference is not a token the parser reports: it is expanded into the text, or opens the entity it
            // names. It goes on with no kind.
            Scan::Reference(len) => Some((None, len)),
            Scan::Pending => return Ok(Progress::NeedMoreInput),
          }
        }
      };

      let Some((kind, len)) = scanned else {
        // The innermost entity is exhausted. Resume the one that referenced it, or, at the
        // document entity, flush any last text and end.
        if self.stack.depth() > 1 {
          self.stack.pop();
          continue;
        }
        if let Some(kind) = self.flush_text()? {
          return Ok(Progress::Token(kind));
        }
        return self.finish();
      };

      let at = self.stack.location();
      // Copy the token out of the stream into `token`, then take it, so the handlers can borrow `self` while reading
      // it; the text and reference arms move the buffer back afterwards to reuse its capacity.
      self.token.clear();
      self.token.push_str(&self.stack.current().stream().remainder()[..len]);
      self.stack.current_mut().stream_mut().advance(len);
      let text = std::mem::take(&mut self.token);

      let event = match kind {
        Some(TokenKind::Text) => {
          let outcome = self.accumulate_text(&text, &at);
          self.token = text;
          outcome?;
          self.flush_if_full()?
        }
        None => {
          let outcome = self.reference(&text, &at);
          self.token = text;
          outcome?;
          self.flush_if_full()?
        }
        Some(kind) => {
          // Markup ends a text run: flush the text first and hold the markup for next time.
          if self.pending_text.is_empty() {
            self.token_at = at;
            let outcome = self.interpret(kind, &text);
            self.token = text;
            outcome?
          } else {
            self.held = Some(Held { kind, text, at });
            self.flush_text()?
          }
        }
      };

      if let Some(kind) = event {
        self.kind = Some(kind);
        return Ok(Progress::Token(kind));
      }
      if let Some(progress) = self.outstanding() {
        return progress;
      }
    }
  }

  /// What is left to do once a token has been interpreted and yielded no event of its own: a DTD waiting to be parsed,
  /// or an entity waiting to be fetched.
  ///
  /// Both ways into `interpret` have to call this. A token that arrives with text before it is held back while that
  /// text is flushed and interpreted on the next turn of the loop, and when that path did not call it, a `<!DOCTYPE>`
  /// written after so much as a newline had its DTD left unparsed — so the next token was scanned first and the
  /// `Doctype` event came out *after* the root element's start tag. Everything downstream believed it: a validator
  /// built when the DOCTYPE arrived never saw the root element open, and unbalanced its stack on the way out.
  fn outstanding(&mut self) -> Option<Result<Progress>> {
    if self.dtd_active {
      return Some(self.drive_dtd());
    }
    if self.pending_entity.is_some() {
      return Some(Ok(Progress::NeedEntity));
    }
    None
  }

  /// Checks that the document is allowed to end here.
  fn finish(&mut self) -> Result<Progress> {
    if let Some(open) = self.open.last() {
      let message = format!("element <{}> is not closed", &self.names[open.lexical.clone()]);
      return Err(self.error(Error::well_formedness, message));
    }
    if self.phase == Phase::Prolog {
      return Err(self.error(Error::well_formedness, "the document has no root element"));
    }
    self.kind = None;
    Ok(Progress::Eof)
  }

  /// Interprets one scanned token, reporting the event it yields, if any.
  ///
  /// `kind` is the kind the scanner settled on, and it selects the handler. Only markup reaches here: a start or end
  /// tag, a comment, a CDATA section, a processing instruction, or a `DOCTYPE`. `advance` accumulates text and expands
  /// references itself, so they never arrive here.
  ///
  /// `text` is that token's source as scanned, delimiters and all (`<a x="1">`, `<!-- c -->`, `<?t d?>`,
  /// `<![CDATA[...]]>`, `<!DOCTYPE ...>`); each handler strips its own. It is borrowed, so interpreting allocates
  /// nothing for it.
  ///
  /// A returned `Some(kind)` is an event to report. `None` means the token was interpreted but yields no event here: a
  /// `<!DOCTYPE>` sets DTD parsing in motion and reports `Doctype` only once that finishes.
  fn interpret(&mut self, kind: TokenKind, text: &str) -> Result<Option<TokenKind>> {
    if !matches!(kind, TokenKind::StartElement | TokenKind::EndElement) {
      // A start or end tag sets its own context in its handler; every other token (comment, CDATA, PI, DOCTYPE)
      // inherits the enclosing element's xml:space, xml:lang, and base URI so the accessors report them for this event.
      self.xml_space = self.open.last().map_or(XmlSpace::Default, |e| e.xml_space);
      self.xml_lang = self.open.last().and_then(|e| e.xml_lang);
      {
        self.base = if self.config.extensions.xml_base {
          self.open.last().map_or_else(|| self.stack.base_uri().cloned(), |e| e.base.clone())
        } else {
          None
        };
      }
    }
    match kind {
      TokenKind::ProcessingInstruction => self.processing_instruction(text),
      TokenKind::Comment => self.comment(text).map(Some),
      TokenKind::Doctype => self.doctype(text),
      TokenKind::StartElement => self.start_tag(text).map(Some),
      TokenKind::EndElement => self.end_tag(text).map(Some),
      TokenKind::CData => self.cdata(text).map(Some),
      // Text never reaches here, since `advance` accumulates it itself, and the scanner never reports an XML
      // declaration: that is a processing instruction until `processing_instruction` says otherwise.
      TokenKind::Text | TokenKind::XmlDeclaration => Ok(None),
    }
  }

  /// Splits `<?...?>` into the XML declaration and ordinary processing instructions.
  fn processing_instruction(&mut self, text: &str) -> Result<Option<TokenKind>> {
    debug_assert!(text.starts_with("<?") && text.ends_with("?>"));
    let body = &text[2..text.len() - 2];
    let target_len = body.find(chars::is_whitespace).unwrap_or(body.len());
    let (target, data) = body.split_at(target_len);

    if target.eq_ignore_ascii_case(XML_PREFIX) {
      // The stream leaves the XML declaration in the character input (it read it only to sniff the encoding), so it
      // arrives here as a `<?xml...?>` token. Only a genuine declaration, at the very start of the document entity, is
      // allowed.
      if target != XML_PREFIX || self.token_at.offset != 0 || self.stack.depth() > 1 {
        let message = format!("\"{target}\" is a reserved target");
        return Err(self.error(Error::well_formedness, message));
      }
      self.xml_declaration(data)?;
      return Ok(Some(TokenKind::XmlDeclaration));
    }
    // Whether `target` is a `Name` is a lexical constraint left to `StrictXmlValidator`.
    self.name = QName::new(None, None, self.pool.intern(target));
    let trimmed = data.trim_start_matches(chars::is_whitespace);
    // The source position of the data, so a handler can map a position inside foreign-language data (a `<?php ... ?>`,
    // say) back to the document. The data begins at `text.len() - "?>".len() - trimmed.len()`; walk the token start
    // over everything before it (`<?`, the target, and the dropped separating whitespace) to find where it is.
    let mut at = self.token_at.clone();
    for c in text[..text.len() - 2 - trimmed.len()].chars() {
      at.advance(c);
    }
    self.pi_data_at = at;
    self.text.clear();
    self.text.push_str(trimmed);
    Ok(Some(TokenKind::ProcessingInstruction))
  }

  /// Reads the pseudo-attributes of the XML declaration.
  fn xml_declaration(&mut self, data: &str) -> Result<()> {
    debug_assert!(data.is_empty() || data.starts_with(chars::is_whitespace));
    let mut rest = data;
    let mut seen: Vec<&str> = Vec::new();
    while !rest.trim_start_matches(chars::is_whitespace).is_empty() {
      // `<?xml version="1.0"encoding="UTF-8"?>` is not a declaration: the production puts an `S` between the
      // pseudo-attributes, not an optional one.
      if whitespace_len(rest) == 0 {
        let message = "the XML declaration needs whitespace between its parts";
        return Err(self.error(Error::well_formedness, message));
      }
      let (name, value, tail) =
        decl::pseudo_attribute(rest, "XML declaration").map_err(|e| e.at(self.stack.location()))?;
      // A repeat of any pseudo-attribute is rejected up front. `seen` only ever holds the three known names, so an
      // unknown one never matches here and falls to the `other` arm below to be reported as unknown.
      if seen.contains(&name) {
        let message = format!("the XML declaration has more than one {name}");
        return Err(self.error(Error::well_formedness, message));
      }
      // Dispatch on the name first, then check its position, so a misplaced `version`/`encoding`/`standalone` is told
      // apart from a name that is not a pseudo-attribute at all.
      match name {
        "version" => {
          // No position guard is needed: version heads the declaration, and a later one is caught as a repeat above.
          // `VersionNum ::= '1.' [0-9]+`, so a stray space or character is not merely an unsupported version but a
          // malformed declaration.
          let digits = value.strip_prefix("1.").filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
          if digits.is_none() {
            let message = format!("{value:?} is not an XML version; it must be \"1.\" followed by digits");
            return Err(self.error(Error::well_formedness, message));
          }
          self.version = value.to_owned();
        }
        "encoding" => {
          // `XMLDecl ::= '<?xml' VersionInfo EncodingDecl? SDDecl? ...`, so encoding sits right after version and
          // ahead of any standalone; `seen == ["version"]` is the only spot the production allows.
          if seen != ["version"] {
            let message = "encoding must come after version and before standalone in the XML declaration";
            return Err(self.error(Error::well_formedness, message));
          }
          if !chars::is_enc_name(value) {
            let message = format!(
              "{value:?} is not an encoding name; it must start with a letter and hold only letters, digits, \".\", \"_\" and \"-\""
            );
            return Err(self.error(Error::well_formedness, message));
          }
          self.declared_encoding = Some(value.to_owned());
        }
        "standalone" => {
          // `SDDecl` trails `VersionInfo` (and an optional `EncodingDecl`), so version must already be in place.
          if !seen.contains(&"version") {
            let message = "standalone must come after version in the XML declaration";
            return Err(self.error(Error::well_formedness, message));
          }
          self.standalone = match value {
            "yes" => Some(true),
            "no" => Some(false),
            other => {
              let message = format!("standalone must be \"yes\" or \"no\", not {other:?}");
              return Err(self.error(Error::well_formedness, message));
            }
          };
        }
        other => {
          let message = format!(
            "{other:?} is not a pseudo-attribute of the XML declaration; only version, encoding and standalone are allowed"
          );
          return Err(self.error(Error::well_formedness, message));
        }
      }
      seen.push(name);
      rest = tail;
    }
    if self.version.is_empty() {
      return Err(self.error(Error::well_formedness, "the XML declaration has no version"));
    }
    Ok(())
  }

  fn comment(&mut self, text: &str) -> Result<TokenKind> {
    debug_assert!(text.starts_with("<!--") && text.ends_with("-->"));
    // Whether the body holds `--` or ends in `-` is a lexical constraint left to `StrictXmlValidator`; the body is
    // delivered as scanned.
    let body = &text[4..text.len() - 3];
    self.text.clear();
    self.text.push_str(body);
    Ok(TokenKind::Comment)
  }

  fn doctype(&mut self, text: &str) -> Result<Option<TokenKind>> {
    debug_assert!(text.starts_with("<!DOCTYPE") && text.ends_with('>'));
    if self.phase != Phase::Prolog {
      let message = "the document type declaration must come before the root element";
      return Err(self.error(Error::well_formedness, message));
    }
    if self.seen_doctype {
      return Err(self.error(Error::well_formedness, "there may be only one document type declaration"));
    }
    self.seen_doctype = true;

    // `<!DOCTYPE` S Name (S ExternalID)? S? ('[' intSubset ']' S?)? `>`
    let body = text[9..text.len() - 1].trim_start_matches(chars::is_whitespace);
    let name_len = body.find(|c: char| chars::is_whitespace(c) || c == '[').unwrap_or(body.len());
    if !chars::is_name(&body[..name_len]) {
      let message = format!("{:?} is not a valid document type name", &body[..name_len.min(body.len())]);
      return Err(self.error(Error::well_formedness, message));
    }
    self.doctype_name = Some(self.pool.intern(&body[..name_len]));
    let after_name = &body[name_len..];

    let (before_bracket, internal_subset) = match after_name.split_once('[') {
      Some((before, subset)) => (before, subset.trim_end().trim_end_matches(']')),
      None => (after_name, ""),
    };
    // Between the name and the internal subset only an external identifier may appear.
    let external = before_bracket.trim();
    self.dtd_external_id = if external.is_empty() {
      None
    } else if external.starts_with("SYSTEM") || external.starts_with("PUBLIC") {
      let (public_id, system_id, tail) = parse_external_id(external).ok_or_else(|| {
        self.error(Error::well_formedness, format!("{external:?} is not a valid external identifier"))
      })?;
      let extra = tail.trim_start_matches(chars::is_whitespace);
      if !extra.is_empty() {
        // `extra` is a subslice of `text`, so their pointer difference is its byte offset within the token, which
        // points the error at the first stray character rather than at the whole declaration.
        let index = extra.as_ptr() as usize - text.as_ptr() as usize;
        let message = "the document type declaration has content after the external identifier";
        return Err(self.error_at(Error::well_formedness, message, text, index));
      }
      Some((public_id, system_id))
    } else {
      let message = format!("{external:?} is not a valid external identifier in the document type declaration");
      return Err(self.error(Error::well_formedness, message));
    };
    // Keep the identifiers for the accessors; `dtd_external_id` itself is consumed on fetch.
    if let Some((public_id, system_id)) = &self.dtd_external_id {
      self.doctype_public_id = public_id.clone();
      self.doctype_system_id = Some(system_id.clone());
    }

    // Keep the DOCTYPE text for the event, and set up the DTD to be parsed by `drive_dtd`, which runs across the
    // entity fetches an external subset or parameter entity may need.
    self.text.clear();
    self.text.push_str(text);
    // `internal_subset` is a subslice of `text` when there is one, so their pointer difference is where it begins.
    let subset_at = if internal_subset.is_empty() {
      self.token_at.clone()
    } else {
      self.location_in(text, internal_subset.as_ptr() as usize - text.as_ptr() as usize)
    };
    self.dtd_assembly = DtdAssembly::with_internal_subset(internal_subset, subset_at);
    self.dtd_active = true;
    Ok(None)
  }

  /// Drives DTD parsing across the entity fetches it may need, and emits the `Doctype` event
  /// when the DTD is complete.
  fn drive_dtd(&mut self) -> Result<Progress> {
    // Fetch the external subset first, if one was declared and is not yet in the buffer.
    if let Some((public_id, system_id)) = self.dtd_external_id.take() {
      let base = self.stack.document().base_uri().map(ToString::to_string);
      self.pending_entity = Some(EntityRequest::new(None, public_id, system_id, base, RequestKind::ExternalSubset));
      return Ok(Progress::NeedEntity);
    }
    // One pass over the buffer. It either finishes the DTD or stops for an external parameter
    // entity; in the latter case the driver fetches it and calls back here through `advance`.
    match self.dtd_assembly.advance(&mut self.pool)? {
      dtd::DtdOutcome::Complete(dtd) => {
        self.dtd = Some(*dtd);
        self.dtd_active = false;
        self.dtd_assembly = DtdAssembly::new();
        self.kind = Some(TokenKind::Doctype);
        Ok(Progress::Token(TokenKind::Doctype))
      }
      dtd::DtdOutcome::NeedExternalPe(pe) => {
        // A relative system identifier is resolved against the resource the entity was declared in, which is the
        // document itself where that resource has no identifier of its own.
        let base = pe.base.clone().or_else(|| self.stack.document().base_uri().map(ToString::to_string));
        self.pending_entity = Some(EntityRequest::new(
          Some(pe.name.clone()),
          pe.public_id.clone(),
          pe.system_id.clone(),
          base,
          RequestKind::ParameterEntity,
        ));
        Ok(Progress::NeedEntity)
      }
    }
  }

  fn cdata(&mut self, text: &str) -> Result<TokenKind> {
    debug_assert!(text.starts_with("<![CDATA[") && text.ends_with("]]>"));
    if self.phase != Phase::Content {
      return Err(self.error(Error::well_formedness, "a CDATA section may only appear inside the root element"));
    }
    self.text.clear();
    self.text.push_str(&text[9..text.len() - 3]);
    Ok(TokenKind::CData)
  }

  /// Appends one fragment of character data to the current text run in `pending_text`, recording where the run begins.
  ///
  /// `text` is plain character data: the scanner ends a `Text` token before every `&` and `<`, so no reference or
  /// markup is inside it.
  ///
  /// Before appending, this rejects the fragment if it contains `]]>`, the one sequence character data may not hold
  /// (XML 1.0 §2.4). Checking each fragment on its own is enough because the scanner never lets a `]]>` straddle two
  /// of them: the scanner holds back a run ending in a trailing `]` or `]]` while more input may follow, rather than
  /// emit a token that would split the `]]>`.
  fn accumulate_text(&mut self, text: &str, at: &Location) -> Result<()> {
    if let Some(i) = text.find("]]>") {
      self.token_at = at.clone();
      let message = "\"]]>\" may not appear in text; write \"]]&gt;\"";
      return Err(self.error_at(Error::well_formedness, message, text, i));
    }
    if self.pending_text.is_empty() {
      self.pending_text_at = at.clone();
    }
    self.pending_text.push_str(text);
    Ok(())
  }

  /// Flushes the pending text as a `Text` event once it reaches [`ParserConfig::text_fragment_len`], returning `None`
  /// while it is still shorter.
  ///
  /// The parser calls this after each text or reference fragment, so it emits a long run in pieces rather than buffer
  /// it without bound. This is why one run can span several `Text` events; [`TokenKind::Text`] covers coalescing them.
  fn flush_if_full(&mut self) -> Result<Option<TokenKind>> {
    if self.pending_text.len() >= self.config.text_fragment_len { self.flush_text() } else { Ok(None) }
  }

  /// Emits the pending text run as a `Text` event, or returns `None` when there is nothing to report.
  ///
  /// Inside the root element, this moves the run into the `text` field, from where [`TokenRef::Text`] borrows it, and
  /// reports a `Text` event. The prolog and epilog allow only whitespace, so there this discards a whitespace-only run
  /// and returns `None` but rejects any other text, pointing the error at its first non-whitespace character. An empty
  /// run also returns `None`.
  fn flush_text(&mut self) -> Result<Option<TokenKind>> {
    if self.pending_text.is_empty() {
      return Ok(None);
    }
    if self.phase != Phase::Content {
      if self.pending_text.chars().all(chars::is_whitespace) {
        self.pending_text.clear();
        return Ok(None);
      }
      // Point at the first non-whitespace character, the one actually out of place, not the leading whitespace the run
      // may open with.
      self.token_at = self.pending_text_at.clone();
      let offending = self.pending_text.find(|c: char| !chars::is_whitespace(c)).unwrap_or(0);
      let place = if self.phase == Phase::Prolog { "before" } else { "after" };
      let message = format!("text may not appear {place} the root element");
      return Err(self.error_at(Error::well_formedness, message, &self.pending_text, offending));
    }
    std::mem::swap(&mut self.text, &mut self.pending_text);
    self.pending_text.clear();
    self.token_at = self.pending_text_at.clone();
    Ok(Some(TokenKind::Text))
  }

  /// Handles a reference token (`&...;`) in content. A reference yields no event of its own, so this returns `None`.
  ///
  /// A character reference (`&#..;`) or a predefined entity (`&lt;`, `&gt;`, `&amp;`, `&apos;`, `&quot;`) resolves to a
  /// character that this appends to the current text run. For a general-entity reference, the parser begins reading the
  /// entity's replacement in place (see [`push_general_entity`](Self::push_general_entity)). The parser rejects a
  /// reference outside the root element.
  fn reference(&mut self, text: &str, at: &Location) -> Result<Option<TokenKind>> {
    debug_assert!(text.starts_with('&') && text.ends_with(';'));
    if self.phase != Phase::Content {
      self.token_at = at.clone();
      let message = "a reference may not appear outside the root element";
      return Err(self.error(Error::well_formedness, message));
    }
    let body = &text[1..text.len() - 1];
    if let Some(c) = self.character_or_predefined(body, text, at)? {
      if self.pending_text.is_empty() {
        self.pending_text_at = at.clone();
      }
      self.pending_text.push(c);
      return Ok(None);
    }
    // A general entity: begin reading its replacement where the reference stood. The pending text is deliberately not
    // flushed, so character data on either side of an entity whose replacement is itself text coalesces into one node,
    // as the data model wants. Markup in the replacement flushes it normally.
    let name = self.pool.intern(body);
    self.push_general_entity(name, at)?;
    Ok(None)
  }

  /// Resolves `body` (the text between `&` and `;`) to the character a reference denotes.
  ///
  /// A character reference (`#..`) or one of the five predefined entities (`lt`, `gt`, `amp`, `apos`, `quot`) gives
  /// `Some(char)`. A general-entity name gives `None`, which the caller resolves against the DTD. Anything else is an
  /// error. `token` is the whole `&...;`, used only for the error message.
  fn character_or_predefined(&self, body: &str, token: &str, at: &Location) -> Result<Option<char>> {
    debug_assert!(token.starts_with('&') && token.ends_with(';'));
    let c = match body {
      _ if body.starts_with('#') => return self.character_reference(body, token, at).map(Some),
      "lt" => '<',
      "gt" => '>',
      "amp" => '&',
      "apos" => '\'',
      "quot" => '"',
      name if chars::is_name(name) => return Ok(None),
      _ => {
        let message = format!("\"&{body};\" is not a reference; write \"&amp;\" for a literal ampersand");
        return Err(Error::well_formedness(message).at(at.clone()));
      }
    };
    Ok(Some(c))
  }

  /// Parses `body`, a `#dd` decimal or `#xhh` hexadecimal character reference, into the character it denotes.
  ///
  /// This rejects an empty or non-digit form, and a code point XML does not allow as a character (XML 1.0 §2.2).
  /// `token` is the whole `&...;`, used only for the error message.
  fn character_reference(&self, body: &str, token: &str, at: &Location) -> Result<char> {
    debug_assert!(token.starts_with('&') && token.ends_with(';'));
    debug_assert!(body.starts_with('#'));
    let error = |message: String| Error::well_formedness(message).at(at.clone());
    let digits = &body[1..];
    let (digits, radix) = match digits.strip_prefix('x') {
      Some(hex) => (hex, 16),
      None => (digits, 10),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
      let hint = if radix == 16 { "after \"&#x\" only 0-9, a-f and A-F may follow" } else { "write \"&#\" and digits" };
      return Err(error(format!("\"{token}\" is not a character reference; {hint}")));
    }
    let code = u32::from_str_radix(digits, radix).ok();
    code.and_then(char::from_u32).filter(|c| chars::is_char(*c)).ok_or_else(|| {
      error(format!(
        "\"{token}\" is not a character XML permits, and no escape can represent it \
         (XML 1.0 allows #x9, #xA, #xD, #x20-#xD7FF, #xE000-#xFFFD and #x10000-#x10FFFF)"
      ))
    })
  }

  /// Handles a general-entity reference in content, dispatching based on how the DTD declared the entity.
  ///
  /// For an internal entity, the parser pushes its replacement onto the entity stack and reads it in place, so it
  /// reports no event. For an external entity, the parser stops with [`Progress::NeedEntity`] for a driver to resolve.
  /// It rejects an unparsed entity, an undeclared one, and, in a standalone document, one that only the external subset
  /// or an external parameter entity declared (WFC: Entity Declared). This is the content path; an attribute value
  /// expands entities inline instead (`expand_at`).
  fn push_general_entity(&mut self, name: NameId, at: &Location) -> Result<()> {
    self.token_at = at.clone();
    let display = self.pool.resolve(name).to_owned();
    // WFC: Entity Declared, standalone form. A standalone document may not reference an entity that only the external
    // subset or an external parameter entity declares.
    if self.standalone == Some(true) && self.dtd.as_ref().is_some_and(|d| d.general_entity_is_external(name)) {
      let message = format!(
        "entity \"{display}\" is declared in the external subset, which a standalone document may not depend on"
      );
      return Err(self.error(Error::well_formedness, message));
    }
    match self.dtd.as_ref().and_then(|dtd| dtd.general_entity(name)).cloned() {
      Some(GeneralEntity::Internal { value }) => {
        let base = self.stack.base_uri().cloned();
        let stream = CharStream::from_text(&value).map_err(|e| e.at(at.clone()))?;
        let entity = Entity::new(Some(display.into()), EntityKind::InternalGeneral, stream, base);
        self.stack.push(entity)
      }
      Some(GeneralEntity::Unparsed { .. }) => {
        let message = format!("unparsed entity \"{display}\" may not be referenced in content");
        Err(self.error(Error::well_formedness, message))
      }
      Some(GeneralEntity::External { public_id, system_id, base }) => {
        // Stop and request it. A driver resolves it and calls `provide_entity`, or `decline_entity`; `advance` sees
        // the request and returns `Progress::NeedEntity`. A relative system identifier is resolved against the
        // resource the entity was declared in, which is the document itself where that resource has no identifier.
        let base = base.or_else(|| self.stack.document().base_uri().map(ToString::to_string));
        self.pending_entity =
          Some(EntityRequest::new(Some(display), public_id, system_id, base, RequestKind::GeneralEntity));
        Ok(())
      }
      None => Err(self.undeclared_entity(&display)),
    }
  }

  /// The error for a reference to an entity that has no declaration in reach.
  fn undeclared_entity(&self, name: &str) -> Error {
    let message = if self.external_subset_unread {
      format!(
        "entity \"{name}\" is not declared in the internal subset, and its external subset has not been read \
         (reading the external subset is not yet supported)"
      )
    } else {
      format!(
        "entity \"{name}\" is not declared; declare it in the document type declaration, \
         or write \"&amp;{name};\" if a literal ampersand was meant"
      )
    };
    self.error(Error::well_formedness, message)
  }

  fn start_tag(&mut self, text: &str) -> Result<TokenKind> {
    debug_assert!(text.starts_with('<') && !text.starts_with("</") && text.ends_with('>'));
    match self.phase {
      Phase::Prolog => self.phase = Phase::Content,
      Phase::Content => {}
      Phase::Epilog => {
        return Err(self.error(Error::well_formedness, "a document may have only one root element"));
      }
    }
    if let Some(limit) = self.config.limits.document.max_element_depth {
      if self.open.len() >= limit {
        let message = format!(
          "elements are nested more than {limit} deep; \
           raise ParserConfig.limits.document.max_element_depth if the document is trusted"
        );
        return Err(self.error(Error::limit, message));
      }
    }
    let empty = text.ends_with("/>");
    let body = &text[1..text.len() - if empty { 2 } else { 1 }];
    let name_len = body.find(chars::is_whitespace).unwrap_or(body.len());
    let lexical = &body[..name_len];

    self.parse_attributes(&body[name_len..], 1 + name_len, text)?;
    let element = self.pool.intern(lexical);
    self.apply_dtd_attributes(element)?;

    let namespace_mark = self.scope.mark();
    self.declare_namespaces()?;
    let name = self.resolve_element_name(lexical)?;
    self.resolve_attribute_names()?;

    let (xml_space, xml_lang) = self.space_and_lang()?;
    self.normalize_xml_id();
    let base = self.element_base()?;
    let lexical = self.remember_name(text, 1, name_len);
    self.name = name;
    self.xml_space = xml_space;
    self.xml_lang = xml_lang;
    {
      self.base = base.clone();
    }
    let entity_depth = self.stack.depth();
    self.open.push(OpenElement { name, lexical, namespace_mark, xml_space, xml_lang, base, entity_depth });
    self.end_pending = empty;
    Ok(TokenKind::StartElement)
  }

  fn end_tag(&mut self, text: &str) -> Result<TokenKind> {
    debug_assert!(text.starts_with("</") && text.ends_with('>'));
    let name = text[2..text.len() - 1].trim_end_matches(chars::is_whitespace);
    let Some(open) = self.open.last() else {
      let message = format!("</{name}> closes an element that was never opened");
      return Err(self.error(Error::well_formedness, message));
    };
    let expected = &self.names[open.lexical.clone()];
    if expected != name {
      let message = format!("</{name}> does not close <{expected}>");
      return Err(self.error(Error::well_formedness, message));
    }
    if open.entity_depth != self.stack.depth() {
      // The start tag and this end tag are in different entities.
      let message = format!("<{expected}> and its end tag are in different entities");
      return Err(self.error(Error::well_formedness, message));
    }
    let open = self.open.pop().expect("just inspected");
    self.close(open);
    Ok(TokenKind::EndElement)
  }

  /// Reports the state of the element being closed, then leaves its scope.
  fn close(&mut self, open: OpenElement) {
    self.name = open.name;
    self.xml_space = open.xml_space;
    self.xml_lang = open.xml_lang;
    // The end-tag event carries the closing element's own base, so a caller reading it here
    // sees the same base URI the start tag did; the parent's is restored on the next event.
    {
      self.base = open.base;
    }
    self.scope.revert(open.namespace_mark);
    self.names.truncate(open.lexical.start);
    self.attributes.clear();
    self.attribute_text.clear();
    if self.open.is_empty() {
      self.phase = Phase::Epilog;
    }
  }

  /// Parses the `name="value"` pairs of a start tag.
  ///
  /// `base` is where `rest` begins within `token`, so errors can point at the right column.
  fn parse_attributes(&mut self, rest: &str, base: usize, token: &str) -> Result<()> {
    self.attributes.clear();
    let mut values = std::mem::take(&mut self.attribute_text);
    values.clear();
    let outcome = self.parse_attributes_into(rest, base, token, &mut values);
    self.attribute_text = values;
    outcome
  }

  fn parse_attributes_into(&mut self, rest: &str, base: usize, token: &str, values: &mut String) -> Result<()> {
    let mut at = 0;
    loop {
      let spaces = whitespace_len(&rest[at..]);
      at += spaces;
      if at == rest.len() {
        return Ok(());
      }
      if spaces == 0 {
        let message = "attributes must be separated by whitespace";
        return Err(self.error_at(Error::well_formedness, message, token, base + at));
      }

      let name_at = self.location_in(token, base + at);
      let name_len = rest[at..].find(|c: char| c == '=' || chars::is_whitespace(c)).unwrap_or(rest.len() - at);
      let name = &rest[at..at + name_len];
      at += name_len;
      at += whitespace_len(&rest[at..]);

      if !rest[at..].starts_with('=') {
        // Most often a bare HTML-style attribute such as `checked` or `disabled`.
        let message = format!("attribute \"{name}\" has no value; every XML attribute needs one, as {name}=\"...\"");
        return Err(self.error_at(Error::well_formedness, message, token, base + at));
      }
      at += 1;
      at += whitespace_len(&rest[at..]);

      let Some(quote) = rest[at..].chars().next().filter(|c| *c == '"' || *c == '\'') else {
        let message = format!("the value of \"{name}\" is not quoted; enclose it in \" or '");
        return Err(self.error_at(Error::well_formedness, message, token, base + at));
      };
      at += quote.len_utf8();
      let Some(end) = rest[at..].find(quote) else {
        let message = format!("the value of \"{name}\" is not terminated");
        return Err(self.error_at(Error::well_formedness, message, token, base + at));
      };
      let raw = &rest[at..at + end];
      let raw_at = base + at;
      let value_at = self.location_in(token, raw_at);
      at += end + quote.len_utf8();

      let (prefix, local) = split_name(name);
      let declares_namespace = prefix == Some(XMLNS_PREFIX) || (prefix.is_none() && local == XMLNS_PREFIX);
      let start = values.len();
      self.expand_at(raw, values, true, token, raw_at)?;
      let name = QName::new(prefix.map(|p| self.pool.intern(p)), None, self.pool.intern(local));
      self.attributes.push(Attribute { name, value: start..values.len(), declares_namespace, at: name_at, value_at });
    }
  }

  /// Applies what the DTD says about this element's attributes: it collapses the whitespace
  /// of any specified attribute with a tokenized type, and supplies the declared defaults for
  /// attributes the start tag left out.
  fn apply_dtd_attributes(&mut self, element: NameId) -> Result<()> {
    let Some(defs) = self.dtd.as_ref().and_then(|dtd| dtd.attlist(element)) else {
      return Ok(());
    };
    let defs = defs.to_vec();
    // The lexical name of each specified attribute, interned to match the DTD's names.
    let mut present: Vec<NameId> = Vec::with_capacity(self.attributes.len());
    for i in 0..self.attributes.len() {
      let lexical = self.attributes[i].name.to_lexical(&self.pool);
      present.push(self.pool.intern(&lexical));
    }

    // Collapse the whitespace of specified values whose type is tokenized.
    let tokenized: Vec<NameId> = defs.iter().filter(|d| d.att_type.is_tokenized()).map(|d| d.name).collect();
    for (i, &lexical) in present.iter().enumerate() {
      if tokenized.contains(&lexical) {
        let value = &self.attribute_text[self.attributes[i].value.clone()];
        let normalized = dtd::normalize_tokenized(value, true);
        let start = self.attribute_text.len();
        self.attribute_text.push_str(&normalized);
        self.attributes[i].value = start..self.attribute_text.len();
      }
    }

    // Supply defaults for attributes the tag did not carry.
    let external_attlist = self.dtd.as_ref().is_some_and(|d| d.attlist_is_external(element));
    for def in &defs {
      let Some(value) = def.default.value() else { continue };
      if present.contains(&def.name) {
        continue;
      }
      // VC: Standalone Document Declaration, which the parser enforces as an error. A default from the external
      // subset or an external parameter entity may not be applied to a standalone document.
      if self.standalone == Some(true) && external_attlist {
        let name = self.pool.resolve(def.name).to_owned();
        let message = format!(
          "attribute \"{name}\" would take a default from the external subset, \
           which a standalone document may not depend on"
        );
        return Err(self.error(Error::well_formedness, message));
      }
      let start = self.attribute_text.len();
      if value.contains('&') {
        // The default carries a reference: expand it now, so an undeclared or recursive
        // entity in a default is caught just as it would be in a written attribute value.
        let mut out = std::mem::take(&mut self.attribute_text);
        let outcome = self.expand_at(value, &mut out, true, value, 0);
        self.attribute_text = out;
        outcome?;
        // The DTD normalized the default before its references were expanded, so a tokenized value is normalized
        // again over the expanded text, as a written value is.
        if def.att_type.is_tokenized() {
          let normalized = dtd::normalize_tokenized(&self.attribute_text[start..], true);
          self.attribute_text.truncate(start);
          self.attribute_text.push_str(&normalized);
        }
      } else {
        self.attribute_text.push_str(value);
      }
      let range = start..self.attribute_text.len();
      let lexical = self.pool.resolve(def.name).to_owned();
      let Some((prefix, local)) = chars::split_qname(&lexical) else {
        let message = format!("the DTD declares an attribute with the invalid name {lexical:?}");
        return Err(self.error(Error::well_formedness, message));
      };
      let declares_namespace = prefix == Some(XMLNS_PREFIX) || (prefix.is_none() && local == XMLNS_PREFIX);
      let name = QName::new(prefix.map(|p| self.pool.intern(p)), None, self.pool.intern(local));
      // A default was never written in the document, so it is located at the start tag it was supplied to.
      self.attributes.push(Attribute {
        name,
        value: range,
        declares_namespace,
        at: self.token_at.clone(),
        value_at: self.token_at.clone(),
      });
    }
    Ok(())
  }

  /// Applies the namespace declarations of the current start tag.
  fn declare_namespaces(&mut self) -> Result<()> {
    for i in 0..self.attributes.len() {
      let attribute = self.attributes[i].clone();
      if !attribute.declares_namespace {
        continue;
      }
      // `xmlns:p` declares p; plain `xmlns` declares the default namespace.
      let prefix = attribute.name.prefix.map(|_| attribute.name.local());
      let value = self.attribute_text[attribute.value].to_owned();

      if let Some(prefix) = prefix {
        let name = self.pool.resolve(prefix);
        let bad = if name == XMLNS_PREFIX {
          Some("the prefix \"xmlns\" cannot be declared".to_owned())
        } else if value.is_empty() {
          Some(format!("prefix \"{name}\" cannot be bound to an empty namespace name"))
        } else if name == XML_PREFIX && value != XML_NS_URI {
          Some("the prefix \"xml\" may only be bound to its own namespace name".to_owned())
        } else if name != XML_PREFIX && value == XML_NS_URI {
          Some(format!("the XML namespace may not be bound to \"{name}\""))
        } else if value == XMLNS_NS_URI {
          Some(format!("the namespace name of xmlns may not be bound to \"{name}\""))
        } else {
          None
        };
        if let Some(message) = bad {
          return Err(self.error(Error::namespace, message));
        }
      } else if value == XML_NS_URI || value == XMLNS_NS_URI {
        let message = format!("{value:?} may not be the default namespace");
        return Err(self.error(Error::namespace, message));
      }

      let namespace = (!value.is_empty()).then(|| self.pool.intern(&value));
      self.scope.bind(prefix, namespace);
    }
    Ok(())
  }

  fn resolve_element_name(&mut self, name: &str) -> Result<QName> {
    let (prefix, local) = split_name(name);
    let prefix = prefix.map(|p| self.pool.intern(p));
    let namespace = self.scope.resolve(prefix);
    if let Some(prefix) = prefix.filter(|_| namespace.is_none()) {
      return Err(self.undeclared_prefix(prefix));
    }
    Ok(QName::new(prefix, namespace, self.pool.intern(local)))
  }

  /// Binds attribute names to namespaces, once every declaration on the tag is in scope.
  fn resolve_attribute_names(&mut self) -> Result<()> {
    for i in 0..self.attributes.len() {
      let attribute = &self.attributes[i];
      let namespace = if attribute.declares_namespace {
        Some(NameId::XMLNS_NS)
      } else if let Some(prefix) = attribute.name.prefix {
        match self.scope.resolve(Some(prefix)) {
          Some(namespace) => Some(namespace),
          None => return Err(self.undeclared_prefix(prefix)),
        }
      } else {
        // An unprefixed attribute is in no namespace: the default namespace does not apply.
        None
      };
      let name = self.attributes[i].name;
      self.attributes[i].name = QName::new(name.prefix, namespace, name.local());
    }
    Ok(())
  }

  /// Computes `xml:space` and `xml:lang` for the element being entered.
  fn space_and_lang(&mut self) -> Result<(XmlSpace, Option<NameId>)> {
    let mut space = self.open.last().map_or(XmlSpace::Default, |e| e.xml_space);
    let mut lang = self.open.last().and_then(|e| e.xml_lang);
    for i in 0..self.attributes.len() {
      let attribute = self.attributes[i].clone();
      if attribute.name.namespace() != Some(NameId::XML_NS) {
        continue;
      }
      let value = self.attribute_text[attribute.value].to_owned();
      if attribute.name.local() == self.space_name {
        space = match value.as_str() {
          "default" => XmlSpace::Default,
          "preserve" => XmlSpace::Preserve,
          other => {
            // Located at the value rather than at the tag, since the value is what is wrong.
            let message = format!("xml:space must be \"default\" or \"preserve\", not {other:?}");
            return Err(Error::well_formedness(message).at(attribute.value_at.clone()));
          }
        };
      } else if attribute.name.local() == self.lang_name {
        lang = (!value.is_empty()).then(|| self.pool.intern(&value));
      }
    }
    Ok((space, lang))
  }

  /// Computes the base URI for the element being entered (XML Base): the enclosing base — the
  /// nearest ancestor's, or the entity's system identifier — overridden by this tag's
  /// `xml:base`, resolved against that enclosing base.
  fn element_base(&mut self) -> Result<Option<UriReference>> {
    if !self.config.extensions.xml_base {
      return Ok(None);
    }
    let inherited = self.open.last().map_or_else(|| self.stack.base_uri().cloned(), |e| e.base.clone());
    for i in 0..self.attributes.len() {
      let attribute = &self.attributes[i];
      if attribute.name.namespace() != Some(NameId::XML_NS) || attribute.name.local() != self.base_name {
        continue;
      }
      // XML Base §3.1: characters disallowed in a URI are escaped before the value is used.
      let escaped = crate::uri::escape_uri(&self.attribute_text[attribute.value.clone()]);
      let reference = UriReference::parse(&escaped).map_err(|e| {
        self.error(Error::uri, format!("xml:base value {escaped:?} is not a valid URI reference: {}", e.message()))
      })?;
      return Ok(Some(inherited.map_or_else(|| reference.clone(), |base| base.resolve(&reference))));
    }
    Ok(inherited)
  }

  /// Normalizes the `xml:id` attribute of the current tag as a tokenized ID, so its reported
  /// value is the ID even when no DTD declared it (xml:id §4).
  fn normalize_xml_id(&mut self) {
    if !self.config.extensions.xml_id {
      return;
    }
    for i in 0..self.attributes.len() {
      let attribute = &self.attributes[i];
      if attribute.name.namespace() != Some(NameId::XML_NS) || attribute.name.local() != self.id_name {
        continue;
      }
      let range = attribute.value.clone();
      let normalized = dtd::normalize_tokenized(&self.attribute_text[range], true);
      let start = self.attribute_text.len();
      self.attribute_text.push_str(&normalized);
      self.attributes[i].value = start..self.attribute_text.len();
    }
  }

  /// Records the lexical element name so its end tag can be compared with it.
  fn remember_name(&mut self, token: &str, from: usize, len: usize) -> Range<usize> {
    let start = self.names.len();
    self.names.push_str(&token[from..from + len]);
    start..self.names.len()
  }

  /// Expands references into `out`.
  ///
  /// With `attribute` set, the normalization of XML 1.0 §3.3.3 applies: whitespace written literally becomes a space,
  /// while whitespace written as a character reference is kept.
  ///
  fn expand_at(&mut self, text: &str, out: &mut String, attribute: bool, token: &str, base: usize) -> Result<()> {
    let mut rest = text;
    let mut done = 0;
    while let Some(i) = rest.find(['&', '<']) {
      out.push_str(&normalize(&rest[..i], attribute));
      done += i;
      if rest.as_bytes()[i] == b'<' {
        let message = "\"<\" may not appear in an attribute value; write \"&lt;\"";
        return Err(self.error_at(Error::well_formedness, message, token, base + done));
      }
      let reference = &rest[i..];
      let Some(end) = reference.find(';') else {
        let message = "a reference must end with \";\"; write \"&amp;\" for a literal ampersand";
        return Err(self.error_at(Error::well_formedness, message, token, base + done));
      };
      self.expand_reference(&reference[1..end], out, token, base + done)?;
      rest = &reference[end + 1..];
      done += end + 1;
    }
    out.push_str(&normalize(rest, attribute));
    Ok(())
  }

  /// Expands one reference, given its text between `&` and `;`.
  ///
  fn expand_reference(&mut self, body: &str, out: &mut String, token: &str, at: usize) -> Result<()> {
    if let Some(digits) = body.strip_prefix('#') {
      // `CharRef ::= '&#' [0-9]+ ';' | '&#x' [0-9a-fA-F]+ ';'`. The `x` is lower case only,
      // and neither form admits a sign, which `from_str_radix` would otherwise accept.
      let (digits, radix) = match digits.strip_prefix('x') {
        Some(hex) => (hex, 16),
        None => (digits, 10),
      };
      if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        let message = if radix == 16 {
          format!("\"&{body};\" is not a character reference; after \"&#x\" only 0-9, a-f and A-F may follow")
        } else {
          format!("\"&{body};\" is not a character reference; write \"&#\" and digits, or \"&#x\" and hex digits")
        };
        return Err(self.error_at(Error::well_formedness, message, token, at));
      }
      let code = u32::from_str_radix(digits, radix).ok();
      let Some(c) = code.and_then(char::from_u32).filter(|c| chars::is_char(*c)) else {
        // Almost always a NUL, a C0 control or half a surrogate pair; none can be escaped.
        let message = format!(
          "\"&{body};\" is not a character XML permits, and no escape can represent it \
           (XML 1.0 allows #x9, #xA, #xD, #x20-#xD7FF, #xE000-#xFFFD and #x10000-#x10FFFF)"
        );
        return Err(self.error_at(Error::well_formedness, message, token, at));
      };
      out.push(c);
      return Ok(());
    }
    match body {
      "lt" => out.push('<'),
      "gt" => out.push('>'),
      "amp" => out.push('&'),
      "apos" => out.push('\''),
      "quot" => out.push('"'),
      name if chars::is_name(name) => return self.expand_general_in_attribute(name, out, token, at),
      other => {
        let message = format!("\"&{other};\" is not a reference; write \"&amp;\" for a literal ampersand");
        return Err(self.error_at(Error::well_formedness, message, token, at));
      }
    }
    Ok(())
  }

  /// Expands a general entity referenced inside an attribute value.
  ///
  /// The replacement is processed recursively, so a `<` in it, or a reference to an external
  /// or unparsed entity, is caught here where XML 1.0 §3.1 and §4.1 forbid it.
  ///
  fn expand_general_in_attribute(&mut self, name: &str, out: &mut String, token: &str, at: usize) -> Result<()> {
    let id = self.pool.intern(name);
    if self.standalone == Some(true) && self.dtd.as_ref().is_some_and(|d| d.general_entity_is_external(id)) {
      let message =
        format!("entity \"{name}\" is declared in the external subset, which a standalone document may not depend on");
      return Err(self.error_at(Error::well_formedness, message, token, at));
    }
    let entity = self.dtd.as_ref().and_then(|dtd| dtd.general_entity(id)).cloned();
    match entity {
      Some(GeneralEntity::Internal { value }) => {
        if let Some(cycle) = self.expanding.iter().position(|&e| e == id) {
          // Trace the loop: from where this entity first opened, round through the others, back to it.
          let path = self.entity_chain(&self.expanding[cycle..], name);
          let message = format!("entity \"{name}\" refers to itself ({path})");
          return Err(self.error_at(Error::well_formedness, message, token, at));
        }
        if let Some(limit) = self.stack.limits().max_depth {
          if self.expanding.len() >= limit {
            let path = self.entity_chain(&self.expanding, name);
            let message = format!(
              "entities are nested more than {limit} deep ({path}); \
               raise ParserConfig.limits.entities.max_depth if the document is trusted"
            );
            return Err(Error::limit(message).at(self.token_at.clone()));
          }
        }
        self.expanding.push(id);
        // The replacement stands where a literal value would, so it normalizes the same way.
        let outcome = self.expand_at(&value, out, true, &value, 0);
        self.expanding.pop();
        outcome
      }
      Some(GeneralEntity::Unparsed { .. }) => {
        let message = format!("unparsed entity \"{name}\" may not be referenced in an attribute value");
        Err(self.error_at(Error::well_formedness, message, token, at))
      }
      Some(GeneralEntity::External { .. }) => {
        let message = format!("external entity \"{name}\" may not be referenced in an attribute value");
        Err(self.error_at(Error::well_formedness, message, token, at))
      }
      None => Err(self.undeclared_entity(name)),
    }
  }

  /// Renders an entity nesting path `ids` ending at `last` for debug or message purpose, abbreviating the middle when
  /// it is long.
  ///
  fn entity_chain(&self, ids: &[NameId], last: &str) -> String {
    let mut chain: Vec<&str> = ids.iter().map(|&id| self.pool.resolve(id)).collect();
    chain.push(last);
    if chain.len() > 12 {
      format!("{} -> ... -> {}", chain[..6].join(" -> "), chain[chain.len() - 6..].join(" -> "))
    } else {
      chain.join(" -> ")
    }
  }

  /// The current token as a borrowed [`TokenRef`], or `None` when there is no current token.
  ///
  /// It carries the same data as the individual accessors, matched as one enum instead of read field by field. The
  /// borrows last only until the next [`advance`](Self::advance). Call it after `advance` reports [`Progress::Token`].
  #[must_use]
  pub fn token_ref(&self) -> Option<TokenRef<'_>> {
    Some(match self.kind? {
      TokenKind::XmlDeclaration => TokenRef::XmlDeclaration {
        version: &self.version,
        encoding: self.declared_encoding.as_deref(),
        standalone: self.standalone,
      },
      TokenKind::Doctype => TokenRef::Doctype(&self.text),
      TokenKind::StartElement => TokenRef::StartElement {
        name: self.name,
        attributes: Attributes { attributes: &self.attributes, text: &self.attribute_text, pool: Some(&self.pool) },
        xml_space: self.xml_space,
        xml_lang: self.xml_lang.map(|l| self.pool.resolve(l)),
      },
      TokenKind::EndElement => TokenRef::EndElement { name: self.name },
      TokenKind::Text => TokenRef::Text(&self.text),
      TokenKind::CData => TokenRef::CData(&self.text),
      TokenKind::Comment => TokenRef::Comment(&self.text),
      TokenKind::ProcessingInstruction => {
        TokenRef::ProcessingInstruction { target: self.local_name(), data: &self.text, data_location: &self.pi_data_at }
      }
    })
  }

  /// The local part of the current element or processing-instruction name.
  ///
  #[must_use]
  pub fn local_name(&self) -> &str {
    self.pool.resolve(self.name.local())
  }

  /// The prefix of the current element name, if it has one.
  ///
  #[must_use]
  pub fn prefix(&self) -> Option<&str> {
    self.name.prefix.map(|p| self.pool.resolve(p))
  }

  /// The namespace name of the current element, if it is in one.
  ///
  #[must_use]
  pub fn namespace_uri(&self) -> Option<&str> {
    self.name.namespace().map(|n| self.pool.resolve(n))
  }

  /// The value of the attribute with this expanded name, if the current tag has it.
  ///
  /// Pass `None` for `namespace` to look for an unprefixed attribute.
  ///
  #[must_use]
  pub fn attribute_value(&self, namespace: Option<&str>, local: &str) -> Option<&str> {
    let namespace = match namespace {
      Some(name) => Some(self.pool.get(name)?),
      None => None,
    };
    let wanted = ExpandedName::new(namespace, self.pool.get(local)?);
    self.attributes.iter().find(|a| a.name.expanded == wanted).map(|a| &self.attribute_text[a.value.clone()])
  }

  /// The value of `xml:space` in effect for the current event.
  ///
  #[must_use]
  pub const fn xml_space(&self) -> XmlSpace {
    self.xml_space
  }

  /// The value of `xml:lang` in effect for the current event.
  ///
  #[must_use]
  pub fn xml_lang(&self) -> Option<&str> {
    self.xml_lang.map(|l| self.pool.resolve(l))
  }

  /// The base URI in effect for the current event (XML Base), if one is known.
  ///
  /// It is the entity's system identifier as overridden by the `xml:base` attributes in scope, resolved to an absolute
  /// (or the most resolved) URI. `None` when nothing establishes a base — no system identifier and no `xml:base` — or
  /// when [`Extensions::xml_base`] is off.
  ///
  #[must_use]
  pub fn base_uri(&self) -> Option<String> {
    self.base.as_ref().map(ToString::to_string)
  }

  /// The normalized `xml:id` of the current start element, if it carried one (xml:id).
  ///
  /// Tokenized normalization has been applied, so the value is already trimmed and collapsed. Whether it is a valid
  /// `NCName` and unique in the document is checked by the validation layer, which reuses the ID machinery for it.
  ///
  /// `None` when [`Extensions::xml_id`] is off.
  ///
  #[must_use]
  pub fn xml_id(&self) -> Option<&str> {
    if !self.config.extensions.xml_id {
      return None;
    }
    self
      .attributes
      .iter()
      .find(|a| a.name.namespace() == Some(NameId::XML_NS) && a.name.local() == self.id_name)
      .map(|a| &self.attribute_text[a.value.clone()])
  }

  /// The encoding actually applied to the bytes, once the stream has settled on one.
  ///
  /// The stream picks it from a byte-order mark, then the declaration, then the UTF-8 default, unless
  /// [`set_encoding`](Self::set_encoding) pinned it, and reports the codec's canonical name. `None` until enough of the
  /// input has been read to decide.
  ///
  /// This is the encoding that decoded the document, which may differ from the one the declaration named
  /// ([`TokenRef::XmlDeclaration`]'s `encoding`): a byte-order mark or a pinned encoding overrides the declaration, and
  /// the declaration's value is taken verbatim, so the declared `"utf-8"` decodes as the canonical `"UTF-8"`. Here the
  /// caller pins UTF-8, so the declaration's `UTF-16` is only what was named, not what decodes the document:
  ///
  /// ```
  /// # use xenolith::io::{CharStream, Entity, TokenKind, TokenRef, Parser, Progress};
  /// let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
  /// let mut parser = Parser::with_document(document);
  /// parser.feed("<?xml version='1.0' encoding='UTF-16'?><a/>".as_bytes(), true).unwrap();
  /// while !matches!(parser.advance().unwrap(), Progress::Token(TokenKind::XmlDeclaration)) {}
  /// let Some(TokenRef::XmlDeclaration { encoding, .. }) = parser.token_ref() else { unreachable!() };
  /// assert_eq!(encoding, Some("UTF-16")); // what the declaration named
  /// assert_eq!(parser.encoding(), Some("UTF-8")); // what actually decodes the bytes
  /// ```
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.stack.document().stream().encoding()
  }

  /// How deeply elements are nested; 0 represents outside the root element.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len()
  }

  /// The parser's current position, which sits at the end of the event just reported.
  ///
  /// For where the current event *begins*, use [`event_location`](Self::event_location).
  #[must_use]
  pub fn location(&self) -> Location {
    self.stack.location()
  }

  /// The source position where the current event begins.
  ///
  /// This is the start of the event's markup, the natural "location" of the event as an object. It differs from
  /// [`location`](Self::location), which is the parser's current position, at the end of the event just read.
  #[must_use]
  pub fn event_location(&self) -> Location {
    self.token_at.clone()
  }

  /// The pool holding every name the parser has seen.
  #[must_use]
  pub const fn pool(&self) -> &NamePool {
    &self.pool
  }

  /// The document type definition, once a `DOCTYPE` has been read; `None` before that, or if the document has no
  /// `DOCTYPE`.
  ///
  /// This is what a validator reads: the declared elements, attributes, entities and notations. It becomes available
  /// with the [`Doctype`](TokenKind::Doctype) event.
  #[must_use]
  pub const fn dtd(&self) -> Option<&Dtd> {
    self.dtd.as_ref()
  }

  /// The root element name the `DOCTYPE` declared, interned in [`pool`](Self::pool), or `None` with no `DOCTYPE`.
  ///
  /// It is the name right after `<!DOCTYPE`: for `<!DOCTYPE greeting SYSTEM "greeting.dtd">` this is the interned
  /// `greeting`, so [`pool`](Self::pool)`.resolve(id)` gives `"greeting"`. A valid document's root element must carry
  /// this name, which a validator checks.
  #[must_use]
  pub const fn doctype_name(&self) -> Option<NameId> {
    self.doctype_name
  }

  /// The public identifier of the `DOCTYPE`'s external subset, if it declared one with `PUBLIC`.
  ///
  /// For `<!DOCTYPE greeting PUBLIC "-//Example//DTD Greeting//EN" "greeting.dtd">` this is
  /// `Some("-//Example//DTD Greeting//EN")`; the `SYSTEM` form and no external subset both give `None`.
  #[must_use]
  pub fn doctype_public_id(&self) -> Option<&str> {
    self.doctype_public_id.as_deref()
  }

  /// The system identifier of the `DOCTYPE`'s external subset, if it declared one.
  ///
  /// It is the second literal of a `PUBLIC` identifier or the only one of a `SYSTEM` identifier: both
  /// `<!DOCTYPE greeting SYSTEM "greeting.dtd">` and `<!DOCTYPE greeting PUBLIC "-//Example//DTD Greeting//EN"
  /// "greeting.dtd">` give `Some("greeting.dtd")`; `None` with no external subset.
  #[must_use]
  pub fn doctype_system_id(&self) -> Option<&str> {
    self.doctype_system_id.as_deref()
  }

  /// The external entity the parser is waiting on, after [`advance`](Self::advance) returned
  /// [`Progress::NeedEntity`].
  ///
  #[must_use]
  pub const fn pending_entity(&self) -> Option<&EntityRequest> {
    self.pending_entity.as_ref()
  }

  /// Begins streaming an external general entity the parser requested, and resumes.
  ///
  /// Unlike [`provide_entity`](Self::provide_entity), which takes the whole entity at once, this opens an empty entity
  /// onto which the driver then feeds the bytes in chunks with [`feed`](Self::feed). The entity's text declaration is
  /// stepped over as it arrives, and the expansion limits are charged per chunk, so a large entity is neither held
  /// whole in memory nor read past the point a limit is exceeded.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Internal`] if the parser is not waiting for an entity (call it only after
  /// [`advance`](Self::advance) returned [`Progress::NeedEntity`]), or if the pending request is not a general entity;
  /// the DTD-side kinds have no streaming form and go through [`provide_entity`](Self::provide_entity). Also passes on
  /// the limit errors that guard against a hostile entity.
  pub fn begin_entity(&mut self) -> Result<()> {
    let Some(request) = self.pending_entity.as_ref() else {
      return Err(Error::internal("begin_entity called while the parser is not waiting for an entity"));
    };
    if request.kind() != RequestKind::GeneralEntity {
      return Err(Error::internal(
        "begin_entity is only for general entities; the DTD-side kinds go through provide_entity",
      ));
    }
    let request = self.pending_entity.take().expect("a pending entity was just inspected");
    let mut stream = CharStream::new();
    if let Some(id) = request.resolved_uri() {
      stream = stream.with_system_id(id);
    }
    // The identifiers travel with the entity, so a position inside it names the entity the way the document declared
    // it as well as the resource it was read from.
    if let Some(id) = request.public_id() {
      stream = stream.with_public_id(id);
    }
    let name = request.name().map(Into::into);
    self.stack.push(Entity::new(name, EntityKind::ExternalGeneral, stream, None))?;
    self.entity_text_decl_pending = true;
    Ok(())
  }

  /// Supplies the entity's full content that the parser requested, then resumes.
  ///
  /// The bytes are the entity's content as retrieved, its own encoding and text declaration included; the parser
  /// sniffs the encoding and strips the text declaration, then routes the content by request kind: it reads a general
  /// entity where the reference stood, appends the external subset to the DTD text, and splices an external parameter
  /// entity in where `%name;` stood. Unlike [`begin_entity`](Self::begin_entity), which streams a general entity in
  /// chunks, this takes the whole entity at once and serves every request kind.
  ///
  /// # Errors
  ///
  /// Returns [`Error::Internal`] if the parser is not waiting for an entity (call it only after
  /// [`advance`](Self::advance) returned [`Progress::NeedEntity`]), [`Error::Encoding`] if the bytes cannot be decoded,
  /// and passes on the limit errors that guard against a hostile entity.
  pub fn provide_entity(&mut self, bytes: &[u8]) -> Result<()> {
    let Some(request) = self.pending_entity.take() else {
      return Err(Error::internal("provide_entity called while the parser is not waiting for an entity"));
    };
    let system_id = request.resolved_uri();
    let mut stream = CharStream::new();
    if let Some(id) = system_id {
      stream = stream.with_system_id(id);
    }
    // As in `begin_entity`. The DTD-side kinds take the text below with the location it begins at, so the identifiers
    // reach a reported position through every arm.
    if let Some(id) = request.public_id() {
      stream = stream.with_public_id(id);
    }
    stream.feed(bytes, true)?;
    decl::strip_text_declaration(&mut stream)?;

    match request.kind() {
      RequestKind::GeneralEntity => {
        let name = request.name().map(Into::into);
        self.stack.push(Entity::new(name, EntityKind::ExternalGeneral, stream, None))
      }
      // The external subset is DTD text: append it after the internal subset and resume.
      RequestKind::ExternalSubset => {
        self.dtd_assembly.add_external_subset(stream.remainder(), stream.location());
        Ok(())
      }
      // An external parameter entity's content replaces the `%name;` that summoned it.
      RequestKind::ParameterEntity => {
        self.dtd_assembly.provide_parameter_entity(stream.remainder(), stream.location());
        Ok(())
      }
    }
  }

  /// Reports that the caller could not resolve the entity the parser requested.
  ///
  /// The external subset is optional for a non-validating processor, so the parser skips it and continues; a later
  /// reference to an entity declared only in that subset then fails as undeclared. A general or parameter entity
  /// cannot be skipped, so declining one is a fatal well-formedness error.
  ///
  /// # Errors
  ///
  /// [`Error::WellFormedness`] for a general or parameter entity that cannot be resolved. Returns [`Error::Internal`]
  /// if the parser is not waiting for an entity.
  pub fn decline_entity(&mut self) -> Result<()> {
    let Some(request) = self.pending_entity.take() else {
      return Err(Error::internal("decline_entity called while the parser is not waiting for an entity"));
    };
    if request.kind() == RequestKind::ExternalSubset {
      self.external_subset_unread = true;
      self.dtd_assembly.discard_pending();
      return Ok(());
    }
    let what = request.name().map_or_else(|| "an external entity".to_owned(), |name| format!("entity \"{name}\""));
    Err(self.error(Error::well_formedness, format!("{what} could not be resolved")))
  }

  /// Builds the namespace error for a prefix with no binding in scope, giving the fix (add an `xmlns:` declaration).
  fn undeclared_prefix(&self, prefix: NameId) -> Error {
    let name = self.pool.resolve(prefix);
    let message =
      format!("prefix \"{name}\" is not bound; add an xmlns:{name} attribute to this element or an ancestor");
    self.error(Error::namespace, message)
  }

  /// Builds an error located at the start of the token being interpreted (`token_at`).
  ///
  /// `build` is one of [`Error`]'s per-kind constructors (for example, [`Error::well_formedness`]) and `message` is
  /// its human-readable text.
  fn error(&self, build: fn(String) -> Error, message: impl Into<String>) -> Error {
    build(message.into()).at(self.token_at.clone())
  }

  /// Builds an error located `index` bytes into `token`, pointing to a specific character rather than the token start.
  ///
  /// It advances the token's start location over `token[..index]`, so `index` is a byte offset within `token`, at a
  /// character boundary. `build` and `message` are as in [`error`](Self::error).
  fn error_at(&self, build: fn(String) -> Error, message: impl Into<String>, token: &str, index: usize) -> Error {
    build(message.into()).at(self.location_in(token, index))
  }

  /// The location `index` bytes into `token`, which is the token's own start advanced over `token[..index]`. `index` is
  /// a byte offset within `token`, at a character boundary.
  fn location_in(&self, token: &str, index: usize) -> Location {
    let mut at = self.token_at.clone();
    for c in token[..index.min(token.len())].chars() {
      at.advance(c);
    }
    at
  }
}

/// Splits a lexical name into its prefix and local part at its first colon, leniently.
///
/// The parser splits to resolve a namespace, not to judge the name: a name with no colon, or one whose prefix or local
/// part would be empty, is kept whole as the local part, and whatever the pieces contain is left for
/// [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator) to refuse. Splitting at the first colon only means
/// `a:b:c` becomes prefix `a` with local part `b:c`, which that validator reports as not a `QName`.
fn split_name(name: &str) -> (Option<&str>, &str) {
  match name.split_once(':') {
    Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => (Some(prefix), local),
    _ => (None, name),
  }
}

/// Returns the byte length of the whitespace run at the start of `text`, or 0 when `text` does not start with
/// whitespace. Callers use it to require an `S` (mandatory whitespace) between parts of a construct.
fn whitespace_len(text: &str) -> usize {
  text.len() - text.trim_start_matches(chars::is_whitespace).len()
}

/// Parses an `ExternalID` into its public and system identifiers, and the text left after them.
///
/// `ExternalID ::= 'SYSTEM' S SystemLiteral | 'PUBLIC' S PubidLiteral S SystemLiteral`. On success, this returns the
/// public identifier (`None` for the `SYSTEM` form), the always-present system identifier, and the remainder after the
/// last literal, which the caller checks is only whitespace. It returns `None` when `text` is not shaped like an
/// `ExternalID`.
fn parse_external_id(text: &str) -> Option<(Option<String>, String, &str)> {
  fn read_literal(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start_matches(chars::is_whitespace);
    let quote = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &s[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some((rest[..end].to_owned(), &rest[end + quote.len_utf8()..]))
  }
  if let Some(rest) = text.strip_prefix("SYSTEM") {
    if !rest.starts_with(chars::is_whitespace) {
      return None;
    }
    let (system, tail) = read_literal(rest)?;
    Some((None, system, tail))
  } else if let Some(rest) = text.strip_prefix("PUBLIC") {
    if !rest.starts_with(chars::is_whitespace) {
      return None;
    }
    let (public, after_public) = read_literal(rest)?;
    // A system literal must be separated from the public one by whitespace.
    if !after_public.starts_with(chars::is_whitespace) {
      return None;
    }
    let (system, tail) = read_literal(after_public)?;
    Some((Some(public), system, tail))
  } else {
    None
  }
}

/// Normalizes one literal fragment of an attribute value per XML 1.0 §3.3.3: a tab or newline becomes a space.
///
/// The stream has already folded CR and CRLF to LF (end-of-line handling), and a space needs no change, so only tabs
/// and newlines remain. This folds literal whitespace only; whitespace written as a character reference (`&#9;`) never
/// reaches here, because the caller resolves references separately, which is what keeps it a tab. With `attribute`
/// false, this returns `text` untouched, since text content keeps its whitespace.
fn normalize(text: &str, attribute: bool) -> Cow<'_, str> {
  if !attribute || !text.contains(['\t', '\n']) {
    return Cow::Borrowed(text);
  }
  // A literal tab or newline in an attribute value is uncommon, so this path is rarely taken; the second scan that
  // `replace` makes over the string is not worth avoiding, while the common case above returns borrowed after one scan.
  Cow::Owned(text.replace(['\t', '\n'], " "))
}
