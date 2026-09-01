//! The sans-I/O parser core.
//!
//! [`Parser`] takes bytes through [`feed`](Parser::feed) and yields one step at a time through
//! [`advance`](Parser::advance), which returns a [`Progress`] telling the caller what to do next. The parser reads no
//! files and opens no sockets, so the same core drives a blocking reader, an async reader, and an in-memory slice, and
//! it can stop between two tokens without holding a thread.
//!
//! The caller reads an event's data through accessors that borrow from the parser rather than through events that own
//! their data, so once the buffers have grown, the parser allocates nothing per event.
//!
//! This is the low level; [`Reader`](crate::Reader) wraps it with the I/O and entity-resolution loop that most callers
//! want.
//!

use std::borrow::Cow;
use std::ops::Range;

use xenolith_core::attr::{AttributeList, AttributeRef};
use xenolith_core::chars;
use xenolith_core::error::{Error, Location, Result};
use xenolith_core::name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};
#[cfg(feature = "xml-base")]
use xenolith_core::uri::UriReference;

use crate::config::{Bounds, ParserConfig};
use crate::dtd::{self, Dtd, GeneralEntity};
use crate::entity::{Entity, EntityKind, EntityStack, Limits};
use crate::event::Event;
use crate::namespace::NamespaceScope;
use crate::resolve::{EntityRequest, RequestKind};
use crate::scan::{Scan, Token, scan};
use crate::stream::CharStream;

/// What a call to [`Parser::advance`] achieved.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Progress {
  /// The parser produced an event. Read its data through the accessors the [`EventKind`] names; the values belong to
  /// this event only, and the next [`advance`](Parser::advance) clears them.
  ///
  Event(EventKind),
  /// The parser needs more bytes to decide the next step. The driver supplies them with [`feed`](Parser::feed), setting
  /// `last` on the final chunk, then calls again.
  ///
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
  /// A blocking driver does all this through a [`UriResolver`](crate::resolve::UriResolver); most callers use a
  /// [`Reader`](crate::Reader) and never see this variant.
  ///
  NeedEntity,
  /// The document is complete; no more events follow, and the driver stops.
  ///
  Eof,
}

/// The kind of event the parser is reporting, carried by [`Progress::Event`].
///
/// It names only the kind, carrying none of the event's data and so no borrow of the parser; that is what lets
/// [`advance`](Parser::advance) report it by value while the caller stays free to [`feed`](Parser::feed) more input. The
/// data is read separately through [`Parser::event_ref`], whose [`EventRef`] variant matches the kind named here, and
/// each variant below points at that counterpart. Those borrowed values are current only until the next
/// [`advance`](Parser::advance).
///
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventKind {
  /// The XML declaration: its version, encoding, and standalone flag are in [`EventRef::XmlDeclaration`].
  ///
  XmlDeclaration,
  /// A document type declaration: its verbatim text is [`EventRef::Doctype`], and its parsed root name and external
  /// identifiers are on the parser ([`doctype_name`](Parser::doctype_name),
  /// [`doctype_public_id`](Parser::doctype_public_id), [`doctype_system_id`](Parser::doctype_system_id)). The internal
  /// subset is parsed into the DTD rather than reported here.
  ///
  Doctype,
  /// The start of an element, or a whole empty element: its name and attributes are in [`EventRef::StartElement`].
  ///
  StartElement,
  /// The end of an element, including the implied end of an empty element: its name is in [`EventRef::EndElement`].
  ///
  EndElement,
  /// Character data, in [`EventRef::Text`].
  ///
  /// One run of character data is not always one event: a long run is delivered as several adjacent `Text` events so
  /// it is not buffered without bound, and a reference or entity boundary within a run also splits it. A consumer that
  /// wants one maximal text node coalesces adjacent `Text` events, as the DOM tree builder does.
  ///
  Text,
  /// The content of a CDATA section, in [`EventRef::CData`]. Reported separately from text because the DOM and the
  /// serializer both need to know where the section boundaries were.
  ///
  CData,
  /// A comment without its delimiters, in [`EventRef::Comment`].
  ///
  Comment,
  /// A processing instruction: its target and data are in [`EventRef::ProcessingInstruction`].
  ///
  ProcessingInstruction,
}

/// The current event's data, as an enum that borrows it from the parser. Each variant provides only the data specific
/// to that event type.
///
/// [`Parser::event_ref`] returns it after [`advance`](Parser::advance) reports [`Progress::Event`]. Match it for the
/// kind that was reported, or, to reach across kinds without a `match`, use [`name`](Self::name), [`text`](Self::text),
/// and [`attributes`](Self::attributes). The borrows are valid only until the next [`advance`](Parser::advance); an
/// event that must outlive the call can be copied into the owned [`Event`] with [`Event::capture`].
///
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum EventRef<'a> {
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

impl<'a> EventRef<'a> {
  /// Which kind of event this is.
  #[must_use]
  pub const fn kind(&self) -> EventKind {
    match self {
      Self::XmlDeclaration { .. } => EventKind::XmlDeclaration,
      Self::Doctype(_) => EventKind::Doctype,
      Self::StartElement { .. } => EventKind::StartElement,
      Self::EndElement { .. } => EventKind::EndElement,
      Self::Text(_) => EventKind::Text,
      Self::CData(_) => EventKind::CData,
      Self::Comment(_) => EventKind::Comment,
      Self::ProcessingInstruction { .. } => EventKind::ProcessingInstruction,
    }
  }

  /// The element's name for a start or end element, or `None` for other kinds.
  ///
  /// For a start element it is also in the [`name`](Self::StartElement) field, and for an end element in the
  /// [`name`](Self::EndElement) field; this reaches whichever of the two applies without a `match`.
  ///
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
  ///
  #[must_use]
  pub const fn text(&self) -> Option<&'a str> {
    match self {
      Self::Text(text) | Self::CData(text) | Self::Comment(text) => Some(text),
      _ => None,
    }
  }

  /// The attributes of a start element, in document order and namespace declarations included, or an empty view for
  /// other kinds.
  ///
  #[must_use]
  pub fn attributes(&self) -> Attributes<'a> {
    match self {
      Self::StartElement { attributes, .. } => *attributes,
      _ => Attributes { attributes: &[], text: "" },
    }
  }
}

/// The `xml:space` handling in effect, taken from the nearest element in scope that set it; see [`Parser::xml_space`].
///
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XmlSpace {
  /// No `xml:space` is in scope, or the nearest one says `default`.
  #[default]
  Default,
  /// `xml:space="preserve"` is in scope.
  Preserve,
}

/// The attributes of a start element, a borrowing view that yields [`AttributeRef`]. [`EventRef::StartElement`]
/// carries one; iterate it with [`iter`](Self::iter) or index it with [`get`](Self::get).
///
/// It implements [`AttributeList`], so a source-independent consumer, a validator or a push handler, reads it through
/// an [`Attributes`](xenolith_core::attr::Attributes) view.
///
#[derive(Clone, Copy, Debug)]
pub struct Attributes<'a> {
  attributes: &'a [Attribute],
  text: &'a str,
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
    self.attributes.get(index).map(|a| AttributeRef {
      name: a.name,
      value: &self.text[a.value.clone()],
      declares_namespace: a.declares_namespace,
    })
  }

  /// Iterates the attributes in document order.
  pub fn iter(&self) -> impl Iterator<Item = AttributeRef<'a>> {
    let (attributes, text) = (self.attributes, self.text);
    (0..attributes.len()).filter_map(move |i| {
      attributes.get(i).map(|a| AttributeRef {
        name: a.name,
        value: &text[a.value.clone()],
        declares_namespace: a.declares_namespace,
      })
    })
  }
}

/// One attribute of the current start tag, stored internally; [`AttributeRef`] is the borrowed view handed to callers.
///
#[derive(Clone, Debug)]
struct Attribute {
  name: QName,
  /// The normalized value is a byte range into the parser's `attribute_text` buffer, so every value shares one
  /// allocation that the parser reuses across elements rather than each owning a `String`.
  ///
  value: Range<usize>,
  declares_namespace: bool,
}

/// An element whose start tag has been read but whose end tag has not: the state the parser keeps while it is open.
///
#[derive(Debug)]
struct OpenElement {
  /// The element's expanded name, reported when it closes.
  ///
  name: QName,
  /// The element's name as written, a range into `self.names`, compared against the end tag's raw name.
  ///
  lexical: Range<usize>,
  /// The namespace-scope position to roll back to when the element closes, dropping the bindings it declared.
  ///
  namespace_mark: usize,
  /// The `xml:space` in effect within the element.
  ///
  xml_space: XmlSpace,
  /// The `xml:lang` in effect within the element, if any.
  ///
  xml_lang: Option<NameId>,
  /// The base URI in effect within this element: the enclosing base, resolved with an `xml:base` attribute if the tag
  /// carried one (XML Base).
  ///
  #[cfg(feature = "xml-base")]
  base: Option<UriReference>,
  /// The entity depth at which the start tag was read. An end tag must be read at the same depth, so an element cannot
  /// start in one entity and end in another (WFC: an element's tags must lie within one entity).
  entity_depth: usize,
}

/// A markup token scanned but not yet interpreted, because pending text had to be emitted
/// first. Holding it lets a text run and the markup that ends it be reported in that order.
#[derive(Debug)]
struct Held {
  token: Token,
  text: String,
  at: Location,
}

