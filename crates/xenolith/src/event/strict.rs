//! Defines a [`StrictXmlValidator`] that verifies whether a sequence of events constitutes a well-formed XML document.
//!
//! The [`StrictXmlValidator`] rejects events that cannot occur in an XML 1.0 document that is both well-formed and
//! namespace-well-formed. This validator does not assume the event sequence originated from a parser; it performs the
//! same validation regardless of the event source — whether the events arise from tree traversal, programmatic
//! construction, or the replay of stored events. When placed before an [`XmlWriter`](crate::io::write::XmlWriter)
//! within a [`Dispatch`](crate::event::Dispatch), it can reject invalid events before they are serialized into a byte
//! stream.
//!
//! In xenolith, documents that have not passed validation by this validator are referred to as "Loose XML." While they
//! may actually be well-formed XML, there is nothing to indicate that they are indeed XML.
//!
//! The [`Parser`](crate::io::parse::Parser) validates most of these constraints during the reading process. If the
//! events are known to originate from a parser, redundant checks can be avoided by using
//! [`StrictXmlValidator::lexical_only`].
//!

#[cfg(test)]
mod test;

use crate::attr::AttributeRef;
use crate::chars;
use crate::error::{Error, Location, Result};
use crate::event::{
  CdataEventRef, CharactersEventRef, CommentEventRef, DoctypeEventRef, EndElementEventRef, EventHandler, EventRef,
  ProcessingInstructionEventRef, StartElementEventRef,
};
use crate::name::{self, XML_NS_URI, XML_PREFIX, XMLNS_NS_URI, XMLNS_PREFIX};

