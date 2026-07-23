//! The sans-I/O parser core.
//!
//! [`Parser`] is fed bytes and asked to make progress. It reads no files and opens no
//! sockets, so the same core serves a blocking reader, an async reader and an in-memory
//! slice, and can stop between two tokens without holding a thread.
//!
//! Values are reached through accessors that borrow from the parser rather than through
//! events that own their data, so nothing is allocated per event once the buffers have grown.

use std::borrow::Cow;
use std::ops::Range;

use xylograph_core::chars;
use xylograph_core::error::{Error, ErrorKind, Location, Result};
use xylograph_core::name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};

use crate::entity::{Entity, EntityStack, Limits};
use crate::event::Event;
use crate::namespace::NamespaceScope;
use crate::scan::{Token, scan};
use crate::stream::CharStream;

/// What a call to [`Parser::advance`] achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Progress {
  /// An event is available; read it through the parser's accessors.
  Event(EventKind),
  /// More bytes are needed before anything can be decided.
  NeedMoreInput,
  /// The document is finished.
  Eof,
}

/// The kind of event the parser is reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventKind {
  /// The XML declaration; see [`Parser::version`] and its neighbours.
  XmlDeclaration,
  /// A document type declaration. Its content is not interpreted until phase 2.
  Doctype,
  /// The start of an element, or a whole empty element.
  StartElement,
  /// The end of an element, including the implied end of an empty element.
  EndElement,
  /// Character data.
  Text,
  /// The content of a CDATA section, reported separately from text because the DOM and the
  /// serializer both need to know where the section boundaries were.
  CData,
  /// A comment, without its delimiters.
  Comment,
  /// A processing instruction; see [`Parser::target`].
  ProcessingInstruction,
}

/// The value of `xml:space` in effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XmlSpace {
  /// No `xml:space` is in scope, or the nearest one says `default`.
  #[default]
  Default,
  /// `xml:space="preserve"` is in scope.
  Preserve,
}

/// One attribute of the current start tag.
#[derive(Clone, Copy, Debug)]
pub struct AttributeRef<'a> {
  /// The attribute's name. An unprefixed attribute is in no namespace, never the default one.
  pub name: QName,
  /// The normalized value.
  pub value: &'a str,
  /// True if this attribute is a namespace declaration (`xmlns` or `xmlns:p`).
  pub declares_namespace: bool,
}

#[derive(Clone, Debug)]
struct Attribute {
  name: QName,
  value: Range<usize>,
  declares_namespace: bool,
}

#[derive(Debug)]
struct OpenElement {
  name: QName,
  lexical: Range<usize>,
  namespace_mark: usize,
  xml_space: XmlSpace,
  xml_lang: Option<NameId>,
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
/// # Examples
///
/// ```
/// use xylograph_parser::{EventKind, Parser, Progress};
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
/// # Ok::<(), xylograph_core::Error>(())
/// ```
///
/// Values are read through accessors while the event is current:
///
/// ```
/// use xylograph_parser::{EventKind, Parser, Progress};
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
/// # Ok::<(), xylograph_core::Error>(())
/// ```
#[derive(Debug)]
pub struct Parser {
  stack: EntityStack,
  pool: NamePool,
  space_name: NameId,
  lang_name: NameId,
  scope: NamespaceScope,
  open: Vec<OpenElement>,
  phase: Phase,
  seen_doctype: bool,
  /// The end of an empty element, owed to the caller on the next call.
  end_pending: bool,
  kind: Option<EventKind>,
  /// Scratch holding the token being interpreted; reused between tokens.
  token: String,
  token_at: Location,
  /// Lexical names of the open elements, so an end tag can be compared with its start tag.
  names: String,
  text: String,
  name: QName,
  attributes: Vec<Attribute>,
  attribute_text: String,
  version: String,
  declared_encoding: Option<String>,
  standalone: Option<bool>,
  xml_space: XmlSpace,
  xml_lang: Option<NameId>,
}

impl Default for Parser {
  fn default() -> Self {
    Self::new()
  }
}

impl Parser {
  /// Creates a parser for a document whose encoding is determined from its bytes.
  #[must_use]
  pub fn new() -> Self {
    Self::with_document(Entity::document(CharStream::new()), Limits::default())
  }