/// The result of looking for a text declaration at the start of an external entity.
///
/// An external entity may open with `<?xml ... ?>` (the `TextDecl` production). The stream reads it to choose the
/// encoding but leaves it in the character input, so the parser steps over a [`Present`](TextDecl::Present) span
/// rather than report it as a processing instruction. [`NeedMore`](TextDecl::NeedMore) arises only while an entity
/// arrives in pieces and its start has not fully landed.
///
enum TextDecl {
  /// The entity does not open with a text declaration.
  ///
  None,
  /// The entity opens with a text declaration this many bytes long; the parser steps over it.
  ///
  Present(usize),
  /// The input read so far is too little to decide; the parser requests more.
  ///
  NeedMore,
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
/// use xenolith_parser::{EventKind, Parser, Progress};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<greeting xml:lang='en'>Hello</greeting>", true)?;
///
/// let mut kinds = Vec::new();
/// loop {
///   match parser.advance()? {
///     Progress::Event(kind) => kinds.push(kind),
///     Progress::Eof => break,
///     // `Progress` grows as later phases land, so a catch-all arm is required.
///     other => panic!("unexpected {other:?}: the whole document was fed at once"),
///   }
/// }
/// assert_eq!(kinds, [EventKind::StartElement, EventKind::Text, EventKind::EndElement]);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// Values are read through accessors while the event is current:
///
/// ```
/// use xenolith_parser::{EventKind, Parser, Progress};
///
/// let mut parser = Parser::new();
/// parser.feed(b"<t:table xmlns:t='urn:t' rows='2'/>", true)?;
///
/// assert_eq!(parser.advance()?, Progress::Event(EventKind::StartElement));
/// assert_eq!(parser.local_name(), "table");
/// assert_eq!(parser.prefix(), Some("t"));
/// assert_eq!(parser.namespace_uri(), Some("urn:t"));
/// assert_eq!(parser.attribute_value(None, "rows"), Some("2"));
///
/// // An empty element reports its end as a separate event.
/// assert_eq!(parser.advance()?, Progress::Event(EventKind::EndElement));
/// assert_eq!(parser.advance()?, Progress::Eof);
/// # Ok::<(), xenolith_core::Error>(())
/// ```
#[derive(Debug)]
pub struct Parser {
  stack: EntityStack,
  pool: NamePool,
  config: ParserConfig,
  bounds: Bounds,
  space_name: NameId,
  lang_name: NameId,
  /// The interned local name `base`, for spotting `xml:base` attributes.
  #[cfg(feature = "xml-base")]
  base_name: NameId,
  /// The interned local name `id`, for spotting `xml:id` attributes.
  #[cfg(feature = "xml-id")]
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
  /// The DTD text being parsed — internal subset then external subset — grown as parameter
  /// entities are spliced in. Non-empty only while the DTD is being parsed.
  dtd_buf: String,
  /// Byte length of the internal subset within [`dtd_buf`](Self::dtd_buf).
  dtd_internal_len: usize,
  /// The external subset's identifier, until it has been fetched.
  dtd_external_id: Option<(Option<String>, String)>,
  /// True while the DTD is being parsed: `advance` drives it before the `Doctype` event.
  dtd_active: bool,
  /// The external parameter entity being fetched, so its content can be spliced back in.
  dtd_pe: Option<dtd::ExternalPe>,
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
  kind: Option<EventKind>,
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
  /// The XML declaration names the encoding. The stream layer sniffed the encoding from the bytes and is already
  /// decoding with it, so this copy is read only to report the declaration, never to pick a codec here.
  ///
  declared_encoding: Option<String>,
  standalone: Option<bool>,
  xml_space: XmlSpace,
  xml_lang: Option<NameId>,
  /// The base URI in effect for the current event (XML Base).
  #[cfg(feature = "xml-base")]
  base: Option<UriReference>,
}

impl Default for Parser {
  fn default() -> Self {
    Self::new()
  }
}

impl Parser {
  /// Creates a parser that sniffs the document's encoding from its bytes, with default limits and no system identifier.
  ///
  /// Use [`with_document`](Self::with_document) to set the system identifier, pin the encoding, or change the limits.
  ///
  #[must_use]
  pub fn new() -> Self {
    Self::with_document(Entity::document(CharStream::new()), Limits::default())
  }

  /// Creates a parser over a prepared document entity, bounded by `limits`.
  ///
  /// Use this when you know the system identifier, encoding, or limits in advance. The document stream's system
  /// identifier becomes the base URI and the origin of error locations; an encoding set with
  /// [`CharStream::with_encoding`] pins decoding instead of sniffing it; and `limits` caps the whole-document work
  /// (see [`Limits`]).
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_parser::{CharStream, Entity, Limits, Parser};
  ///
  /// let document = Entity::document(CharStream::with_encoding("UTF-8")?.with_system_id("file:///doc.xml"));
  /// let mut parser = Parser::with_document(document, Limits::default().with_max_depth(16));
  /// parser.feed(b"<a/>", true)?;
  /// parser.advance()?;
  /// assert_eq!(parser.location().system_id.as_deref(), Some("file:///doc.xml"));
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  #[must_use]
  pub fn with_document(document: Entity, limits: Limits) -> Self {
    let mut pool = NamePool::new();
    let space_name = pool.intern("space");
    let lang_name = pool.intern("lang");
    #[cfg(feature = "xml-base")]
    let base_name = pool.intern("base");
    #[cfg(feature = "xml-id")]
    let id_name = pool.intern("id");
    Self {
      stack: EntityStack::new(document, limits),
      pool,
      config: ParserConfig::default(),
      bounds: Bounds::default(),
      space_name,
      lang_name,
      #[cfg(feature = "xml-base")]
      base_name,
      #[cfg(feature = "xml-id")]
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
      dtd_buf: String::new(),
      dtd_internal_len: 0,
      dtd_external_id: None,
      dtd_active: false,
      dtd_pe: None,
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
      #[cfg(feature = "xml-base")]
      base: None,
    }
  }

  /// Replaces the parser's configuration; set it before parsing begins.
  ///
  /// Its options (`xml:base`, `xml:id`) take effect only where the matching Cargo feature is compiled in; see
  /// [`ParserConfig`].
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_parser::{Parser, ParserConfig};
  ///
  /// let mut parser = Parser::new();
  /// parser.set_config(ParserConfig::none());
  /// ```
  pub fn set_config(&mut self, config: ParserConfig) {
    self.config = config;
  }

  /// The configuration in effect; change it with [`set_config`](Self::set_config).
  ///
  #[must_use]
  pub const fn config(&self) -> &ParserConfig {
    &self.config
  }

  /// Sets the per-token byte-length bounds the scanner enforces; set them before parsing begins.
  ///
  /// The default [`Bounds`] already caps each markup token generously. [`Bounds::unlimited`] lifts those caps for
  /// trusted input, and a tighter value rejects a single token that grows past its limit in input that is not. These
  /// bound one token at a time; whole-document work such as nesting depth and entity expansion is capped by [`Limits`]
  /// instead.
  ///
  pub fn set_bounds(&mut self, bounds: Bounds) {
    self.bounds = bounds;
  }

  /// The per-token byte-length bounds in effect; change them with [`set_bounds`](Self::set_bounds).
  ///
  #[must_use]
  pub const fn bounds(&self) -> &Bounds {
    &self.bounds
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
  ///
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
  ///
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    self.stack.feed(bytes, last)
  }