/// Validates whether the input events constitute a well-formed XML document.
///
/// The constraints and the rules upon which they are based are as follows:
///
/// - Document structure:
///   - Document events must be placed between [`StartDocument`](EventRef::StartDocument) and
///     [`EndDocument`](EventRef::EndDocument) (XML 1.0 §2.1).
///   - There must be exactly one root element, and every started element must be closed in the correct order by an
///     end element with the same name (XML 1.0 §2.1, §3, WFC: Element Type Match).
///   - A document type declaration must appear before the root element and occur at most once (XML 1.0 §2.8).
///   - Outside the root element, text must consist only of whitespace characters, and no CDATA sections may exist
///     (XML 1.0 §2.1, §2.7).
/// - Names and markup:
///   - Element names or attribute names must be a `QName`; that is, a `Name` (XML 1.0 §2.3) containing at most one
///     colon that separates a non-empty prefix from a non-empty local part (Namespaces in XML §4).
///   - In a start tag, no attributes with duplicate expanded names may be specified (XML 1.0 §3.1, WFC: Unique Att
///     Spec; Namespaces §6.3).
///   - Comments must not contain `--` and must not end with `-` (XML 1.0 §2.5).
///   - Processing instruction targets must be a `Name` other than `xml` (case-insensitive), and the data must not
///     contain `?>` (XML 1.0 §2.6).
///   - CDATA sections must not contain `]]>` (XML 1.0 §2.7).
///   - A document type declaration must have a `Name`, the public identifier must consist solely of `PubidChar`
///     characters, and a system identifier must be present. Additionally, the system identifier must not mix the two
///     types of quotation marks (single and double quotes) (XML 1.0 §2.8, §4.2.2).
///   - All characters in text, CDATA sections, comments, processing instruction data, attribute values, and system
///     identifiers must be `Char` (XML 1.0 §2.2).
///   - The value of `xml:space` must be either `default` or `preserve` (XML 1.0 §2.10).
/// - Namespaces:
///   - Any prefix used must be declared, and the namespace associated with an event must be the namespace to which
///     that prefix is bound (or the default namespace for elements without a prefix). Attributes without a prefix
///     must not belong to any namespace (Namespaces §5, §6).
///   - The prefix `xmlns` must not be declared; the prefix `xml` must be bound only to the XML namespace (and that
///     namespace bound only to `xml`); the `xmlns` namespace must not be bound to anything (including the default
///     namespace); and no prefix may be bound to an empty name (Namespaces §3).
///
/// The first violation is returned as an [`Err`] from the event containing the violation. The error location is the
/// character position if the event holds text containing the violation (text, CDATA section, comment, processing
/// instruction data, or attribute value), the attribute position if the issue concerns an attribute, or the event's
/// start position otherwise. Since [`EndDocument`](EventRef::EndDocument) lacks position information, the error
/// location for an unclosed element is the position of the corresponding start element. Since each
/// [`StartDocument`](EventRef::StartDocument) resets the validator, a single validator can sequentially check multiple
/// documents, even if processing stops midway.
///
/// Since each [`StartDocument`](EventRef::StartDocument) resets the internal state of the validator, a single
/// validator can be used to validate multiple documents, even if processing stops midway.
///
/// For applications that need to allow certain rule violations for various reasons, you can implement a custom
/// [`EventHandler`] that wraps this and modifies the events being passed. The second example below allows comments
/// containing `--`.
///
/// # Examples
///
/// ```
/// use xenolith::event::strict::StrictXmlValidator;
/// use xenolith::event::{EventCursor, EventSource};
/// use xenolith::io::StreamSource;
///
/// let xml = "<a>\n  <!-- a -- b -->\n</a>";
/// let mut strict = StrictXmlValidator::new();
/// let error = StreamSource::new(xml.as_bytes()).with_handler(&mut strict).emit().unwrap_err();
/// assert!(error.message().contains("--"));
/// // Located at the first of the two dashes.
/// assert_eq!((error.location().line, error.location().column), (2, 10));
/// ```
///
/// For example, even if a legacy system stores handwritten XML containing comments that include `--`, you can allow
/// the use of `--` while still verifying all other constraints, as shown below.
///
/// ```
/// use xenolith::Result;
/// use xenolith::event::strict::StrictXmlValidator;
/// use xenolith::event::{CommentEventRef, EventCursor, EventHandler, EventRef, EventSource};
/// use xenolith::io::StreamSource;
///
/// /// Checks a document as strictly as `StrictXmlValidator`, except that a comment may hold `--`.
/// #[derive(Default)]
/// struct CommentDashesAllowed {
///   strict: StrictXmlValidator,
///   text: String,
/// }
///
/// impl EventHandler for CommentDashesAllowed {
///   fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
///     let EventRef::Comment(comment) = event else { return self.strict.handle(event) };
///     // One character is replaced by one, so every position is kept. A final dash is not followed by another and
///     // stays, so a comment that ends in `-` is still refused.
///     self.text.clear();
///     let mut chars = comment.text.chars().peekable();
///     while let Some(c) = chars.next() {
///       self.text.push(if c == '-' && chars.peek() == Some(&'-') { '_' } else { c });
///     }
///     self.strict.handle(&EventRef::Comment(CommentEventRef::new(&self.text, comment.location.clone())))
///   }
/// }
///
/// let mut relaxed = CommentDashesAllowed::default();
/// StreamSource::new("<a><!-- -- --></a>".as_bytes()).with_handler(&mut relaxed).emit()?;
///
/// // Only the dashes are excused: the mismatched end tag is still refused.
/// let mut relaxed = CommentDashesAllowed::default();
/// let error = StreamSource::new("<a><!-- -- --></b>".as_bytes()).with_handler(&mut relaxed).emit().unwrap_err();
/// assert!(error.message().contains("</b> does not close <a>"), "{error}");
/// # Ok::<(), xenolith::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct StrictXmlValidator {
  /// Whether to check only those constraints not verified by the parser (configured via
  /// [`lexical_only`](Self::lexical_only)).
  lexical_only: bool,
  /// The progress of event processing within the document.
  phase: Phase,
  /// `true` if a document type declaration has been encountered in the current document.
  seen_doctype: bool,
  /// A stack of elements that have opened but not yet ended (ordered from outermost to innermost).
  open: Vec<OpenElement>,
  /// A stack of namespace bindings in scope (with the innermost bindings placed last); each element consists of a
  /// prefix (`None` for the default namespace) and a namespace name (`None` if the default namespace declaration is
  /// undeclared via `xmlns=""`).
  bindings: Vec<(Option<String>, Option<String>)>,
}