  /// Creates a parser over a prepared document entity.
  ///
  /// Use this when the system identifier, the encoding or the limits are known in advance.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_parser::{CharStream, Entity, Limits, Parser};
  ///
  /// let document = Entity::document(CharStream::with_encoding("UTF-8")?.with_system_id("file:///doc.xml"));
  /// let mut parser = Parser::with_document(document, Limits::default().with_max_depth(16));
  /// parser.feed(b"<a/>", true)?;
  /// parser.advance()?;
  /// assert_eq!(parser.location().system_id.as_deref(), Some("file:///doc.xml"));
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  #[must_use]
  pub fn with_document(document: Entity, limits: Limits) -> Self {
    let mut pool = NamePool::new();
    let space_name = pool.intern("space");
    let lang_name = pool.intern("lang");
    Self {
      stack: EntityStack::new(document, limits),
      pool,
      space_name,
      lang_name,
      scope: NamespaceScope::new(),
      open: Vec::new(),
      phase: Phase::Prolog,
      seen_doctype: false,
      end_pending: false,
      kind: None,
      token: String::new(),
      token_at: Location::unknown(),
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
    }
  }

  /// Supplies bytes of the document, or of whatever entity is innermost.
  ///
  /// # Errors
  ///
  /// See [`EntityStack::feed`].
  pub fn feed(&mut self, bytes: &[u8], last: bool) -> Result<()> {
    self.stack.feed(bytes, last)
  }

  /// Advances to the next event.
  ///
  /// # Errors
  ///
  /// Returns [`ErrorKind::WellFormedness`] or [`ErrorKind::Namespace`] for a document that
  /// breaks the rules, and passes on decoding and limit errors.
  pub fn advance(&mut self) -> Result<Progress> {
    if self.end_pending {
      self.end_pending = false;
      let open = self.open.pop().expect("an empty element was left open");
      self.close(open);
      self.kind = Some(EventKind::EndElement);
      return Ok(Progress::Event(EventKind::EndElement));
    }
    loop {
      let (token, len) = {
        let stream = self.stack.current().stream();
        let rest = stream.remainder();
        if rest.is_empty() {
          if !stream.is_complete() {
            return Ok(Progress::NeedMoreInput);
          }
          if self.stack.depth() > 1 {
            self.stack.pop();
            continue;
          }
          return self.finish();
        }
        match scan(rest, stream.is_complete()).map_err(|e| e.at(self.stack.location()))? {
          Some(found) => found,
          None => return Ok(Progress::NeedMoreInput),
        }
      };

      self.token_at = self.stack.location();
      self.token.clear();
      self.token.push_str(&self.stack.current().stream().remainder()[..len]);
      self.stack.current_mut().stream_mut().advance(len);

      // Take the scratch out so the interpreting code can borrow it while mutating `self`.
      let text = std::mem::take(&mut self.token);
      let outcome = self.interpret(token, &text);
      self.token = text;

      if let Some(kind) = outcome? {
        self.kind = Some(kind);
        return Ok(Progress::Event(kind));
      }
    }
  }

  /// Checks that the document is allowed to end here.
  fn finish(&mut self) -> Result<Progress> {
    if let Some(open) = self.open.last() {
      let message = format!("element <{}> is never closed", &self.names[open.lexical.clone()]);
      return Err(self.error(ErrorKind::WellFormedness, message));
    }
    if self.phase == Phase::Prolog {
      return Err(self.error(ErrorKind::WellFormedness, "the document has no root element"));
    }
    self.kind = None;
    Ok(Progress::Eof)
  }