  /// Clears the per-event output so no accessor reports a value an earlier event left behind; each event's handler
  /// then sets what it reports. Document-level state (the XML declaration and `DOCTYPE` metadata) is not per-event
  /// and stays in place.
  ///
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
  /// - [`Event`](Progress::Event) carries the [`EventKind`]. Read the event's data through [`event_ref`](Self::event_ref),
  ///   whose [`EventRef`] variant matches that kind (for example, [`EventRef::StartElement`] carries a start element's
  ///   name and attributes); its values belong to this event only.
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
  /// A [`Reader`](crate::Reader) runs this loop and resolves entities through a
  /// [`UriResolver`](crate::resolve::UriResolver), so most callers never call `advance` directly.
  ///
  /// # Errors
  ///
  /// Returns [`Error::WellFormedness`] or [`Error::Namespace`] for a document that breaks the rules, and passes on
  /// decoding and limit errors.
  ///
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
      self.kind = Some(EventKind::EndElement);
      return Ok(Progress::Event(EventKind::EndElement));
    }
    loop {
      // A markup token held back while the text before it was flushed.
      if let Some(held) = self.held.take() {
        self.token_at = held.at;
        if let Some(kind) = self.interpret(held.token, &held.text)? {
          self.kind = Some(kind);
          return Ok(Progress::Event(kind));
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
        match self.text_declaration_span(rem, last).map_err(|e| e.at(self.stack.location()))? {
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
          match scan(rest, no_more_input, &self.bounds).map_err(|e| e.at(self.stack.location()))? {
            Scan::Found(token, len) => Some((token, len)),
            Scan::Pending => return Ok(Progress::NeedMoreInput),
          }
        }
      };

      let Some((token, len)) = scanned else {
        // The innermost entity is exhausted. Resume the one that referenced it, or, at the
        // document entity, flush any last text and end.
        if self.stack.depth() > 1 {
          self.stack.pop();
          continue;
        }
        if let Some(kind) = self.flush_text()? {
          return Ok(Progress::Event(kind));
        }
        return self.finish();
      };

      let at = self.stack.location();
      // Copy the token out of the stream into `token`, then take it, so the handlers can borrow `self` while reading it;
      // the text and reference arms move the buffer back afterwards to reuse its capacity.
      self.token.clear();
      self.token.push_str(&self.stack.current().stream().remainder()[..len]);
      self.stack.current_mut().stream_mut().advance(len);
      let text = std::mem::take(&mut self.token);

      let event = match token {
        Token::Text => {
          let outcome = self.accumulate_text(&text, &at);
          self.token = text;
          outcome?;
          self.flush_if_full()?
        }
        Token::Reference => {
          let outcome = self.reference(&text, &at);
          self.token = text;
          outcome?;
          self.flush_if_full()?
        }
        _ => {
          // Markup ends a text run: flush the text first and hold the markup for next time.
          if self.pending_text.is_empty() {
            self.token_at = at;
            let outcome = self.interpret(token, &text);
            self.token = text;
            outcome?
          } else {
            self.held = Some(Held { token, text, at });
            self.flush_text()?
          }
        }
      };

      if let Some(kind) = event {
        self.kind = Some(kind);
        return Ok(Progress::Event(kind));
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
  /// written
  /// after so much as a newline had its DTD left unparsed — so the next token was scanned first and the `Doctype`
  /// event came out *after* the root element's start tag. Everything downstream believed it: a validator built when
  /// the DOCTYPE arrived never saw the root element open, and unbalanced its stack on the way out.
  ///
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
  ///
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
  /// `token` is the kind the scanner settled on, and it selects the handler. Only markup reaches here: a start or end
  /// tag, a comment, a CDATA section, a processing instruction, or a `DOCTYPE`. `advance` accumulates text and
  /// references itself, so they never arrive here.
  ///
  /// `text` is that token's source as scanned, delimiters and all (`<a x="1">`, `<!-- c -->`, `<?t d?>`,
  /// `<![CDATA[...]]>`, `<!DOCTYPE ...>`); each handler strips its own. It is borrowed, so interpreting allocates
  /// nothing for it.
  ///
  /// A returned `Some(kind)` is an event to report. `None` means the token was interpreted but yields no event here: a
  /// `<!DOCTYPE>` sets DTD parsing in motion and reports `Doctype` only once that finishes.
  ///
  fn interpret(&mut self, token: Token, text: &str) -> Result<Option<EventKind>> {
    if !matches!(token, Token::StartTag | Token::EndTag) {
      // A start or end tag sets its own context in its handler; every other token (comment, CDATA, PI, DOCTYPE)
      // inherits the enclosing element's xml:space, xml:lang, and base URI so the accessors report them for this event.
      self.xml_space = self.open.last().map_or(XmlSpace::Default, |e| e.xml_space);
      self.xml_lang = self.open.last().and_then(|e| e.xml_lang);
      #[cfg(feature = "xml-base")]
      {
        self.base = if self.config.xml_base {
          self.open.last().map_or_else(|| self.stack.base_uri().cloned(), |e| e.base.clone())
        } else {
          None
        };
      }
    }
    match token {
      Token::Pi => self.processing_instruction(text),
      Token::Comment => self.comment(text).map(Some),
      Token::Doctype => self.doctype(text),
      Token::StartTag => self.start_tag(text).map(Some),
      Token::EndTag => self.end_tag(text).map(Some),
      Token::CData => self.cdata(text).map(Some),
      // Text and references never reach here: `advance` accumulates them itself.
      Token::Text | Token::Reference => Ok(None),
    }
  }

  /// Splits `<?...?>` into the XML declaration and ordinary processing instructions.
  ///
  fn processing_instruction(&mut self, text: &str) -> Result<Option<EventKind>> {
    debug_assert!(text.starts_with("<?") && text.ends_with("?>"));
    let body = &text[2..text.len() - 2];
    let target_len = body.find(chars::is_whitespace).unwrap_or(body.len());
    let (target, data) = body.split_at(target_len);

    if target.eq_ignore_ascii_case("xml") {
      // The stream leaves the XML declaration in the character input (it read it only to sniff the encoding), so it
      // arrives here as a `<?xml...?>` token. Only a genuine declaration, at the very start of the document entity, is
      // allowed.
      if target != "xml" || self.token_at.offset != 0 || self.stack.depth() > 1 {
        let message = format!("\"{target}\" is a reserved target");
        return Err(self.error(Error::well_formedness, message));
      }
      self.xml_declaration(data)?;
      return Ok(Some(EventKind::XmlDeclaration));
    }
    if !chars::is_name(target) {
      let message = format!("{target:?} is not a valid processing instruction target");
      return Err(self.error(Error::well_formedness, message));
    }
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
    Ok(Some(EventKind::ProcessingInstruction))
  }

  /// Reads the pseudo-attributes of the XML declaration.
  ///
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
      let (name, value, tail) = self.pseudo_attribute(rest, "XML declaration")?;
      // A repeat of any pseudo-attribute is rejected up front. `seen` only ever holds the three known names, so an
      // unknown one never matches here and falls to the `other` arm below to be named as unknown.
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

  /// Reads one `name = "value"` of the XML or text declaration, returning it and what follows.
  ///
  /// `decl` names the enclosing declaration for the error message, since both share this pseudo-attribute syntax.
  ///
  fn pseudo_attribute<'t>(&self, rest: &'t str, decl: &str) -> Result<(&'t str, &'t str, &'t str)> {
    let malformed = |what: &str| self.error(Error::well_formedness, format!("the {decl} {what}"));
    let rest = rest.trim_start_matches(chars::is_whitespace);
    let name_len = rest.find(|c: char| c == '=' || chars::is_whitespace(c)).unwrap_or(rest.len());
    let (name, rest) = rest.split_at(name_len);
    let rest = rest.trim_start_matches(chars::is_whitespace);
    let rest = rest.strip_prefix('=').ok_or_else(|| malformed("is missing an \"=\""))?;
    let rest = rest.trim_start_matches(chars::is_whitespace);
    let quote =
      rest.chars().next().filter(|c| *c == '"' || *c == '\'').ok_or_else(|| malformed("has an unquoted value"))?;
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote).ok_or_else(|| malformed("has an unterminated value"))?;
    Ok((name, &rest[..end], &rest[end + quote.len_utf8()..]))
  }

  /// Consumes and checks a leading text declaration on a fully-read external entity's stream.
  ///
  /// This is the whole-buffer path, used when the entity's bytes arrive all at once. The incremental driver uses
  /// [`text_declaration_span`](Self::text_declaration_span) directly.
  ///
  fn strip_text_declaration(&self, stream: &mut CharStream) -> Result<()> {
    let at = stream.location();
    let len = match self.text_declaration_span(stream.remainder(), true).map_err(|e| e.at(at))? {
      TextDecl::Present(len) => len,
      TextDecl::None => return Ok(()),
      TextDecl::NeedMore => {
        return Err(Error::internal("a completed entity still requested more of its text declaration"));
      }
    };
    stream.advance(len);
    Ok(())
  }

  /// Measures a leading text declaration on an external entity, without consuming it.
  ///
  /// `TextDecl ::= '<?xml' VersionInfo? EncodingDecl S? '?>'`: the encoding is required, the version optional and, if
  /// present, first, and — unlike the XML declaration — there is no standalone. The stream already read it to choose
  /// the encoding; this checks its shape and reports how many bytes to step over so it does not reach the parser as a
  /// processing instruction. `last` is true once the entity has been read to its end; while it is false and the
  /// declaration is not yet complete, [`TextDecl::NeedMore`] requests more input. Errors carry no location; the caller
  /// adds one.
  ///
  fn text_declaration_span(&self, rem: &str, last: bool) -> Result<TextDecl> {
    const HEAD: &str = "<?xml";
    // Too little read to tell `<?xml` from a shorter prefix, or its following character apart.
    if !last && rem.len() <= HEAD.len() && HEAD.starts_with(rem) {
      return Ok(TextDecl::NeedMore);
    }
    let Some(after) = rem.strip_prefix(HEAD) else { return Ok(TextDecl::None) };
    // `<?xmlfoo` is not a declaration; a real one is followed by whitespace.
    match after.chars().next() {
      None if !last => return Ok(TextDecl::NeedMore),
      Some(c) if chars::is_whitespace(c) => {}
      _ => return Ok(TextDecl::None),
    }
    let malformed = |what: &str| Error::well_formedness(format!("the text declaration {what}"));
    let Some(end) = rem.find("?>") else {
      return if last { Err(malformed("is not closed by \"?>\"")) } else { Ok(TextDecl::NeedMore) };
    };
    let mut rest = &rem[HEAD.len()..end];
    let mut seen: Vec<&str> = Vec::new();
    while !rest.trim_start_matches(chars::is_whitespace).is_empty() {
      let (name, _value, tail) = self.pseudo_attribute(rest, "text declaration")?;
      // Dispatch on the name first, so a misplaced or repeated version/encoding is told apart from a name that is not a
      // pseudo-attribute at all.
      if seen.contains(&name) {
        return Err(malformed(&format!("has more than one {name}")));
      }
      match name {
        // `TextDecl ::= '<?xml' VersionInfo? EncodingDecl S? '?>'`, so version, when present, comes before encoding.
        "version" if seen.is_empty() => {}
        "version" => return Err(malformed("has version after encoding")),
        "encoding" => {}
        "standalone" => return Err(malformed("may not have a standalone declaration")),
        other => return Err(malformed(&format!("has {other:?}, which is not one of version or encoding"))),
      }
      seen.push(name);
      rest = tail;
    }
    if !seen.contains(&"encoding") {
      return Err(malformed("has no encoding"));
    }
    Ok(TextDecl::Present(end + 2))
  }

  fn comment(&mut self, text: &str) -> Result<EventKind> {
    debug_assert!(text.starts_with("<!--") && text.ends_with("-->"));
    let body = &text[4..text.len() - 3];
    if let Some(i) = body.find("--") {
      let message = "a comment may not contain \"--\"; use \"-\" or end the comment here";
      return Err(self.error_at(Error::well_formedness, message, text, 4 + i));
    }
    // `Comment ::= '<!--' ((Char - '-') | ('-' (Char - '-')))* '-->'`, so the body cannot end
    // with a dash either: `<!--a--->` is three dashes, not a comment closed by the last two.
    if body.ends_with('-') {
      let message = "a comment may not end with \"-\" before its \"-->\"";
      return Err(self.error_at(Error::well_formedness, message, text, text.len() - 4));
    }
    self.text.clear();
    self.text.push_str(body);
    Ok(EventKind::Comment)
  }

  fn doctype(&mut self, text: &str) -> Result<Option<EventKind>> {
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

    // Keep the DOCTYPE text for the event, and set up the DTD to be parsed by `drive_dtd`,
    // which runs across the entity fetches an external subset or parameter entity may need.
    self.text.clear();
    self.text.push_str(text);
    self.dtd_buf = internal_subset.to_owned();
    self.dtd_internal_len = self.dtd_buf.len();
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
    let base = self.token_at.clone();
    match dtd::parse_dtd(&mut self.dtd_buf, &mut self.dtd_internal_len, &mut self.pool, &base)? {
      dtd::DtdOutcome::Complete(dtd) => {
        self.dtd = Some(*dtd);
        self.dtd_active = false;
        self.dtd_buf = String::new();
        self.kind = Some(EventKind::Doctype);
        Ok(Progress::Event(EventKind::Doctype))
      }
      dtd::DtdOutcome::NeedExternalPe(pe) => {
        let base = self.stack.document().base_uri().map(ToString::to_string);
        self.pending_entity = Some(EntityRequest::new(
          Some(pe.name.clone()),
          pe.public_id.clone(),
          pe.system_id.clone(),
          base,
          RequestKind::ParameterEntity,
        ));
        self.dtd_pe = Some(pe);
        Ok(Progress::NeedEntity)
      }
    }
  }

  fn cdata(&mut self, text: &str) -> Result<EventKind> {
    debug_assert!(text.starts_with("<![CDATA[") && text.ends_with("]]>"));
    if self.phase != Phase::Content {
      return Err(self.error(Error::well_formedness, "a CDATA section may only appear inside the root element"));
    }
    self.text.clear();
    self.text.push_str(&text[9..text.len() - 3]);
    Ok(EventKind::CData)
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
  ///
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

  /// Flushes the pending text as a `Text` event once it reaches [`Bounds::text_fragment_len`], returning `None` while
  /// it is still shorter.
  ///
  /// The parser calls this after each text or reference fragment, so it emits a long run in pieces rather than buffer
  /// it without bound. This is why one run can span several `Text` events; [`EventKind::Text`] covers coalescing them.
  ///
  fn flush_if_full(&mut self) -> Result<Option<EventKind>> {
    if self.pending_text.len() >= self.bounds.text_fragment_len { self.flush_text() } else { Ok(None) }
  }

  /// Emits the pending text run as a `Text` event, or returns `None` when there is nothing to report.
  ///
  /// Inside the root element, this moves the run into the `text` field, from where [`EventRef::Text`] borrows it, and
  /// reports a `Text` event. The prolog and
  /// epilog allow only whitespace, so there this discards a whitespace-only run and returns `None` but rejects any
  /// other text, pointing the error at its first non-whitespace character. An empty run also returns `None`.
  ///
  fn flush_text(&mut self) -> Result<Option<EventKind>> {
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
    Ok(Some(EventKind::Text))
  }

  /// Handles a reference token (`&...;`) in content. A reference yields no event of its own, so this returns `None`.
  ///
  /// A character reference (`&#..;`) or a predefined entity (`&lt;`, `&gt;`, `&amp;`, `&apos;`, `&quot;`) resolves to a
  /// character that this appends to the current text run. For a general-entity reference, the parser begins reading the
  /// entity's replacement in place (see [`push_general_entity`](Self::push_general_entity)). The parser rejects a
  /// reference outside the root element.
  ///
  fn reference(&mut self, text: &str, at: &Location) -> Result<Option<EventKind>> {
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
  ///
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
  ///
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
  /// declared (WFC: Entity Declared). This is the content path; an attribute value expands entities inline instead
  /// (`expand_at`).
  ///
  fn push_general_entity(&mut self, name: NameId, at: &Location) -> Result<()> {
    self.token_at = at.clone();
    let display = self.pool.resolve(name).to_owned();
    // WFC: Entity Declared, standalone form. A standalone document may not reference an entity that only the external
    // subset declares.
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
      Some(GeneralEntity::External { public_id, system_id }) => {
        // Stop and request it. A driver resolves it and calls `provide_entity`, or `decline_entity`; `advance` sees
        // the request and returns `Progress::NeedEntity`.
        let base = self.stack.document().base_uri().map(ToString::to_string);
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

  fn start_tag(&mut self, text: &str) -> Result<EventKind> {
    debug_assert!(text.starts_with('<') && !text.starts_with("</") && text.ends_with('>'));
    match self.phase {
      Phase::Prolog => self.phase = Phase::Content,
      Phase::Content => {}
      Phase::Epilog => {
        return Err(self.error(Error::well_formedness, "a document may have only one root element"));
      }
    }
    if let Some(limit) = self.stack.limits().max_element_depth {
      if self.open.len() >= limit {
        let message = format!(
          "elements are nested more than {limit} deep; raise Limits::max_element_depth if the document is trusted"
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
    self.check_attribute_uniqueness()?;

    let (xml_space, xml_lang) = self.space_and_lang()?;
    #[cfg(feature = "xml-id")]
    self.normalize_xml_id();
    #[cfg(feature = "xml-base")]
    let base = self.element_base()?;
    let lexical = self.remember_name(text, 1, name_len);
    self.name = name;
    self.xml_space = xml_space;
    self.xml_lang = xml_lang;
    #[cfg(feature = "xml-base")]
    {
      self.base = base.clone();
    }
    let entity_depth = self.stack.depth();
    self.open.push(OpenElement {
      name,
      lexical,
      namespace_mark,
      xml_space,
      xml_lang,
      #[cfg(feature = "xml-base")]
      base,
      entity_depth,
    });
    self.end_pending = empty;
    Ok(EventKind::StartElement)
  }

  fn end_tag(&mut self, text: &str) -> Result<EventKind> {
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
    Ok(EventKind::EndElement)
  }

  /// Reports the state of the element being closed, then leaves its scope.
  fn close(&mut self, open: OpenElement) {
    self.name = open.name;
    self.xml_space = open.xml_space;
    self.xml_lang = open.xml_lang;
    // The end-tag event carries the closing element's own base, so a caller reading it here
    // sees the same base URI the start tag did; the parent's is restored on the next event.
    #[cfg(feature = "xml-base")]
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

      let name_len = rest[at..].find(|c: char| c == '=' || chars::is_whitespace(c)).unwrap_or(rest.len() - at);
      let name = &rest[at..at + name_len];
      let name_at = base + at;
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
      at += end + quote.len_utf8();

      let Some((prefix, local)) = chars::split_qname(name) else {
        return Err(self.error_at(Error::namespace, bad_qname(name, "attribute"), token, name_at));
      };
      let declares_namespace = prefix == Some("xmlns") || (prefix.is_none() && local == "xmlns");
      let start = values.len();
      self.expand_at(raw, values, true, token, raw_at)?;
      let name = QName::new(prefix.map(|p| self.pool.intern(p)), None, self.pool.intern(local));
      self.attributes.push(Attribute { name, value: start..values.len(), declares_namespace });
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
      // WFC: Standalone Document Declaration. A default from the external subset may not be
      // applied to a standalone document.
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
      } else {
        self.attribute_text.push_str(value);
      }
      let range = start..self.attribute_text.len();
      let lexical = self.pool.resolve(def.name).to_owned();
      let Some((prefix, local)) = chars::split_qname(&lexical) else {
        let message = format!("the DTD declares an attribute with the invalid name {lexical:?}");
        return Err(self.error(Error::well_formedness, message));
      };
      let declares_namespace = prefix == Some("xmlns") || (prefix.is_none() && local == "xmlns");
      let name = QName::new(prefix.map(|p| self.pool.intern(p)), None, self.pool.intern(local));
      self.attributes.push(Attribute { name, value: range, declares_namespace });
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
        let bad = if name == "xmlns" {
          Some("the prefix \"xmlns\" cannot be declared".to_owned())
        } else if value.is_empty() {
          Some(format!("prefix \"{name}\" cannot be bound to an empty namespace name"))
        } else if name == "xml" && value != XML_NS_URI {
          Some("the prefix \"xml\" may only be bound to its own namespace name".to_owned())
        } else if name != "xml" && value == XML_NS_URI {
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
    let Some((prefix, local)) = chars::split_qname(name) else {
      return Err(self.error(Error::namespace, bad_qname(name, "element")));
    };
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

  fn check_attribute_uniqueness(&self) -> Result<()> {
    for (i, attribute) in self.attributes.iter().enumerate() {
      if let Some(other) = self.attributes[..i].iter().find(|a| a.name.expanded == attribute.name.expanded) {
        let message = format!("attribute \"{}\" appears twice", other.name.to_lexical(&self.pool));
        return Err(self.error(Error::well_formedness, message));
      }
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
            let message = format!("xml:space must be \"default\" or \"preserve\", not {other:?}");
            return Err(self.error(Error::well_formedness, message));
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
  #[cfg(feature = "xml-base")]
  fn element_base(&mut self) -> Result<Option<UriReference>> {
    if !self.config.xml_base {
      return Ok(None);
    }
    let inherited = self.open.last().map_or_else(|| self.stack.base_uri().cloned(), |e| e.base.clone());
    for i in 0..self.attributes.len() {
      let attribute = &self.attributes[i];
      if attribute.name.namespace() != Some(NameId::XML_NS) || attribute.name.local() != self.base_name {
        continue;
      }
      // XML Base §3.1: characters disallowed in a URI are escaped before the value is used.
      let escaped = xenolith_core::uri::escape_uri(&self.attribute_text[attribute.value.clone()]);
      let reference = UriReference::parse(&escaped).map_err(|e| {
        self.error(Error::uri, format!("xml:base value {escaped:?} is not a valid URI reference: {}", e.message()))
      })?;
      return Ok(Some(inherited.map_or_else(|| reference.clone(), |base| base.resolve(&reference))));
    }
    Ok(inherited)
  }

  /// Normalizes the `xml:id` attribute of the current tag as a tokenized ID, so its reported
  /// value is the ID even when no DTD declared it (xml:id §4).
  #[cfg(feature = "xml-id")]
  fn normalize_xml_id(&mut self) {
    if !self.config.xml_id {
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
  /// or unparsed entity, is caught here where XML 1.0 §3.3.1 forbids it.
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
              "entities are nested more than {limit} deep ({path}); raise Limits::max_depth if the document is trusted"
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

  /// The current event as a borrowed [`EventRef`], or `None` when there is no current event.
  ///
  /// It carries the same data as the individual accessors, matched as one enum instead of read field by field. The
  /// borrows last only until the next [`advance`](Self::advance). Call it after `advance` reports [`Progress::Event`].
  #[must_use]
  pub fn event_ref(&self) -> Option<EventRef<'_>> {
    Some(match self.kind? {
      EventKind::XmlDeclaration => EventRef::XmlDeclaration {
        version: &self.version,
        encoding: self.declared_encoding.as_deref(),
        standalone: self.standalone,
      },
      EventKind::Doctype => EventRef::Doctype(&self.text),
      EventKind::StartElement => EventRef::StartElement {
        name: self.name,
        attributes: Attributes { attributes: &self.attributes, text: &self.attribute_text },
        xml_space: self.xml_space,
        xml_lang: self.xml_lang.map(|l| self.pool.resolve(l)),
      },
      EventKind::EndElement => EventRef::EndElement { name: self.name },
      EventKind::Text => EventRef::Text(&self.text),
      EventKind::CData => EventRef::CData(&self.text),
      EventKind::Comment => EventRef::Comment(&self.text),
      EventKind::ProcessingInstruction => {
        EventRef::ProcessingInstruction { target: self.local_name(), data: &self.text, data_location: &self.pi_data_at }
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
  /// when [`ParserConfig::xml_base`] is off.
  ///
  /// Available only with the `xml-base` feature.
  ///
  #[cfg(feature = "xml-base")]
  #[must_use]
  pub fn base_uri(&self) -> Option<String> {
    self.base.as_ref().map(ToString::to_string)
  }

  /// The normalized `xml:id` of the current start element, if it carried one (xml:id).
  ///
  /// Tokenized normalization has been applied, so the value is already trimmed and collapsed. Whether it is a valid
  /// `NCName` and unique in the document is checked by the validation layer, which reuses the ID machinery for it.
  ///
  /// Available only with the `xml-id` feature, and `None` when [`ParserConfig::xml_id`] is off.
  ///
  #[cfg(feature = "xml-id")]
  #[must_use]
  pub fn xml_id(&self) -> Option<&str> {
    if !self.config.xml_id {
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
  /// ([`EventRef::XmlDeclaration`]'s `encoding`): a byte-order mark or a pinned encoding overrides the declaration, and
  /// the declaration's value is taken verbatim, so the declared `"utf-8"` decodes as the canonical `"UTF-8"`. Here the
  /// caller pins UTF-8, so the declaration's `UTF-16` is only what was named, not what decodes the document:
  ///
  /// ```
  /// # use xenolith_parser::{CharStream, Entity, EventKind, EventRef, Limits, Parser, Progress};
  /// let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
  /// let mut parser = Parser::with_document(document, Limits::default());
  /// parser.feed("<?xml version='1.0' encoding='UTF-16'?><a/>".as_bytes(), true).unwrap();
  /// while !matches!(parser.advance().unwrap(), Progress::Event(EventKind::XmlDeclaration)) {}
  /// let Some(EventRef::XmlDeclaration { encoding, .. }) = parser.event_ref() else { unreachable!() };
  /// assert_eq!(encoding, Some("UTF-16")); // what the declaration named
  /// assert_eq!(parser.encoding(), Some("UTF-8")); // what actually decodes the bytes
  /// ```
  #[must_use]
  pub fn encoding(&self) -> Option<&str> {
    self.stack.document().stream().encoding()
  }

  /// How deeply elements are nested; 0 represents outside the root element.
  ///
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len()
  }

  /// The parser's current position, which sits at the end of the event just reported.
  ///
  /// For where the current event *begins*, use [`event_location`](Self::event_location).
  ///
  #[must_use]
  pub fn location(&self) -> Location {
    self.stack.location()
  }

  /// The source position where the current event begins.
  ///
  /// This is the start of the event's markup, the natural "location" of the event as an object. It differs from
  /// [`location`](Self::location), which is the parser's current position, at the end of the event just read.
  ///
  #[must_use]
  pub fn event_location(&self) -> Location {
    self.token_at.clone()
  }

  /// The pool holding every name the parser has seen.
  ///
  #[must_use]
  pub const fn pool(&self) -> &NamePool {
    &self.pool
  }

  /// The document type definition, once a `DOCTYPE` has been read; `None` before that, or if the document has no
  /// `DOCTYPE`.
  ///
  /// This is what a validator reads: the declared elements, attributes, entities and notations. It becomes available
  /// with the [`Doctype`](EventKind::Doctype) event.
  ///
  #[must_use]
  pub const fn dtd(&self) -> Option<&Dtd> {
    self.dtd.as_ref()
  }

  /// The root element name the `DOCTYPE` declared, interned in [`pool`](Self::pool), or `None` with no `DOCTYPE`.
  ///
  /// It is the name right after `<!DOCTYPE`: for `<!DOCTYPE greeting SYSTEM "greeting.dtd">` this is the interned
  /// `greeting`, so [`pool`](Self::pool)`.resolve(id)` gives `"greeting"`. A valid document's root element must carry
  /// this name, which a validator checks.
  ///
  #[must_use]
  pub const fn doctype_name(&self) -> Option<NameId> {
    self.doctype_name
  }

  /// The public identifier of the `DOCTYPE`'s external subset, if it declared one with `PUBLIC`.
  ///
  /// For `<!DOCTYPE greeting PUBLIC "-//Example//DTD Greeting//EN" "greeting.dtd">` this is
  /// `Some("-//Example//DTD Greeting//EN")`; the `SYSTEM` form and no external subset both give `None`.
  ///
  #[must_use]
  pub fn doctype_public_id(&self) -> Option<&str> {
    self.doctype_public_id.as_deref()
  }

  /// The system identifier of the `DOCTYPE`'s external subset, if it declared one.
  ///
  /// It is the second literal of a `PUBLIC` identifier or the only one of a `SYSTEM` identifier: both
  /// `<!DOCTYPE greeting SYSTEM "greeting.dtd">` and `<!DOCTYPE greeting PUBLIC "-//Example//DTD Greeting//EN"
  /// "greeting.dtd">` give `Some("greeting.dtd")`; `None` with no external subset.
  ///
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
  ///
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
  ///
  pub fn provide_entity(&mut self, bytes: &[u8]) -> Result<()> {
    let Some(request) = self.pending_entity.take() else {
      return Err(Error::internal("provide_entity called while the parser is not waiting for an entity"));
    };
    let system_id = request.resolved_uri();
    let mut stream = CharStream::new();
    if let Some(id) = system_id {
      stream = stream.with_system_id(id);
    }
    stream.feed(bytes, true)?;
    self.strip_text_declaration(&mut stream)?;

    match request.kind() {
      RequestKind::GeneralEntity => {
        let name = request.name().map(Into::into);
        self.stack.push(Entity::new(name, EntityKind::ExternalGeneral, stream, None))
      }
      // The external subset is DTD text: append it after the internal subset and resume.
      RequestKind::ExternalSubset => {
        if !self.dtd_buf.is_empty() {
          self.dtd_buf.push('\n');
        }
        self.dtd_buf.push_str(stream.remainder());
        Ok(())
      }
      // An external parameter entity's content replaces the `%name;` that summoned it.
      RequestKind::ParameterEntity => {
        let pe = self.dtd_pe.take().expect("a parameter entity was pending");
        let replacement = format!(" {} ", stream.remainder());
        if pe.at < self.dtd_internal_len {
          let removed = pe.end.min(self.dtd_internal_len) - pe.at;
          self.dtd_internal_len = self.dtd_internal_len - removed + replacement.len();
        }
        self.dtd_buf.replace_range(pe.at..pe.end, &replacement);
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
  /// [`Error::WellFormedness`] for a general or parameter entity that cannot be resolved. Returns [`Error::Internal`] if
  /// the parser is not waiting for an entity.
  ///
  pub fn decline_entity(&mut self) -> Result<()> {
    let Some(request) = self.pending_entity.take() else {
      return Err(Error::internal("decline_entity called while the parser is not waiting for an entity"));
    };
    if request.kind() == RequestKind::ExternalSubset {
      self.external_subset_unread = true;
      self.dtd_pe = None;
      return Ok(());
    }
    let what = request.name().map_or_else(|| "an external entity".to_owned(), |name| format!("entity \"{name}\""));
    Err(self.error(Error::well_formedness, format!("{what} could not be resolved")))
  }

  /// Iterates the remaining events as owned [`Event`] values, each yielded as a `Result`.
  ///
  /// The iterator cannot drive I/O, so the whole document must be in hand and self-contained: every byte fed, and no
  /// external entity to resolve. If the parser would request either, the iterator yields an error instead, since
  /// [`Progress::NeedMoreInput`] and [`Progress::NeedEntity`] have nowhere to go here. Use [`advance`](Self::advance)
  /// directly, or a [`Reader`](crate::Reader) with a resolver, when input arrives in pieces or the document pulls in
  /// external entities. Iteration stops at [`Eof`](Progress::Eof) and after the first error.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_parser::{Event, Parser};
  ///
  /// let mut parser = Parser::new();
  /// parser.feed(b"<a>hi</a>", true)?;
  ///
  /// let names: Vec<String> = parser
  ///   .events()
  ///   .filter_map(|event| event.ok()?.text().map(ToOwned::to_owned))
  ///   .collect();
  /// assert_eq!(names, ["hi"]);
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  pub fn events(&mut self) -> Events<'_> {
    Events { parser: self, done: false }
  }

  /// Builds the namespace error for a prefix with no binding in scope, naming the fix (add an `xmlns:` declaration).
  ///
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
  ///
  fn error(&self, build: fn(String) -> Error, message: impl Into<String>) -> Error {
    build(message.into()).at(self.token_at.clone())
  }

  /// Builds an error located `index` bytes into `token`, pointing to a specific character rather than the token start.
  ///
  /// It advances the token's start location over `token[..index]`, so `index` is a byte offset within `token`, at a
  /// character boundary. `build` and `message` are as in [`error`](Self::error).
  ///
  fn error_at(&self, build: fn(String) -> Error, message: impl Into<String>, token: &str, index: usize) -> Error {
    let mut at = self.token_at.clone();
    for c in token[..index.min(token.len())].chars() {
      at.advance(c);
    }
    build(message.into()).at(at)
  }
}

/// Iterator over the remaining events of a [`Parser`]; see [`Parser::events`].
#[derive(Debug)]
pub struct Events<'a> {
  parser: &'a mut Parser,
  done: bool,
}

impl Iterator for Events<'_> {
  type Item = Result<Event>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.done {
      return None;
    }
    match self.parser.advance() {
      Ok(Progress::Event(_)) => Some(Event::capture(self.parser)),
      Ok(Progress::Eof) => {
        self.done = true;
        None
      }
      Ok(Progress::NeedMoreInput) => {
        self.done = true;
        let message = "the document is incomplete; feed the remaining bytes before iterating, \
                       or drive the parser with advance() so it can request more";
        Some(Err(Error::Internal { message: message.into() }.at(self.parser.location())))
      }
      Ok(Progress::NeedEntity) => {
        self.done = true;
        let message = "the document references an external entity; iterate over a Reader with a \
                       resolver, or drive the parser with advance() so the entity can be provided";
        Some(Err(Error::Internal { message: message.into() }.at(self.parser.location())))
      }
      Err(e) => {
        self.done = true;
        Some(Err(e))
      }
    }
  }
}

/// Returns the byte length of the whitespace run at the start of `text`, or 0 when `text` does not start with
/// whitespace. Callers use it to require an `S` (mandatory whitespace) between parts of a construct.
///
fn whitespace_len(text: &str) -> usize {
  text.len() - text.trim_start_matches(chars::is_whitespace).len()
}

/// Parses an `ExternalID` into its public and system identifiers, and the text left after them.
///
/// `ExternalID ::= 'SYSTEM' S SystemLiteral | 'PUBLIC' S PubidLiteral S SystemLiteral`. On success, this returns the
/// public identifier (`None` for the `SYSTEM` form), the always-present system identifier, and the remainder after the
/// last literal, which the caller checks is only whitespace. It returns `None` when `text` is not shaped like an
/// `ExternalID`.
///
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

/// Explains why `name` is not a usable name; `role` (for example, `"element"` or `"attribute"`) names the kind in the
/// message.
///
/// A bare "not a valid name" leaves the author hunting, so this points to the offending character or the extra colon,
/// which usually helps find the typo.
///
fn bad_qname(name: &str, role: &str) -> String {
  if name.is_empty() {
    return format!("this {role} has no name");
  }
  if name.matches(':').count() > 1 {
    return format!("{role} name {name:?} has more than one colon; only one separates a prefix from a local name");
  }
  if name.starts_with(':') || name.ends_with(':') {
    return format!("{role} name {name:?} has an empty prefix or local name");
  }
  match name.chars().find(|c| !chars::is_name_char(*c)) {
    Some(c) => format!("{role} name {name:?} contains {c:?}, which names may not"),
    // Every character is allowed somewhere in a name, so only the first can be at fault.
    None => format!("{role} name {name:?} starts with a character that may not begin a name"),
  }
}

/// Normalizes one literal fragment of an attribute value per XML 1.0 §3.3.3: a tab or newline becomes a space.
///
/// The stream has already folded CR and CRLF to LF (end-of-line handling), and a space needs no change, so only tabs
/// and newlines remain. This folds literal whitespace only; whitespace written as a character reference (`&#9;`) never
/// reaches here, because the caller resolves references separately, which is what keeps it a tab. With `attribute`
/// false, this returns `text` untouched, since text content keeps its whitespace.
///
fn normalize(text: &str, attribute: bool) -> Cow<'_, str> {
  if !attribute || !text.contains(['\t', '\n']) {
    return Cow::Borrowed(text);
  }
  // A literal tab or newline in an attribute value is uncommon, so this path is rarely taken; the second scan that
  // `replace` makes over the string is not worth avoiding, while the common case above returns borrowed after one scan.
  Cow::Owned(text.replace(['\t', '\n'], " "))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Renders one event compactly, so tests can assert on a whole document at once.
  fn render(parser: &Parser, event: EventRef) -> String {
    match event {
      EventRef::XmlDeclaration { version, encoding, standalone } => {
        let mut s = format!("?xml {version}");
        if let Some(encoding) = encoding {
          s.push_str(&format!(" {encoding}"));
        }
        if let Some(standalone) = standalone {
          s.push_str(if standalone { " standalone" } else { " not-standalone" });
        }
        s
      }
      EventRef::Doctype(text) => format!("!doctype {text}"),
      EventRef::StartElement { name, attributes, .. } => {
        let mut s = format!("<{}", qualified(parser, name));
        for attribute in attributes.iter() {
          s.push_str(&format!(" {}={}", qualified(parser, attribute.name), attribute.value));
        }
        s.push('>');
        s
      }
      EventRef::EndElement { name } => format!("</{}>", qualified(parser, name)),
      EventRef::Text(text) => format!("t:{text}"),
      EventRef::CData(text) => format!("c:{text}"),
      EventRef::Comment(text) => format!("!:{text}"),
      EventRef::ProcessingInstruction { target, data, .. } => format!("?{target} {data}"),
    }
  }

  /// The character data of the current text, CDATA, or comment event, for the tests that collect a run by hand.
  fn text_of(parser: &Parser) -> &str {
    parser.event_ref().and_then(|e| e.text()).expect("the current event is character data")
  }

  /// `{namespace}local`, so namespace resolution is visible in the trace.
  fn qualified(parser: &Parser, name: QName) -> String {
    match name.namespace() {
      Some(ns) => format!("{{{}}}{}", parser.pool().resolve(ns), parser.pool().resolve(name.local())),
      None => parser.pool().resolve(name.local()).to_owned(),
    }
  }

  /// Parses `xml` fed in chunks of `chunk` bytes, returning the rendered events.
  fn trace_in_chunks(xml: &str, chunk: usize) -> Result<Vec<String>> {
    let mut parser = Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8")?), Limits::default());
    let bytes = xml.as_bytes();
    let mut fed = 0;
    let mut events = Vec::new();
    loop {
      match parser.advance()? {
        Progress::Event(_) => events.push(render(&parser, parser.event_ref().expect("a current event"))),
        Progress::Eof => return Ok(events),
        Progress::NeedMoreInput => {
          assert!(fed <= bytes.len(), "requested input after everything was fed");
          let end = (fed + chunk).min(bytes.len());
          parser.feed(&bytes[fed..end], end == bytes.len())?;
          fed = end;
        }
        // These unit tests use no external entities.
        Progress::NeedEntity => parser.decline_entity()?,
      }
    }
  }

  fn trace(xml: &str) -> Result<Vec<String>> {
    let all = trace_in_chunks(xml, xml.len().max(1))?;
    // Every document must parse identically however the input is split; this is the property
    // the resumable design exists for, so it is checked on every case rather than once.
    for chunk in [1, 2, 3, 7] {
      let split = trace_in_chunks(xml, chunk).unwrap_or_else(|e| panic!("failed at chunk size {chunk}: {e}"));
      assert_eq!(split, all, "chunk size {chunk} changed the result");
    }
    Ok(all)
  }

  fn error(xml: &str) -> Error {
    let whole = trace_in_chunks(xml, xml.len().max(1)).expect_err("should fail");
    for chunk in [1, 2, 3, 7] {
      let split = trace_in_chunks(xml, chunk).expect_err("should fail whatever the chunk size");
      assert_eq!(
        std::mem::discriminant(&split),
        std::mem::discriminant(&whole),
        "chunk size {chunk} changed the error kind"
      );
    }
    whole
  }

  /// Drives the parser over `xml`, streaming any general entity named in `entities` in
  /// `chunk`-byte pieces through `begin_entity` + `feed`, exactly as the streaming driver will.
  /// A stack of byte sources mirrors the parser's entity stack: the innermost source feeds the
  /// innermost entity.
  fn trace_streaming_in_chunks(xml: &str, entities: &[(&str, &str)], chunk: usize) -> Result<Vec<String>> {
    let mut parser = Parser::with_document(Entity::document(CharStream::new()), Limits::default());
    let mut sources: Vec<(Vec<u8>, usize)> = vec![(xml.as_bytes().to_vec(), 0)];
    let mut events = Vec::new();
    loop {
      match parser.advance()? {
        Progress::Event(_) => events.push(render(&parser, parser.event_ref().expect("a current event"))),
        Progress::Eof => return Ok(events),
        Progress::NeedMoreInput => {
          let (bytes, at) = sources.last_mut().expect("a source for the innermost entity");
          let end = at.saturating_add(chunk).min(bytes.len());
          let last = end == bytes.len();
          parser.feed(&bytes[*at..end], last)?;
          *at = end;
          if last {
            sources.pop();
          }
        }
        Progress::NeedEntity => {
          let name = parser.pending_entity().and_then(|r| r.name()).expect("a named general entity").to_owned();
          match entities.iter().find(|(n, _)| *n == name) {
            Some((_, content)) => {
              parser.begin_entity()?;
              sources.push((content.as_bytes().to_vec(), 0));
            }
            None => parser.decline_entity()?,
          }
        }
      }
    }
  }

  /// Streams the entities and asserts the result does not depend on how the bytes are split.
  fn trace_streaming(xml: &str, entities: &[(&str, &str)]) -> Result<Vec<String>> {
    let whole = trace_streaming_in_chunks(xml, entities, usize::MAX)?;
    for chunk in [1, 2, 3, 7] {
      let split =
        trace_streaming_in_chunks(xml, entities, chunk).unwrap_or_else(|e| panic!("failed at chunk size {chunk}: {e}"));
      assert_eq!(split, whole, "chunk size {chunk} changed the result");
    }
    Ok(whole)
  }

  #[test]
  fn a_streamed_external_entity_is_read_in_chunks() {
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let events = trace_streaming(xml, &[("e", "<b>in</b>")]).unwrap();
    assert_eq!(events, ["!doctype <!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]>", "<a>", "<b>", "t:in", "</b>", "</a>"]);
  }

  #[test]
  fn a_streamed_entity_text_declaration_is_stripped_across_feeds() {
    // The text declaration can straddle any feed boundary; it must be stepped over, not surface
    // as a processing instruction, whatever the chunk size.
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let entity = "<?xml version='1.0' encoding='UTF-8'?><b>in</b>";
    let events = trace_streaming(xml, &[("e", entity)]).unwrap();
    assert_eq!(events, ["!doctype <!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]>", "<a>", "<b>", "t:in", "</b>", "</a>"]);
  }

  #[test]
  fn a_malformed_text_declaration_names_what_is_wrong() {
    let xml = "<!DOCTYPE a [<!ENTITY e SYSTEM 'e.ent'>]><a>&e;</a>";
    let bad = |decl: &str| trace_streaming(xml, &[("e", &format!("{decl}<b/>"))]).unwrap_err().message().to_owned();
    // A name that is not a pseudo-attribute is reported as such, not as a misordering.
    assert!(bad("<?xml version='1.0' bogus='x'?>").contains("not one of version or encoding"));
    // A known pseudo-attribute misplaced or repeated is reported specifically.
    assert!(bad("<?xml encoding='UTF-8' version='1.0'?>").contains("version after encoding"));
    assert!(bad("<?xml encoding='UTF-8' encoding='UTF-8'?>").contains("more than one encoding"));
    assert!(bad("<?xml version='1.0' encoding='UTF-8' standalone='yes'?>").contains("standalone"));
    // The pseudo-attribute parser's own reason survives, named for the text declaration, not overwritten as "malformed".
    assert_eq!(bad("<?xml version=1.0?>"), "the text declaration has an unquoted value");
  }

  #[test]
  fn reports_the_events_of_a_small_document() {
    assert_eq!(
      trace("<?xml version='1.0' encoding='UTF-8'?>\n<!--hi--><a x='1'>text<b/></a>\n").unwrap(),
      ["?xml 1.0 UTF-8", "!:hi", "<a x=1>", "t:text", "<b>", "</b>", "</a>"]
    );
  }

  #[test]
  fn an_empty_element_reports_a_start_and_an_end() {
    assert_eq!(trace("<a/>").unwrap(), ["<a>", "</a>"]);
    assert_eq!(trace("<a></a>").unwrap(), ["<a>", "</a>"]);
    assert_eq!(trace("<a>  </a>").unwrap(), ["<a>", "t:  ", "</a>"]);
  }

  #[test]
  fn whitespace_outside_the_root_is_dropped_but_markup_is_not() {
    assert_eq!(trace("  <a/>\n\n<!--after-->\n<?pi data?>  ").unwrap(), ["<a>", "</a>", "!:after", "?pi data"]);
  }

  #[test]
  fn resolves_namespaces() {
    let events = trace("<a xmlns='urn:d' xmlns:p='urn:p'><p:b q='1' p:r='2'/></a>").unwrap();
    assert_eq!(
      events,
      [
        "<{urn:d}a {http://www.w3.org/2000/xmlns/}xmlns=urn:d {http://www.w3.org/2000/xmlns/}p=urn:p>",
        "<{urn:p}b q=1 {urn:p}r=2>",
        "</{urn:p}b>",
        "</{urn:d}a>",
      ]
    );
  }

  #[test]
  fn an_unprefixed_attribute_is_in_no_namespace() {
    let events = trace("<a xmlns='urn:d' x='1'/>").unwrap();
    assert!(events[0].contains(" x=1"), "{events:?}");
    assert!(events[0].starts_with("<{urn:d}a"), "{events:?}");
  }

  #[test]
  fn the_default_namespace_can_be_undeclared() {
    let events = trace("<a xmlns='urn:d'><b xmlns=''/></a>").unwrap();
    assert!(events[1].starts_with("<b "), "{events:?}");
  }

  #[test]
  fn a_namespace_declaration_leaves_scope_with_its_element() {
    assert!(matches!(error("<a><b xmlns:p='urn:p'/><p:c/></a>"), Error::Namespace { .. }));
  }

  #[test]
  fn xml_is_always_bound() {
    let events = trace("<a xml:lang='en'/>").unwrap();
    assert!(events[0].contains("{http://www.w3.org/XML/1998/namespace}lang=en"), "{events:?}");
  }

  #[test]
  fn expands_character_and_predefined_references() {
    assert_eq!(trace("<a>&lt;&amp;&gt;&#65;&#x42;&apos;&quot;</a>").unwrap()[1], "t:<&>AB'\"");
    assert_eq!(trace("<a b='&lt;&#65;'/>").unwrap()[0], "<a b=<A>");
  }

  #[test]
  fn normalizes_attribute_values() {
    // Literal whitespace becomes a space; whitespace written as a reference does not.
    assert_eq!(trace("<a b='x\ty\nz'/>").unwrap()[0], "<a b=x y z>");
    assert_eq!(trace("<a b='x&#9;y'/>").unwrap()[0], "<a b=x\ty>");
  }

  #[test]
  fn cdata_is_reported_separately_and_is_not_expanded() {
    assert_eq!(trace("<a><![CDATA[<&]]>tail</a>").unwrap(), ["<a>", "c:<&", "t:tail", "</a>"]);
  }

  #[test]
  fn processing_instructions_keep_their_data_verbatim() {
    assert_eq!(trace("<a><?target a='1' &b;?></a>").unwrap()[1], "?target a='1' &b;");
    assert_eq!(trace("<a><?bare?></a>").unwrap()[1], "?bare ");
  }

  #[test]
  fn tracks_xml_space_and_lang_through_the_tree() {
    let mut parser = Parser::new();
    parser.feed(b"<a xml:space='preserve' xml:lang='ja'><b xml:space='default'><c/></b></a>", true).unwrap();

    let mut seen = Vec::new();
    while let Progress::Event(kind) = parser.advance().unwrap() {
      if kind == EventKind::StartElement {
        seen.push((parser.local_name().to_owned(), parser.xml_space(), parser.xml_lang().map(str::to_owned)));
      }
    }
    assert_eq!(
      seen,
      [
        ("a".to_owned(), XmlSpace::Preserve, Some("ja".to_owned())),
        ("b".to_owned(), XmlSpace::Default, Some("ja".to_owned())),
        ("c".to_owned(), XmlSpace::Default, Some("ja".to_owned())),
      ]
    );
  }

  #[cfg(feature = "xml-base")]
  #[test]
  fn computes_base_uris_from_the_system_id_and_xml_base() {
    let doc = Entity::document(CharStream::with_encoding("UTF-8").unwrap().with_system_id("file:///a/b/doc.xml"));
    let mut parser = Parser::with_document(doc, Limits::default());
    parser.feed(b"<a><b xml:base='../c/'><d xml:base='e.xml'/></b><f/></a>", true).unwrap();

    let mut seen = Vec::new();
    while let Progress::Event(kind) = parser.advance().unwrap() {
      if kind == EventKind::StartElement {
        seen.push((parser.local_name().to_owned(), parser.base_uri()));
      }
    }
    assert_eq!(
      seen,
      [
        ("a".to_owned(), Some("file:///a/b/doc.xml".to_owned())),
        ("b".to_owned(), Some("file:///a/c/".to_owned())),
        ("d".to_owned(), Some("file:///a/c/e.xml".to_owned())),
        ("f".to_owned(), Some("file:///a/b/doc.xml".to_owned())),
      ]
    );
  }

  #[cfg(feature = "xml-base")]
  #[test]
  fn xml_base_can_be_turned_off() {
    let doc = Entity::document(CharStream::with_encoding("UTF-8").unwrap().with_system_id("file:///doc.xml"));
    let mut parser = Parser::with_document(doc, Limits::default());
    parser.set_config(ParserConfig::none());
    parser.feed(b"<a xml:base='sub/'/>", true).unwrap();
    parser.advance().unwrap();
    assert_eq!(parser.base_uri(), None);
  }

  #[cfg(feature = "xml-id")]
  #[test]
  fn normalizes_and_exposes_xml_id() {
    let mut parser = Parser::new();
    parser.feed(b"<a xml:id='  x1  '><b/></a>", true).unwrap();
    parser.advance().unwrap(); // <a>
    assert_eq!(parser.xml_id(), Some("x1"), "surrounding whitespace is collapsed away");
    assert_eq!(parser.attribute_value(Some(XML_NS_URI), "id"), Some("x1"), "the reported value is normalized too");
    parser.advance().unwrap(); // <b>
    assert_eq!(parser.xml_id(), None);
  }

  #[test]
  fn reports_depth() {
    let mut parser = Parser::new();
    parser.feed(b"<a><b/></a>", true).unwrap();
    let mut depths = Vec::new();
    while let Progress::Event(_) = parser.advance().unwrap() {
      depths.push(parser.depth());
    }
    assert_eq!(depths, [1, 2, 1, 0]);
  }

  #[test]
  fn keeps_the_doctype_for_later_phases() {
    let events = trace("<!DOCTYPE a [<!ENTITY e 'v'>]><a/>").unwrap();
    assert_eq!(events[0], "!doctype <!DOCTYPE a [<!ENTITY e 'v'>]>");
  }

  #[test]
  fn rejects_content_after_the_doctype_external_id() {
    let err = error("<!DOCTYPE r SYSTEM 'a.dtd' junk><r/>");
    assert!(err.message().contains("content after the external identifier"), "{}", err.message());
    // The error points at the stray content, not at the whole declaration.
    assert_eq!(err.location().column, 28);
  }

  #[test]
  fn each_event_clears_the_previous_events_accessors() {
    let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
    let mut parser = Parser::with_document(document, Limits::default());
    parser.feed("<a x='1'>hi</a>".as_bytes(), true).unwrap();
    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::StartElement));
    assert_eq!(parser.local_name(), "a");
    assert_eq!(parser.event_ref().unwrap().attributes().len(), 1);
    // The start tag's name must not linger into the following text event; its attributes cannot, since a `Text`
    // event is not the variant that carries them.
    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::Text));
    assert_eq!(text_of(&parser), "hi");
    assert!(parser.event_ref().unwrap().attributes().is_empty());
    assert_eq!(parser.local_name(), "", "the start tag's name leaked into the text event");
  }

  #[test]
  fn event_ref_gives_the_current_event_as_a_borrowed_enum() {
    let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
    let mut parser = Parser::with_document(document, Limits::default());
    parser.feed("<a x='1'>hi</a>".as_bytes(), true).unwrap();

    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::StartElement));
    let EventRef::StartElement { name, attributes, .. } = parser.event_ref().unwrap() else {
      panic!("expected a start element");
    };
    assert_eq!(parser.pool().resolve(name.local()), "a");
    assert_eq!(attributes.len(), 1);
    assert_eq!(attributes.get(0).unwrap().value, "1");

    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::Text));
    assert!(matches!(parser.event_ref(), Some(EventRef::Text("hi"))));

    // No current event before the first advance or after the last one.
    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::EndElement));
    assert_eq!(parser.advance().unwrap(), Progress::Eof);
    assert!(parser.event_ref().is_none());
  }

  #[test]
  fn a_too_deep_entity_chain_in_an_attribute_names_the_path() {
    let xml = "<!DOCTYPE a [<!ENTITY e0 'x'><!ENTITY e1 '&e0;'><!ENTITY e2 '&e1;'><!ENTITY e3 '&e2;'>]><a v='&e3;'/>";
    let document = Entity::document(CharStream::with_encoding("UTF-8").unwrap());
    let mut parser = Parser::with_document(document, Limits::default().with_max_depth(2));
    parser.feed(xml.as_bytes(), true).unwrap();
    let error = loop {
      match parser.advance() {
        Ok(Progress::Eof) => panic!("expected a depth-limit error"),
        Ok(Progress::NeedEntity) => parser.decline_entity().unwrap(),
        Ok(_) => {}
        Err(error) => break error,
      }
    };
    let message = error.message();
    assert!(message.contains("nested more than 2 deep"), "{message}");
    // The message traces the nesting path that tripped the limit.
    assert!(message.contains("e3 -> e2 -> e1"), "{message}");
  }

  #[test]
  fn a_cyclic_entity_in_an_attribute_names_the_loop() {
    // `a` and `b` refer to each other, so expanding either in an attribute loops.
    let xml = "<!DOCTYPE d [<!ENTITY a '&b;'><!ENTITY b '&a;'>]><d v='&a;'/>";
    let message = error(xml).message().to_owned();
    assert!(message.contains("refers to itself"), "{message}");
    assert!(message.contains("a -> b -> a"), "{message}");
  }

  #[test]
  fn rejects_mismatched_and_stray_end_tags() {
    assert!(error("<a></b>").message().contains("does not close"));
    assert!(error("<a/></a>").message().contains("never opened"));
    assert!(error("<a>").message().contains("not closed"));
    assert!(matches!(error("<a></a></a>"), Error::WellFormedness { .. }));
  }

  #[test]
  fn rejects_documents_without_exactly_one_root() {
    assert!(error("").message().contains("no root element"));
    assert!(error("<!--only a comment-->").message().contains("no root element"));
    assert!(error("<a/><b/>").message().contains("only one root"));
    assert!(error("text<a/>").message().contains("before the root"));
    assert!(error("<a/>text").message().contains("after the root"));
    // The error points at the first non-whitespace character, past the leading whitespace of the run.
    assert_eq!(error("  x<a/>").location().column, 3);
    assert_eq!(error("<a/>\n\ny").location().line, 3);
  }

  #[test]
  fn rejects_duplicate_attributes_only_when_the_names_are_the_same() {
    assert!(error("<a x='1' x='2'/>").message().contains("appears twice"));
    // Different prefixes bound to the same namespace still collide.
    assert!(error("<a xmlns:p='u' xmlns:q='u' p:x='1' q:x='2'/>").message().contains("appears twice"));
    // The same local name in different namespaces does not.
    assert_eq!(trace("<a xmlns:p='u' xmlns:q='v' p:x='1' q:x='2'/>").unwrap().len(), 2);
  }

  #[test]
  fn rejects_undeclared_prefixes() {
    assert!(matches!(error("<p:a/>"), Error::Namespace { .. }));
    assert!(matches!(error("<a p:x='1'/>"), Error::Namespace { .. }));
    assert!(matches!(error("<a xmlns:p=''/>"), Error::Namespace { .. }));
  }

  #[test]
  fn protects_the_reserved_prefixes() {
    assert!(matches!(error("<a xmlns:xmlns='urn:x'/>"), Error::Namespace { .. }));
    assert!(matches!(error("<a xmlns:xml='urn:x'/>"), Error::Namespace { .. }));
    assert!(matches!(error("<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>"), Error::Namespace { .. }));
    assert!(matches!(error("<a xmlns='http://www.w3.org/2000/xmlns/'/>"), Error::Namespace { .. }));
    // Rebinding xml to its own namespace name is allowed.
    assert_eq!(trace("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>").unwrap().len(), 2);
  }

  #[test]
  fn rejects_malformed_tags() {
    assert!(error("<a x/>").message().contains("no value"));
    assert!(error("<a x=1/>").message().contains("not quoted"));
    assert!(error("<a x='1'y='2'/>").message().contains("separated by whitespace"));
    assert!(matches!(error("<a:b:c/>"), Error::Namespace { .. }));
    assert!(matches!(error("<1a/>"), Error::Namespace { .. }));
  }

  #[test]
  fn rejects_bad_references() {
    assert!(error("<a>&nosuch;</a>").message().contains("not declared"));
    assert!(error("<a>&#xD800;</a>").message().contains("not a character"));
    assert!(error("<a>&#0;</a>").message().contains("not a character"));
    assert!(error("<a>&amp</a>").message().contains("must end with \";\""));
    assert!(error("<a b='&'/>").message().contains("must end with \";\""));
    assert!(error("<a b='<'/>").message().contains("\"<\" may not appear"));
  }

  #[test]
  fn rejects_forbidden_sequences_in_text_and_comments() {
    assert!(error("<a>]]></a>").message().contains("]]>"));
    assert!(error("<a><!-- a -- b --></a>").message().contains("--"));
  }

  #[test]
  fn rejects_a_misplaced_or_malformed_xml_declaration() {
    assert!(error("<a><?xml version='1.0'?></a>").message().contains("reserved"));
    assert!(error(" <?xml version='1.0'?><a/>").message().contains("reserved"));
    assert!(error("<?XML version='1.0'?><a/>").message().contains("reserved"));
    assert!(error("<?xml?><a/>").message().contains("no version"));
    assert!(error("<?xml version='2.0'?><a/>").message().contains("not an XML version"));
    assert!(error("<?xml version='1.0' standalone='maybe'?><a/>").message().contains("standalone"));
    // A misplaced version/encoding/standalone reports its position, not that the name is unknown.
    assert!(error("<?xml encoding='UTF-8' version='1.0'?><a/>").message().contains("encoding must come after version"));
    assert!(
      error("<?xml version='1.0' standalone='yes' encoding='UTF-8'?><a/>").message().contains("encoding must come")
    );
    assert!(
      error("<?xml standalone='yes' version='1.0'?><a/>").message().contains("standalone must come after version")
    );
    assert!(error("<?xml version='1.0' version='1.0'?><a/>").message().contains("more than one version"));
    assert!(
      error("<?xml version='1.0' encoding='UTF-8' encoding='UTF-8'?><a/>").message().contains("more than one encoding")
    );
    assert!(
      error("<?xml version='1.0' standalone='yes' standalone='no'?><a/>")
        .message()
        .contains("more than one standalone")
    );
    // A name that is not a pseudo-attribute says so, rather than blaming its position.
    assert!(error("<?xml version='1.0' encdng='UTF-8'?><a/>").message().contains("not a pseudo-attribute"));
  }

  #[test]
  fn reads_the_standalone_declaration() {
    assert_eq!(trace("<?xml version='1.0' standalone='yes'?><a/>").unwrap()[0], "?xml 1.0 standalone");
    assert_eq!(trace("<?xml version='1.1'?><a/>").unwrap()[0], "?xml 1.1");
  }

  #[test]
  fn rejects_an_invalid_xml_space() {
    assert!(error("<a xml:space='maybe'/>").message().contains("xml:space"));
  }

  /// Every message is read by someone deciding what to do next, so each one is checked for
  /// the remedy and not merely for the complaint. See the guidance in `xenolith_core::error`.
  #[test]
  fn messages_say_what_to_do_next() {
    let cases: [(&str, &str); 8] = [
      ("<a>&nosuch;</a>", "write \"&amp;nosuch;\""),
      ("<a>Tom & Jerry</a>", "a reference must end with \";\""),
      ("<a>]]></a>", "write \"]]&gt;\""),
      ("<a b='<'/>", "write \"&lt;\""),
      ("<p:a/>", "add an xmlns:p attribute"),
      ("<a checked/>", "checked=\"...\""),
      ("<a b=1/>", "enclose it in \" or '"),
      ("<a xml:space='maybe'/>", "\"default\" or \"preserve\""),
    ];
    for (xml, expected) in cases {
      let message = error(xml).message().to_owned();
      assert!(message.contains(expected), "parsing {xml:?} said {message:?},\n  which lacks {expected:?}");
    }
  }

  #[test]
  fn name_errors_name_the_offending_character() {
    assert!(error("<a b c='1'/>").message().contains("no value"));
    assert!(error("<a:b:c/>").message().contains("more than one colon"));
    assert!(error("<a b^c='1'/>").message().contains("'^'"));
    assert!(error("<1a/>").message().contains("may not begin a name"));
  }

  #[test]
  fn errors_point_at_the_offending_position() {
    let at = error("<a>\n  <b x='1' x='2'/>\n</a>").location().clone();
    assert_eq!(at.line, 2, "the duplicate is on the second line");

    let at = error("<a>\n  &nosuch;\n</a>").location().clone();
    assert_eq!((at.line, at.column), (2, 3));

    let at = error("<a>\n  <!-- a -- b -->\n</a>").location().clone();
    assert_eq!((at.line, at.column), (2, 10));
  }

  #[test]
  fn attributes_can_be_looked_up_by_expanded_name() {
    let mut parser = Parser::new();
    parser.feed(b"<a xmlns:p='urn:p' x='1' p:y='2'/>", true).unwrap();
    parser.advance().unwrap();
    assert_eq!(parser.attribute_value(None, "x"), Some("1"));
    assert_eq!(parser.attribute_value(Some("urn:p"), "y"), Some("2"));
    assert_eq!(parser.attribute_value(None, "y"), None, "the prefix is not ignored");
    assert_eq!(parser.attribute_value(Some("urn:none"), "x"), None);
    assert_eq!(parser.event_ref().unwrap().attributes().len(), 3);
  }

  #[test]
  fn text_split_across_chunks_is_still_one_event_per_run() {
    // The scanner may cut text short, so a run can arrive as several events; the
    // concatenation is what must be stable.
    let mut parser =
      Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8").unwrap()), Limits::default());
    let xml = b"<a>one &amp; two</a>";
    let mut text = String::new();
    let mut fed = 0;
    loop {
      match parser.advance().unwrap() {
        Progress::Event(EventKind::Text) => text.push_str(text_of(&parser)),
        Progress::Eof => break,
        Progress::NeedMoreInput => {
          let end = (fed + 1).min(xml.len());
          parser.feed(&xml[fed..end], end == xml.len()).unwrap();
          fed = end;
        }
        _ => {}
      }
    }
    assert_eq!(text, "one & two");
  }

  #[test]
  fn a_long_text_run_is_delivered_in_bounded_fragments() {
    // Fed in pieces, a run longer than the fragmentation threshold must come out as more than one Text
    // event, so neither the stream nor the parser buffers the whole run. The pieces still concatenate
    // to the original.
    let mut parser =
      Parser::with_document(Entity::document(CharStream::with_encoding("UTF-8").unwrap()), Limits::default());
    let body = "x".repeat(30_000);
    let xml = format!("<a>{body}</a>");
    let bytes = xml.as_bytes();
    let mut text = String::new();
    let mut text_events = 0;
    let mut fed = 0;
    loop {
      match parser.advance().unwrap() {
        Progress::Event(EventKind::Text) => {
          text.push_str(text_of(&parser));
          text_events += 1;
        }
        Progress::Eof => break,
        Progress::NeedMoreInput => {
          let end = (fed + 1000).min(bytes.len());
          parser.feed(&bytes[fed..end], end == bytes.len()).unwrap();
          fed = end;
        }
        _ => {}
      }
    }
    assert_eq!(text, body);
    assert!(text_events > 1, "a long run must be split into fragments, got {text_events}");
  }

  #[test]
  fn a_document_can_be_parsed_from_a_reader_that_stalls() {
    // NeedMoreInput must be answerable with nothing at all without losing state.
    let mut parser = Parser::new();
    assert_eq!(parser.advance().unwrap(), Progress::NeedMoreInput);
    parser.feed(b"", false).unwrap();
    assert_eq!(parser.advance().unwrap(), Progress::NeedMoreInput);
    parser.feed(b"<a/>", true).unwrap();
    assert_eq!(parser.advance().unwrap(), Progress::Event(EventKind::StartElement));
  }

  #[test]
  fn non_utf8_documents_are_decoded_before_parsing() {
    let mut parser = Parser::new();
    let mut bytes = b"<?xml version='1.0' encoding='ISO-8859-1'?><a>".to_vec();
    bytes.push(0xE9); // e-acute in Latin-1
    bytes.extend_from_slice(b"</a>");
    parser.feed(&bytes, true).unwrap();

    let mut text = None;
    while let Progress::Event(kind) = parser.advance().unwrap() {
      if kind == EventKind::Text {
        text = Some(text_of(&parser).to_owned());
      }
    }
    assert_eq!(text.as_deref(), Some("é"));
  }
}