/// An identifier indicating the stage of event progression within the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
  /// Outside the document — that is, before [`StartDocument`](EventRef::StartDocument) or after [`EndDocument`]
  #[default]
  Outside,
  /// Before the root element of the document.
  Prolog,
  /// Inside the root element of the document.
  Content,
  /// After the root element of the document.
  Epilog,
}

/// Stores information about elements that have started in the event sequence so far but have not yet ended.
#[derive(Clone, Debug)]
struct OpenElement {
  /// The prefix of the start element; used for comparison with the end element.
  prefix: Option<String>,
  /// The local name of the start element; used for comparison with the end element.
  local: String,
  /// The namespace of the start element; used for comparison with the end element.
  namespace: Option<String>,
  /// The number of bindings in scope prior to the declaration of this element; retained to restore the scope stack
  /// when the element ends.
  bindings: usize,
  /// The location of the start element; used when reporting elements left unclosed.
  location: Location,
}

impl StrictXmlValidator {
  /// Creates a validator.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Creates a validator that checks only those constraints not verified by the [`Parser`](crate::io::parse::Parser)
  /// during parsing. In other words, it should be used with event sources that driven by a
  /// [`Parser`](crate::io::Parser), such as a [`StreamSource`](crate::io::StreamSource), because the parser itself
  /// rejects all other violations.
  ///
  /// Specifically, it verifies that element and attribute names are valid `QNames`, that
  /// there are no duplicate attributes, that comments do not contain `--` or end with `-`, and that processing
  /// instruction targets are valid `Names`. Since tree structure, namespaces, and character validation are not
  /// performed, please use the standard [`new`](Self::new) for events from other sources, such as tree traversals or
  /// events generated programmatically.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::event::EventCursor;
  /// use xenolith::event::strict::StrictXmlValidator;
  /// use xenolith::io::StreamSource;
  /// use xenolith::event::EventSource;
  ///
  /// let mut strict = StrictXmlValidator::lexical_only();
  /// let error = StreamSource::new("<a><!-- a -- b --></a>".as_bytes()).with_handler(&mut strict).emit().unwrap_err();
  /// assert!(error.message().contains("--"));
  /// ```
  #[must_use]
  pub fn lexical_only() -> Self {
    Self { lexical_only: true, ..Self::default() }
  }

  /// Checks the position, namespace, and attribute values of the start element within the document and initiates its
  /// scope.
  fn start_element(&mut self, event: &StartElementEventRef<'_>) -> Result<()> {
    let at = &event.location;
    match self.phase {
      Phase::Prolog => self.phase = Phase::Content,
      Phase::Content => {}
      Phase::Epilog | Phase::Outside => {
        return Err(Error::well_formedness("a document may have only one root element").at(at.clone()));
      }
    }

    // Declarations are bound first, because they are in scope for the element's own name and for all its attributes.
    let bindings = self.bindings.len();
    for attribute in event.attributes.iter() {
      if let Some((i, c)) = first_non_char(attribute.value) {
        let message = format!("the value of attribute \"{}\" contains {}", attribute.lexical(), not_a_char(c));
        let at = advanced(&attribute.value_location, "", &attribute.value[..i]);
        return Err(Error::well_formedness(message).at(at));
      }
      if attribute.declares_namespace() {
        let prefix = attribute.prefix.map(|_| attribute.local);
        if let Some(message) = bad_declaration(prefix, attribute.value) {
          return Err(Error::namespace(message).at(attribute.location.clone()));
        }
        let namespace = (!attribute.value.is_empty()).then(|| attribute.value.to_owned());
        self.bindings.push((prefix.map(ToOwned::to_owned), namespace));
      }
    }

    let bound = self.bound(event.prefix, at)?;
    if event.namespace != bound {
      let message = format!(
        "element \"{}\" is given {}, but its name is in {}",
        event.lexical(),
        describe(event.namespace),
        describe(bound)
      );
      return Err(Error::namespace(message).at(at.clone()));
    }
    for attribute in event.attributes.iter().filter(|attribute| !attribute.declares_namespace()) {
      // An unprefixed attribute is in no namespace, since the default namespace does not apply to attributes
      // (Namespaces §6.2).
      let bound = if attribute.prefix.is_some() { self.bound(attribute.prefix, &attribute.location)? } else { None };
      if attribute.namespace != bound {
        let message = format!(
          "attribute \"{}\" is given {}, but its name is in {}",
          attribute.lexical(),
          describe(attribute.namespace),
          describe(bound)
        );
        return Err(Error::namespace(message).at(attribute.location.clone()));
      }
      if attribute.prefix == Some(XML_PREFIX)
        && attribute.local == "space"
        && !matches!(attribute.value, "default" | "preserve")
      {
        let message = format!("xml:space must be \"default\" or \"preserve\", not {:?}", attribute.value);
        return Err(Error::well_formedness(message).at(attribute.value_location.clone()));
      }
    }

    self.open.push(OpenElement {
      prefix: event.prefix.map(ToOwned::to_owned),
      local: event.local.to_owned(),
      namespace: event.namespace.map(ToOwned::to_owned),
      bindings,
      location: at.clone(),
    });
    Ok(())
  }