  /// Interprets one token, returning the event to report, if any.
  fn interpret(&mut self, token: Token, text: &str) -> Result<Option<EventKind>> {
    if !matches!(token, Token::StartTag | Token::EndTag) {
      // Everything else inherits the context of the enclosing element.
      self.xml_space = self.open.last().map_or(XmlSpace::Default, |e| e.xml_space);
      self.xml_lang = self.open.last().and_then(|e| e.xml_lang);
    }
    match token {
      Token::Pi => self.processing_instruction(text),
      Token::Comment => self.comment(text).map(Some),
      Token::Doctype => self.doctype(text).map(Some),
      Token::StartTag => self.start_tag(text).map(Some),
      Token::EndTag => self.end_tag(text).map(Some),
      Token::CData => self.cdata(text).map(Some),
      Token::Text => self.character_data(text),
    }
  }

  /// Splits `<?...?>` into the XML declaration and ordinary processing instructions.
  fn processing_instruction(&mut self, text: &str) -> Result<Option<EventKind>> {
    let body = &text[2..text.len() - 2];
    let target_len = body.find(chars::is_whitespace).unwrap_or(body.len());
    let (target, data) = body.split_at(target_len);

    if target.eq_ignore_ascii_case("xml") {
      // Only a genuine declaration, at the very start of the document entity, is allowed.
      if target != "xml" || self.token_at.offset != 0 || self.stack.depth() > 1 {
        let message = format!("\"{target}\" is a reserved target");
        return Err(self.error(ErrorKind::WellFormedness, message));
      }
      self.xml_declaration(data)?;
      return Ok(Some(EventKind::XmlDeclaration));
    }
    if !chars::is_name(target) {
      let message = format!("{target:?} is not a valid processing instruction target");
      return Err(self.error(ErrorKind::WellFormedness, message));
    }
    self.name = QName::new(None, None, self.pool.intern(target));
    self.text.clear();
    self.text.push_str(data.trim_start_matches(chars::is_whitespace));
    Ok(Some(EventKind::ProcessingInstruction))
  }

  /// Reads the pseudo-attributes of the XML declaration.
  fn xml_declaration(&mut self, data: &str) -> Result<()> {
    let mut rest = data;
    let mut seen: Vec<&str> = Vec::new();
    while !rest.trim_start_matches(chars::is_whitespace).is_empty() {
      let (name, value, tail) = self.pseudo_attribute(rest)?;
      match name {
        "version" if seen.is_empty() => {
          if !value.starts_with("1.") {
            let message = format!("XML version {value:?} is not supported");
            return Err(self.error(ErrorKind::WellFormedness, message));
          }
          self.version = value.to_owned();
        }
        "encoding" if seen == ["version"] => self.declared_encoding = Some(value.to_owned()),
        "standalone" if !seen.is_empty() && !seen.contains(&"standalone") => {
          self.standalone = match value {
            "yes" => Some(true),
            "no" => Some(false),
            other => {
              let message = format!("standalone must be \"yes\" or \"no\", not {other:?}");
              return Err(self.error(ErrorKind::WellFormedness, message));
            }
          };
        }
        other => {
          let message = format!("{other:?} is out of place in the XML declaration");
          return Err(self.error(ErrorKind::WellFormedness, message));
        }
      }
      seen.push(name);
      rest = tail;
    }
    if self.version.is_empty() {
      return Err(self.error(ErrorKind::WellFormedness, "the XML declaration has no version"));
    }
    Ok(())
  }

  /// Reads one `name = "value"` of the XML declaration, returning it and what follows.
  fn pseudo_attribute<'t>(&self, rest: &'t str) -> Result<(&'t str, &'t str, &'t str)> {
    let malformed = |what: &str| self.error(ErrorKind::WellFormedness, format!("the XML declaration {what}"));
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

  fn comment(&mut self, text: &str) -> Result<EventKind> {
    let body = &text[4..text.len() - 3];
    if let Some(i) = body.find("--") {
      let message = "a comment may not contain \"--\"; use \"-\" or end the comment here";
      return Err(self.error_at(ErrorKind::WellFormedness, message, text, 4 + i));
    }
    self.text.clear();
    self.text.push_str(body);
    Ok(EventKind::Comment)
  }

  fn doctype(&mut self, text: &str) -> Result<EventKind> {
    if self.phase != Phase::Prolog {
      let message = "the document type declaration must come before the root element";
      return Err(self.error(ErrorKind::WellFormedness, message));
    }
    if self.seen_doctype {
      return Err(self.error(ErrorKind::WellFormedness, "there may be only one document type declaration"));
    }
    self.seen_doctype = true;
    self.text.clear();
    self.text.push_str(text);
    Ok(EventKind::Doctype)
  }

  fn cdata(&mut self, text: &str) -> Result<EventKind> {
    if self.phase != Phase::Content {
      return Err(self.error(ErrorKind::WellFormedness, "a CDATA section may only appear inside the root element"));
    }
    self.text.clear();
    self.text.push_str(&text[9..text.len() - 3]);
    Ok(EventKind::CData)
  }

  /// Interprets character data. Whitespace outside the root element is dropped.
  fn character_data(&mut self, text: &str) -> Result<Option<EventKind>> {
    if self.phase != Phase::Content {
      if text.chars().all(chars::is_whitespace) {
        return Ok(None);
      }
      let place = if self.phase == Phase::Prolog { "before" } else { "after" };
      let message = format!("text may not appear {place} the root element");
      return Err(self.error(ErrorKind::WellFormedness, message));
    }
    if let Some(i) = text.find("]]>") {
      let message = "\"]]>\" may not appear in text; write \"]]&gt;\"";
      return Err(self.error_at(ErrorKind::WellFormedness, message, text, i));
    }
    let mut out = std::mem::take(&mut self.text);
    out.clear();
    let outcome = self.expand(text, &mut out, false);
    self.text = out;
    outcome?;
    Ok(Some(EventKind::Text))
  }

  fn start_tag(&mut self, text: &str) -> Result<EventKind> {
    match self.phase {
      Phase::Prolog => self.phase = Phase::Content,
      Phase::Content => {}
      Phase::Epilog => {
        return Err(self.error(ErrorKind::WellFormedness, "a document may have only one root element"));
      }
    }
    let limit = self.stack.limits().max_element_depth;
    if self.open.len() >= limit {
      let message = format!(
        "elements are nested more than {limit} deep; raise Limits::max_element_depth if the document is trusted"
      );
      return Err(self.error(ErrorKind::Limit, message));
    }
    let empty = text.ends_with("/>");
    let body = &text[1..text.len() - if empty { 2 } else { 1 }];
    let name_len = body.find(chars::is_whitespace).unwrap_or(body.len());
    let lexical = &body[..name_len];

    self.parse_attributes(&body[name_len..], 1 + name_len, text)?;

    let namespace_mark = self.scope.mark();
    self.declare_namespaces()?;
    let name = self.resolve_element_name(lexical)?;
    self.resolve_attribute_names()?;
    self.check_attribute_uniqueness()?;

    let (xml_space, xml_lang) = self.space_and_lang()?;
    let lexical = self.remember_name(text, 1, name_len);
    self.name = name;
    self.xml_space = xml_space;
    self.xml_lang = xml_lang;
    self.open.push(OpenElement { name, lexical, namespace_mark, xml_space, xml_lang });
    self.end_pending = empty;
    Ok(EventKind::StartElement)
  }

  fn end_tag(&mut self, text: &str) -> Result<EventKind> {
    let name = text[2..text.len() - 1].trim_end_matches(chars::is_whitespace);
    let Some(open) = self.open.last() else {
      let message = format!("</{name}> closes an element that was never opened");
      return Err(self.error(ErrorKind::WellFormedness, message));
    };
    let expected = &self.names[open.lexical.clone()];
    if expected != name {
      let message = format!("</{name}> does not close <{expected}>");
      return Err(self.error(ErrorKind::WellFormedness, message));
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
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + at));
      }

      let name_len = rest[at..].find(|c: char| c == '=' || chars::is_whitespace(c)).unwrap_or(rest.len() - at);
      let name = &rest[at..at + name_len];
      let name_at = base + at;
      at += name_len;
      at += whitespace_len(&rest[at..]);