  /// Checks that an end element closes the innermost open element, and closes that element's scope.
  fn end_element(&mut self, event: &EndElementEventRef<'_>) -> Result<()> {
    let at = &event.location;
    let Some(open) = self.open.last() else {
      let message = format!("</{}> closes an element that was never opened", event.lexical());
      return Err(Error::well_formedness(message).at(at.clone()));
    };
    if open.prefix.as_deref() != event.prefix || open.local != event.local {
      let expected = name::lexical(open.prefix.as_deref(), &open.local);
      let message =
        format!("<{expected}>{} is closed with an invalid </{}>", opened_at(&open.location), event.lexical());
      return Err(Error::well_formedness(message).at(at.clone()));
    }
    if open.namespace.as_deref() != event.namespace {
      let message = format!(
        "</{}> is given {}, but its start element{} was in {}",
        event.lexical(),
        describe(event.namespace),
        opened_at(&open.location),
        describe(open.namespace.as_deref())
      );
      return Err(Error::namespace(message).at(at.clone()));
    }
    let open = self.open.pop().expect("just inspected");
    self.bindings.truncate(open.bindings);
    if self.open.is_empty() {
      self.phase = Phase::Epilog;
    }
    Ok(())
  }

  /// Checks that text outside the root element is whitespace, and that every character is a `Char`.
  fn characters(&self, event: &CharactersEventRef<'_>) -> Result<()> {
    let outside = self.phase != Phase::Content;
    if let Some(i) = event.text.find(|c| !chars::is_whitespace(c)).filter(|_| outside) {
      let place = if self.phase == Phase::Prolog { "before" } else { "after" };
      let message = format!("text may not appear {place} the root element");
      return Err(Error::well_formedness(message).at(advanced(&event.location, "", &event.text[..i])));
    }
    check_chars(event.text, &event.location, "")
  }

  /// Checks that a CDATA section is inside the root element, holds no `]]>`, and holds only `Char`s.
  fn cdata(&self, event: &CdataEventRef<'_>) -> Result<()> {
    if self.phase != Phase::Content {
      let message = "a CDATA section may only appear inside the root element";
      return Err(Error::well_formedness(message).at(event.location.clone()));
    }
    if let Some(i) = event.text.find("]]>") {
      let message = "a CDATA section may not contain \"]]>\"";
      return Err(Error::well_formedness(message).at(advanced(&event.location, "<![CDATA[", &event.text[..i])));
    }
    check_chars(event.text, &event.location, "<![CDATA[")
  }