      if !rest[at..].starts_with('=') {
        // Most often a bare HTML-style attribute such as `checked` or `disabled`.
        let message = format!("attribute \"{name}\" has no value; every XML attribute needs one, as {name}=\"...\"");
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + at));
      }
      at += 1;
      at += whitespace_len(&rest[at..]);

      let Some(quote) = rest[at..].chars().next().filter(|c| *c == '"' || *c == '\'') else {
        let message = format!("the value of \"{name}\" is not quoted; enclose it in \" or '");
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + at));
      };
      at += quote.len_utf8();
      let Some(end) = rest[at..].find(quote) else {
        let message = format!("the value of \"{name}\" is not terminated");
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + at));
      };
      let raw = &rest[at..at + end];
      let raw_at = base + at;
      at += end + quote.len_utf8();

      let Some((prefix, local)) = chars::split_qname(name) else {
        return Err(self.error_at(ErrorKind::Namespace, bad_qname(name, "attribute"), token, name_at));
      };
      let declares_namespace = prefix == Some("xmlns") || (prefix.is_none() && local == "xmlns");
      let start = values.len();
      self.expand_at(raw, values, true, token, raw_at)?;
      let name = QName::new(prefix.map(|p| self.pool.intern(p)), None, self.pool.intern(local));
      self.attributes.push(Attribute { name, value: start..values.len(), declares_namespace });
    }
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
          return Err(self.error(ErrorKind::Namespace, message));
        }
      } else if value == XML_NS_URI || value == XMLNS_NS_URI {
        let message = format!("{value:?} may not be the default namespace");
        return Err(self.error(ErrorKind::Namespace, message));
      }

      let namespace = (!value.is_empty()).then(|| self.pool.intern(&value));
      self.scope.bind(prefix, namespace);
    }
    Ok(())
  }

  fn resolve_element_name(&mut self, name: &str) -> Result<QName> {
    let Some((prefix, local)) = chars::split_qname(name) else {
      return Err(self.error(ErrorKind::Namespace, bad_qname(name, "element")));
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
        return Err(self.error(ErrorKind::WellFormedness, message));
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
            return Err(self.error(ErrorKind::WellFormedness, message));
          }
        };
      } else if attribute.name.local() == self.lang_name {
        lang = (!value.is_empty()).then(|| self.pool.intern(&value));
      }
    }
    Ok((space, lang))
  }

  /// Records the lexical element name so its end tag can be compared with it.
  fn remember_name(&mut self, token: &str, from: usize, len: usize) -> Range<usize> {
    let start = self.names.len();
    self.names.push_str(&token[from..from + len]);
    start..self.names.len()
  }

  /// Expands references in text that is not part of a token being located precisely.
  fn expand(&self, text: &str, out: &mut String, attribute: bool) -> Result<()> {
    self.expand_at(text, out, attribute, text, 0)
  }

  /// Expands references into `out`.
  ///
  /// With `attribute` set, the normalization of XML 1.0 §3.3.3 applies: whitespace written
  /// literally becomes a space, while whitespace written as a character reference is kept.
  fn expand_at(&self, text: &str, out: &mut String, attribute: bool, token: &str, base: usize) -> Result<()> {
    let mut rest = text;
    let mut done = 0;
    while let Some(i) = rest.find(['&', '<']) {
      out.push_str(&normalize(&rest[..i], attribute));
      done += i;
      if rest.as_bytes()[i] == b'<' {
        let message = "\"<\" may not appear in an attribute value; write \"&lt;\"";
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + done));
      }
      let reference = &rest[i..];
      let Some(end) = reference.find(';') else {
        let message = "a reference must end with \";\"; write \"&amp;\" for a literal ampersand";
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, base + done));
      };
      self.expand_reference(&reference[1..end], out, token, base + done)?;
      rest = &reference[end + 1..];
      done += end + 1;
    }
    out.push_str(&normalize(rest, attribute));
    Ok(())
  }

  /// Expands one reference, given its text between `&` and `;`.
  fn expand_reference(&self, body: &str, out: &mut String, token: &str, at: usize) -> Result<()> {
    if let Some(digits) = body.strip_prefix('#') {
      let (digits, radix) = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => (hex, 16),
        None => (digits, 10),
      };
      let code = u32::from_str_radix(digits, radix).ok();
      let Some(c) = code.and_then(char::from_u32).filter(|c| chars::is_char(*c)) else {
        // Almost always a NUL, a C0 control or half a surrogate pair; none can be escaped.
        let message = format!(
          "\"&{body};\" is not a character XML permits, and no escape can represent it \
           (XML 1.0 allows #x9, #xA, #xD, #x20-#xD7FF, #xE000-#xFFFD and #x10000-#x10FFFF)"
        );
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, at));
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
      // Declared entities arrive with the DTD in phase 2.
      name if chars::is_name(name) => {
        let message = format!(
          "entity \"{name}\" is not declared; declare it in the document type declaration, \
           or write \"&amp;{name};\" if a literal ampersand was meant"
        );
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, at));
      }
      other => {
        let message = format!("\"&{other};\" is not a reference; write \"&amp;\" for a literal ampersand");
        return Err(self.error_at(ErrorKind::WellFormedness, message, token, at));
      }
    }
    Ok(())
  }

  /// The kind of the current event, or `None` before the first one and after the last.
  #[must_use]
  pub const fn event(&self) -> Option<EventKind> {
    self.kind
  }

  /// The name of the current element, or the target of a processing instruction.
  #[must_use]
  pub const fn name(&self) -> QName {
    self.name
  }

  /// The local part of [`name`](Self::name).
  #[must_use]
  pub fn local_name(&self) -> &str {
    self.pool.resolve(self.name.local())
  }

  /// The prefix of [`name`](Self::name), if it has one.
  #[must_use]
  pub fn prefix(&self) -> Option<&str> {
    self.name.prefix.map(|p| self.pool.resolve(p))
  }

  /// The namespace name of [`name`](Self::name), if it is in one.
  #[must_use]
  pub fn namespace_uri(&self) -> Option<&str> {
    self.name.namespace().map(|n| self.pool.resolve(n))
  }

  /// The target of the current processing instruction.
  #[must_use]
  pub fn target(&self) -> &str {
    self.local_name()
  }

  /// The text of the current event: character data, the content of a CDATA section, a comment
  /// body, the data of a processing instruction, or a whole document type declaration.
  #[must_use]
  pub fn text(&self) -> &str {
    &self.text
  }

  /// How many attributes the current start tag has, namespace declarations included.
  #[must_use]
  pub fn attribute_count(&self) -> usize {
    self.attributes.len()
  }

  /// The attribute at `index`.
  #[must_use]
  pub fn attribute(&self, index: usize) -> Option<AttributeRef<'_>> {
    self.attributes.get(index).map(|a| AttributeRef {
      name: a.name,
      value: &self.attribute_text[a.value.clone()],
      declares_namespace: a.declares_namespace,
    })
  }

  /// The attributes of the current start tag, in document order.
  pub fn attributes(&self) -> impl Iterator<Item = AttributeRef<'_>> + '_ {
    (0..self.attributes.len()).filter_map(|i| self.attribute(i))
  }

  /// The value of the attribute with this expanded name, if the current tag has it.
  ///
  /// Pass `None` for `namespace` to look for an unprefixed attribute.
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
  #[must_use]
  pub const fn xml_space(&self) -> XmlSpace {
    self.xml_space
  }

  /// The value of `xml:lang` in effect for the current event.
  #[must_use]
  pub fn xml_lang(&self) -> Option<&str> {
    self.xml_lang.map(|l| self.pool.resolve(l))
  }

  /// The version from the XML declaration, or an empty string if there was none.
  #[must_use]
  pub fn version(&self) -> &str {
    &self.version
  }

  /// The encoding named by the XML declaration, which need not be the one actually in use.
  #[must_use]
  pub fn declared_encoding(&self) -> Option<&str> {
    self.declared_encoding.as_deref()
  }

  /// The value of the standalone declaration, if there was one.
  #[must_use]
  pub const fn standalone(&self) -> Option<bool> {
    self.standalone
  }

  /// How deeply elements are nested; 0 outside the root element.
  #[must_use]
  pub fn depth(&self) -> usize {
    self.open.len()
  }

  /// The current position.
  #[must_use]
  pub fn location(&self) -> Location {
    self.stack.location()
  }

  /// The pool holding every name the parser has seen.
  #[must_use]
  pub const fn pool(&self) -> &NamePool {
    &self.pool
  }

  /// Iterates over the remaining events, copying each one.
  ///
  /// Every byte must already have been fed: an iterator has nowhere to report that it needs
  /// more input, so [`Progress::NeedMoreInput`] becomes an error. Use [`advance`](Self::advance)
  /// directly, or one of the readers, when the input arrives in pieces.
  ///
  /// Iteration stops after the first error.
  ///
  /// # Examples
  ///
  /// ```
  /// use xylograph_parser::{Event, Parser};
  ///
  /// let mut parser = Parser::new();
  /// parser.feed(b"<a>hi</a>", true)?;
  ///
  /// let names: Vec<String> = parser
  ///   .events()
  ///   .filter_map(|event| event.ok()?.text().map(ToOwned::to_owned))
  ///   .collect();
  /// assert_eq!(names, ["hi"]);
  /// # Ok::<(), xylograph_core::Error>(())
  /// ```
  pub fn events(&mut self) -> Events<'_> {
    Events { parser: self, done: false }
  }

  /// Builds the error for a prefix used without a declaration, naming the fix.
  fn undeclared_prefix(&self, prefix: NameId) -> Error {
    let name = self.pool.resolve(prefix);
    let message =
      format!("prefix \"{name}\" is not bound; add an xmlns:{name} attribute to this element or an ancestor");
    self.error(ErrorKind::Namespace, message)
  }

  /// Builds an error at the start of the token being interpreted.
  fn error(&self, kind: ErrorKind, message: impl Into<String>) -> Error {
    Error::new(kind, message).at(self.token_at.clone())
  }

  /// Builds an error pointing at `index` within the current token.
  fn error_at(&self, kind: ErrorKind, message: impl Into<String>, token: &str, index: usize) -> Error {
    let mut at = self.token_at.clone();
    for c in token[..index.min(token.len())].chars() {
      if c == '\n' {
        at.line += 1;
        at.column = 1;
      } else {
        at.column += 1;
      }
      at.offset += 1;
    }
    Error::new(kind, message).at(at)
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
      Ok(Progress::Event(_)) => Some(Ok(Event::capture(self.parser))),
      Ok(Progress::Eof) => {
        self.done = true;
        None
      }
      Ok(Progress::NeedMoreInput) => {
        self.done = true;
        let message = "the document is incomplete; feed the remaining bytes before iterating, \
                       or drive the parser with advance() so it can ask for more";
        Some(Err(Error::new(ErrorKind::Internal, message).at(self.parser.location())))
      }
      Err(e) => {
        self.done = true;
        Some(Err(e))
      }
    }
  }
}

fn whitespace_len(text: &str) -> usize {
  text.len() - text.trim_start_matches(chars::is_whitespace).len()
}

/// Explains why `name` is not a usable element or attribute name.
///
/// "not a valid name" alone leaves the author hunting; naming the offending character, or the
/// extra colon, usually points straight at the typo.
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

/// Applies attribute-value normalization to literal text.
fn normalize(text: &str, attribute: bool) -> Cow<'_, str> {
  if !attribute || !text.contains(['\t', '\n']) {
    return Cow::Borrowed(text);
  }
  Cow::Owned(text.replace(['\t', '\n'], " "))
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Renders one event compactly, so tests can assert on a whole document at once.
  fn render(parser: &Parser, kind: EventKind) -> String {
    match kind {
      EventKind::XmlDeclaration => {
        let mut s = format!("?xml {}", parser.version());
        if let Some(encoding) = parser.declared_encoding() {
          s.push_str(&format!(" {encoding}"));
        }
        if let Some(standalone) = parser.standalone() {
          s.push_str(if standalone { " standalone" } else { " not-standalone" });
        }
        s
      }
      EventKind::Doctype => format!("!doctype {}", parser.text()),
      EventKind::StartElement => {
        let mut s = format!("<{}", qualified(parser, parser.name()));
        for attribute in parser.attributes() {
          s.push_str(&format!(" {}={}", qualified(parser, attribute.name), attribute.value));
        }
        s.push('>');
        s
      }
      EventKind::EndElement => format!("</{}>", qualified(parser, parser.name())),
      EventKind::Text => format!("t:{}", parser.text()),
      EventKind::CData => format!("c:{}", parser.text()),
      EventKind::Comment => format!("!:{}", parser.text()),
      EventKind::ProcessingInstruction => format!("?{} {}", parser.target(), parser.text()),
    }
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
        Progress::Event(kind) => events.push(render(&parser, kind)),
        Progress::Eof => return Ok(events),
        Progress::NeedMoreInput => {
          assert!(fed <= bytes.len(), "asked for input after everything was fed");
          let end = (fed + chunk).min(bytes.len());
          parser.feed(&bytes[fed..end], end == bytes.len())?;
          fed = end;
        }
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
      assert_eq!(split.kind(), whole.kind(), "chunk size {chunk} changed the error kind");
    }
    whole
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
    assert_eq!(error("<a><b xmlns:p='urn:p'/><p:c/></a>").kind(), ErrorKind::Namespace);
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
  fn rejects_mismatched_and_stray_end_tags() {
    assert!(error("<a></b>").message().contains("does not close"));
    assert!(error("<a/></a>").message().contains("never opened"));
    assert!(error("<a>").message().contains("never closed"));
    assert_eq!(error("<a></a></a>").kind(), ErrorKind::WellFormedness);
  }

  #[test]
  fn rejects_documents_without_exactly_one_root() {
    assert!(error("").message().contains("no root element"));
    assert!(error("<!--only a comment-->").message().contains("no root element"));
    assert!(error("<a/><b/>").message().contains("only one root"));
    assert!(error("text<a/>").message().contains("before the root"));
    assert!(error("<a/>text").message().contains("after the root"));
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
    assert_eq!(error("<p:a/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<a p:x='1'/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<a xmlns:p=''/>").kind(), ErrorKind::Namespace);
  }

  #[test]
  fn protects_the_reserved_prefixes() {
    assert_eq!(error("<a xmlns:xmlns='urn:x'/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<a xmlns:xml='urn:x'/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<a xmlns:p='http://www.w3.org/XML/1998/namespace'/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<a xmlns='http://www.w3.org/2000/xmlns/'/>").kind(), ErrorKind::Namespace);
    // Rebinding xml to its own namespace name is allowed.
    assert_eq!(trace("<a xmlns:xml='http://www.w3.org/XML/1998/namespace'/>").unwrap().len(), 2);
  }

  #[test]
  fn rejects_malformed_tags() {
    assert!(error("<a x/>").message().contains("no value"));
    assert!(error("<a x=1/>").message().contains("not quoted"));
    assert!(error("<a x='1'y='2'/>").message().contains("separated by whitespace"));
    assert_eq!(error("<a:b:c/>").kind(), ErrorKind::Namespace);
    assert_eq!(error("<1a/>").kind(), ErrorKind::Namespace);
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
    assert!(error("<?xml version='2.0'?><a/>").message().contains("not supported"));
    assert!(error("<?xml encoding='UTF-8' version='1.0'?><a/>").message().contains("out of place"));
    assert!(error("<?xml version='1.0' standalone='maybe'?><a/>").message().contains("standalone"));
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
  /// the remedy and not merely for the complaint. See the guidance in `xylograph_core::error`.
  #[test]
  fn messages_say_what_to_do_next() {
    let cases: [(&str, &str); 8] = [
      ("<a>&nosuch;</a>", "write \"&amp;nosuch;\""),
      ("<a>Tom & Jerry</a>", "write \"&amp;\""),
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
    assert_eq!(parser.attribute_count(), 3);
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
        Progress::Event(EventKind::Text) => text.push_str(parser.text()),
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
        text = Some(parser.text().to_owned());
      }
    }
    assert_eq!(text.as_deref(), Some("é"));
  }
}