  /// Checks a document type declaration's position in the document and the form of its name and identifiers.
  fn doctype(&mut self, event: &DoctypeEventRef<'_>) -> Result<()> {
    let at = &event.location;
    let refuse = |message: String| Err(Error::well_formedness(message).at(at.clone()));
    if self.phase != Phase::Prolog {
      return refuse("the document type declaration must come before the root element".to_owned());
    }
    if self.seen_doctype {
      return refuse("there may be only one document type declaration".to_owned());
    }
    self.seen_doctype = true;
    let Some(name) = event.name else {
      return refuse("the document type declaration has no name".to_owned());
    };
    if !chars::is_name(name) {
      return refuse(format!("{name:?} is not a valid document type name"));
    }
    if let Some(public_id) = event.public_id {
      if event.system_id.is_none() {
        return refuse(format!("public identifier {public_id:?} is not followed by a system identifier"));
      }
      if let Some(c) = public_id.chars().find(|c| !chars::is_pubid_char(*c)) {
        return refuse(format!("public identifier {public_id:?} contains {c:?}, which a public identifier may not"));
      }
    }
    if let Some(system_id) = event.system_id {
      if system_id.contains('"') && system_id.contains('\'') {
        return refuse(format!("system identifier {system_id:?} holds both kinds of quotation mark"));
      }
      if let Some((_, c)) = first_non_char(system_id) {
        return refuse(format!("system identifier {system_id:?} contains {}", not_a_char(c)));
      }
    }
    Ok(())
  }

  /// Begins a document, discarding the state of the previous one.
  fn start_document(&mut self) -> Result<()> {
    // The collections are cleared rather than replaced, so that checking several documents keeps their allocations.
    self.phase = Phase::Prolog;
    self.seen_doctype = false;
    self.open.clear();
    self.bindings.clear();
    Ok(())
  }

  /// Checks that the document has a root element and no element left open, and ends the document.
  fn end_document(&mut self) -> Result<()> {
    self.phase = match (self.open.last(), self.phase) {
      (Some(open), _) => {
        // `EndDocument` has no location, so the error is located at the start element of the innermost open element.
        let message = format!("element <{}> is not closed", name::lexical(open.prefix.as_deref(), &open.local));
        return Err(Error::well_formedness(message).at(open.location.clone()));
      }
      (None, Phase::Prolog) => return Err(Error::well_formedness("the document has no root element")),
      _ => Phase::Outside,
    };
    Ok(())
  }

  /// The namespace that `prefix` is bound to, or the default namespace when `prefix` is `None`. A prefix that is not
  /// bound is refused with an error located at `at`.
  fn bound(&self, prefix: Option<&str>, at: &Location) -> Result<Option<&str>> {
    // The prefix `xml` is bound by definition and needs no declaration (Namespaces §3).
    if prefix == Some(XML_PREFIX) {
      return Ok(Some(XML_NS_URI));
    }
    let namespace =
      self.bindings.iter().rev().find(|(bound, _)| bound.as_deref() == prefix).and_then(|(_, ns)| ns.as_deref());
    match (prefix, namespace) {
      (Some(prefix), None) => {
        let message =
          format!("prefix \"{prefix}\" is not bound; add an xmlns:{prefix} attribute to this element or an ancestor");
        Err(Error::namespace(message).at(at.clone()))
      }
      _ => Ok(namespace),
    }
  }
}

impl EventHandler for StrictXmlValidator {
  fn handle(&mut self, event: &EventRef<'_>) -> Result<()> {
    if self.lexical_only {
      return lexical(event);
    }
    if let EventRef::StartDocument = event {
      return self.start_document();
    }
    // Checked before the content of the event, so that an event outside a document is refused for being there.
    if self.phase == Phase::Outside {
      let message = "an event arrived outside a document, before its StartDocument or after its EndDocument";
      return Err(Error::well_formedness(message).at(event.location().cloned().unwrap_or_else(Location::unknown)));
    }
    lexical(event)?;
    match event {
      EventRef::StartDocument => unreachable!("StartDocument is handled above"),
      EventRef::EndDocument => self.end_document(),
      EventRef::StartElement(event) => self.start_element(event),
      EventRef::EndElement(event) => self.end_element(event),
      EventRef::Characters(event) => self.characters(event),
      EventRef::Cdata(event) => self.cdata(event),
      EventRef::Comment(event) => check_chars(event.text, &event.location, "<!--"),
      EventRef::ProcessingInstruction(event) => processing_instruction(event),
      EventRef::Doctype(event) => self.doctype(event),
    }
  }
}

/// Validates constraints that can be determined from a single event but are not checked by the parser during loading
/// (such as element names, attribute names, duplicate attributes, hyphens within comments, and processing instruction
/// targets).
fn lexical(event: &EventRef<'_>) -> Result<()> {
  match event {
    EventRef::StartElement(event) => lexical_start_element(event),
    EventRef::EndElement(event) => check_name(event.prefix, event.local, "element", &event.location),
    EventRef::Comment(event) => lexical_comment(event),
    EventRef::ProcessingInstruction(event) => lexical_target(event),
    _ => Ok(()),
  }
}

/// Checks a start element's name, each attribute's name, and that no attribute is given twice.
fn lexical_start_element(event: &StartElementEventRef<'_>) -> Result<()> {
  check_name(event.prefix, event.local, "element", &event.location)?;
  let attributes: Vec<_> = event.attributes.iter().collect();
  for (i, attribute) in attributes.iter().enumerate() {
    check_name(attribute.prefix, attribute.local, "attribute", &attribute.location)?;
    // Compared by expanded name, not by prefix: two attributes with the same namespace and local part are the same
    // attribute, whatever prefixes they were written with (Namespaces §6.3).
    let same = |a: &&AttributeRef<'_>| a.namespace == attribute.namespace && a.local == attribute.local;
    if let Some(other) = attributes[..i].iter().find(same) {
      // Located at the repetition, since the first occurrence alone would have been allowed.
      let message = format!("attribute \"{}\" appears twice", other.lexical());
      return Err(Error::well_formedness(message).at(attribute.location.clone()));
    }
  }
  Ok(())
}

/// Checks that a comment's body holds no `--` and does not end in `-`.
fn lexical_comment(event: &CommentEventRef<'_>) -> Result<()> {
  if let Some(i) = event.text.find("--") {
    let message = "a comment may not contain \"--\"; use \"-\" or end the comment here";
    return Err(Error::well_formedness(message).at(advanced(&event.location, "<!--", &event.text[..i])));
  }
  // `Comment ::= '<!--' ((Char - '-') | ('-' (Char - '-')))* '-->'` does not allow a final dash either: in `<!--a--->`
  // the body `a-` is followed by `-->`, which puts three dashes together.
  if event.text.ends_with('-') {
    let message = "a comment may not end with \"-\" before its \"-->\"";
    let at = advanced(&event.location, "<!--", &event.text[..event.text.len() - 1]);
    return Err(Error::well_formedness(message).at(at));
  }
  Ok(())
}

/// Checks that a processing instruction's target is a `Name`.
fn lexical_target(event: &ProcessingInstructionEventRef<'_>) -> Result<()> {
  if chars::is_name(event.target) {
    return Ok(());
  }
  let message = format!("{:?} is not a valid processing instruction target", event.target);
  Err(Error::well_formedness(message).at(event.location.clone()))
}

/// Checks that a processing instruction's target is not reserved, and that its data holds no `?>` and only `Char`s.
fn processing_instruction(event: &ProcessingInstructionEventRef<'_>) -> Result<()> {
  // `PITarget ::= Name - (('X' | 'x') ('M' | 'm') ('L' | 'l'))`
  if event.target.eq_ignore_ascii_case(XML_PREFIX) {
    let message = format!("\"{}\" is a reserved target", event.target);
    return Err(Error::well_formedness(message).at(event.location.clone()));
  }
  if let Some(i) = event.data.find("?>") {
    let message = "processing instruction data may not contain \"?>\"";
    return Err(Error::well_formedness(message).at(advanced(&event.data_location, "", &event.data[..i])));
  }
  check_chars(event.data, &event.data_location, "")
}

/// Checks that a name given as its prefix and local part is a `QName`. Otherwise it is refused with a message that
/// names the fault; `role` says what the name belongs to (an element or an attribute).
fn check_name(prefix: Option<&str>, local: &str, role: &str, at: &Location) -> Result<()> {
  if prefix.is_none_or(chars::is_ncname) && chars::is_ncname(local) {
    return Ok(());
  }
  Err(Error::namespace(bad_qname(&name::lexical(prefix, local), role)).at(at.clone()))
}

/// The reason a declaration that binds `prefix`, or the default namespace when `prefix` is `None`, to `value` is
/// refused (Namespaces §3), or `None` when it is allowed.
fn bad_declaration(prefix: Option<&str>, value: &str) -> Option<String> {
  let Some(prefix) = prefix else {
    return (value == XML_NS_URI || value == XMLNS_NS_URI)
      .then(|| format!("{value:?} may not be the default namespace"));
  };
  if prefix == XMLNS_PREFIX {
    Some("the prefix \"xmlns\" cannot be declared".to_owned())
  } else if value.is_empty() {
    Some(format!("prefix \"{prefix}\" cannot be bound to an empty namespace name"))
  } else if prefix == XML_PREFIX && value != XML_NS_URI {
    Some("the prefix \"xml\" may only be bound to its own namespace name".to_owned())
  } else if prefix != XML_PREFIX && value == XML_NS_URI {
    Some(format!("the XML namespace may not be bound to \"{prefix}\""))
  } else if value == XMLNS_NS_URI {
    Some(format!("the namespace name of xmlns may not be bound to \"{prefix}\""))
  } else {
    None
  }
}

/// Checks that every character of `text` is a `Char`. A violation is located by advancing `at`, the start of the event,
/// over `markup`, the delimiter before the text, and over the text before the character.
fn check_chars(text: &str, at: &Location, markup: &str) -> Result<()> {
  match first_non_char(text) {
    None => Ok(()),
    Some((i, c)) => Err(Error::well_formedness(not_a_char(c)).at(advanced(at, markup, &text[..i]))),
  }
}

/// The byte index and the value of the first character in `text` that is not a `Char`.
fn first_non_char(text: &str) -> Option<(usize, char)> {
  text.char_indices().find(|(_, c)| !chars::is_char(*c))
}

/// The part of a message that names a character that is not a `Char`.
fn not_a_char(c: char) -> String {
  format!("U+{:04X}, which may not appear in XML in any form", c as u32)
}

/// The position of a start element as an end element's message gives it, ` (opened at line 3, column 5)`, or nothing
/// when the position is unknown. The error itself is located at the end element, so the start element can only be
/// named in the message.
fn opened_at(location: &Location) -> String {
  if location.is_unknown() {
    return String::new();
  }
  format!(" (opened at line {}, column {})", location.line, location.column)
}

/// A namespace, or its absence, as a message names it.
fn describe(namespace: Option<&str>) -> String {
  namespace.map_or_else(|| "no namespace".to_owned(), |namespace| format!("namespace {namespace:?}"))
}

/// The location `at` advanced over `markup` and then over `text`, to point at a character inside an event whose
/// location is its start. An unknown location, which a tree walk reports, stays unknown.
fn advanced(at: &Location, markup: &str, text: &str) -> Location {
  let mut at = at.clone();
  for c in markup.chars().chain(text.chars()) {
    at.advance(c);
  }
  at
}

/// The message for a name that is not a `QName`, naming the character or the shape at fault.
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
  if let Some(c) = name.chars().find(|c| !chars::is_name_char(*c)) {
    return format!("{role} name {name:?} contains {c:?}, which names may not");
  }
  // Every character may appear inside a name, so the fault is the first character of a part that may not begin one.
  // The prefix is judged first, so a name whose two parts both begin badly is reported for its prefix.
  let starts_badly = |part: &str| part.chars().next().filter(|c| !chars::is_ncname_start_char(*c));
  match name.split_once(':') {
    Some((prefix, local)) => match (starts_badly(prefix), starts_badly(local)) {
      (Some(c), _) => format!("{role} name {name:?} has a prefix that starts with {c:?}, which may not begin a name"),
      (None, Some(c)) => {
        format!("{role} name {name:?} has a local part that starts with {c:?}, which may not begin a name")
      }
      (None, None) => format!("{role} name {name:?} is not a QName"),
    },
    None => match starts_badly(name) {
      Some(c) => format!("{role} name {name:?} starts with {c:?}, which may not begin a name"),
      None => format!("{role} name {name:?} is not a QName"),
    },
  }
}
