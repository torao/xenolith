//! XInclude 1.0 transformation process.
//!
//! [`XIncludeTransformer`] is a type of [transformer](crate::event#transformer) that performs XInclude 1.0 processing
//! while relaying events through a pipeline. By placing it between an event producer and a consumer, it replaces
//! `xi:include` elements found in the document with the content of the referenced resources and passes the result
//! downstream in the pipeline.
//!
//! Resource retrieval is an I/O operation performed using a [`UriResolver`]. As with external entity references,
//! access destinations should be restricted when dealing with untrusted documents. The URI specified in the `href`
//! attribute is resolved based on the base URI effective at the location of the `xi:include` element.
//!
//! When an external resource is read as XML, it is parsed with XInclude processing enabled. This recursive loading is
//! subject to loop detection for identical resource URIs, as well as depth and frequency limits defined in [`Limits`].
//!
//! If resource retrieval fails, the `xi:fallback` mechanism activates, replacing the `xi:include` element with the
//! content of the `xi:fallback` element. However, fatal errors — such as the resource content not being well-formed
//! XML — do not trigger the fallback. This behavior is defined in XInclude 1.0 §4.2. Failure to retrieve a resource
//! for an `xi:include` element that lacks an `xi:fallback` results in an error. Any `xi:include` elements contained
//! within an `xi:fallback` are processed recursively.
//!
//! If elements included as XML possess effective base URI or language values, those values remain valid after
//! inclusion. If these values differ from the effective values at the `xi:include` location, `xml:base` or `xml:lang`
//! attributes are explicitly added or replaced (§4.5.5, §4.5.6). This behavior can be disabled using
//! [`with_xml_base`](XIncludeTransformer::with_xml_base) and [`with_xml_lang`](XIncludeTransformer::with_xml_lang).
//!
//! [`XIncludeTransformer`] performs its own validation against upstream events using [`XIncludeSchema`]. If the
//! document structure does not comply with the XInclude 1.0 specification, processing halts at the first violation
//! with an [`Error::Validity`] error.
//!
//! The `xpointer` attribute selects a portion of a resource loaded as XML via [`XPointerFilter`]. The
//! [`xpointer`](crate::xpointer) module enumerates the formats subject to evaluation. Pointers that cannot be parsed,
//! pointers that identify nothing, and pointers that the filter cannot evaluate result in a resource error (Section
//! 4.2, "XPointer errors are resource errors"); consequently, `xi:fallback` is used in such cases.
//!
//! # Unimplemented Features
//!
//! - An `xi:include` element with an omitted `href` attribute is treated as a resource error (§4.1). If an
//!   `xi:fallback` element is present, it is used as a replacement; otherwise, processing terminates with an error.
//!   Omitting the `href` attribute implies including the document currently being processed in the event pipeline
//!   (using an XPointer if `parse="xml"` is specified). However, implementing this behavior within the event pipeline
//!   requires constructing and saving the document first. In cases requiring self-reference, please expand the
//!   document as XML into memory or a file so that it can be read via a [`UriResolver`] using the URI specified in the
//!   `href` attribute.
//!
//! # Specifications
//!
//! - [XInclude 1.0 (Second Edition)] — W3C Recommendation, November 15, 2006.
//! - [XML Base (Second Edition)] — W3C Recommendation, January 28, 2009. The resolution of `href` is based on this.
//!
//! [XInclude 1.0 (Second Edition)]: https://www.w3.org/TR/2006/REC-xinclude-20061115/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/

#[cfg(test)]
mod test;
mod validate;

use crate::error::{Error, Result};
use crate::event::strict::StrictXmlConstraints;
use crate::event::validate::ValidatorSet;
use crate::event::{
  CharactersEventRef, Dispatcher, EventConsumer, EventCursor, EventProducer, EventRef, Flow, Outcome,
  StartElementEvent, StartElementEventRef,
};
use crate::io::resolve::{NoResolver, UriResolver};
use crate::io::{CharStream, Entity, EntityRequest, ParserConfig, RequestKind, StreamSource};
use crate::name::XML_PREFIX;
use crate::xinclude::ScopeBehavior::{FallbackInProgress, IgnoreAll, InclusionInProcessing, SeekingFallback};
use crate::xinclude::validate::RootElementConstraints;
pub use crate::xinclude::validate::XIncludeSchema;
use crate::xpointer::{XPointer, XPointerFilter};
use crate::{Attribute, Attributes, Location, XML_NS_URI, uri};
use std::io::Read;

/// The namespace to which the `xi:include` and `xi:fallback` elements belong.
pub const XINCLUDE_NS: &str = "http://www.w3.org/2001/XInclude";

/// Upper limit of the inclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
  /// The maximum number of nested `xi:include` elements that can be open at the same time. This includes both those
  /// currently being included and those serving as fallbacks. The default is 8. `None` means no upper limit.
  pub max_depth: Option<usize>,
  /// The maximum number of `xi:include` elements that can be processed in a single document processing operation. This
  /// includes those from imported documents and those that occur during fallback. The default is 1,000. `None` means
  /// no upper limit.
  pub max_includes: Option<usize>,
  /// The maximum number of characters an `xpointer` can buffer. For an `xpointer` composed of multiple PointerParts,
  /// parts further to the right may need to buffer their selected events until the part to their left begins
  /// transmitting events. This value represents the maximum total number of characters (in terms of character count)
  /// buffered across all parts (specified via [`XPointerFilter::with_max_pending_chars`]). The default is 16 Mi
  /// characters; `None` indicates no upper limit.
  pub max_pending_chars: Option<usize>,
}

impl Default for Limits {
  /// Create a default [`Limits`] with a `max_depth` of 8, a `max_includes` of 1,000, and a `max_pending_chars` of 16 Mi
  /// characters.
  fn default() -> Self {
    Self {
      max_depth: Some(8),
      max_includes: Some(1000),
      max_pending_chars: Some(crate::xpointer::DEFAULT_MAX_PENDING_CHARS),
    }
  }
}

impl Limits {
  /// Creates a [`Limits`] with all restrictions removed. This should only be used for trusted input.
  #[must_use]
  pub fn unlimited() -> Self {
    Self { max_depth: None, max_includes: None, max_pending_chars: None }
  }
}

/// Enumeration values that defines the behavior for events within a given [`IncludeScope`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum ScopeBehavior {
  /// It is sending the event stream from the resource being ingested downstream.
  InclusionInProcessing,
  /// Either `xi:include` succeeded or `xi:fallback` has been processed. Ignore any remaining events up to the closing
  /// tag of `xi:include`.
  IgnoreAll,
  /// Failed to retrieve the resource. The transformer is searching for an `xi:fallback` element among the direct child
  /// elements of `xi:include` and ignores all other nodes.
  SeekingFallback,
  /// It is sending the event stream within `xi:fallback` downstream.
  FallbackInProgress,
}

/// The state of a single `xi:include` element currently being processed. One state is pushed onto the stack for each
/// `xi:include` and removed from the stack when that element is closed.
struct IncludeScope {
  /// The location of the `xi:include` element.
  location: Location,
  /// A resolved and normalized resource URI. It is compared during loop detection as described in §4.2.
  uri: String,
  /// The value of the `xpointer` attribute, which is compared together with `uri` during loop detection (§4.2.7).
  xpointer: Option<String>,
  /// A base URI at the `xi:include` location. It is compared to the value of the root element of the retrieved
  /// document. If no actual value exists, it is `None`.
  xml_base: Option<String>,
  /// A language at the `xi:include` location. It is compared to the value of the root element of the retrieved
  /// document. If no actual value is present, it is `None`.
  xml_lang: Option<String>,
  /// Whether the root element of the document retrieved by this `xi:include` has been observed. This determines
  /// whether to add `xml:base` and `xml:lang` attributes to the element.
  included_root_observed: bool,
  /// The depth of each node, with the node directly under `xi:include` set to 0.
  depth: usize,
  /// A resource retrieval error that occurred within this scope. If a `xi:fallback` is found, the error is canceled;
  /// if not, it is returned as an error at the end of the `xi:include`.
  error: Option<Error>,
  /// A control value that determines how to respond to events that occur within this scope.
  behavior: ScopeBehavior,
}

/// An XInclude 1.0 transformation that can be installed on the event pipeline.
///
/// This transformer replaces the `xi:include` elements in the event sequence received by the [`EventConsumer`] with the
/// resources referenced by those elements, and sends the transformed result to the subsequent [`EventConsumer`]. The
/// received events are validated against the [`XIncludeSchema`], and processing is terminated upon the first violation.
///
/// # Example
///
/// ```
/// use std::io::Read;
/// use xenolith::event::{EventCursor, EventProducer};
/// use xenolith::io::StreamSource;
/// use xenolith::io::resolve::{EntityRequest, UriResolver};
/// use xenolith::io::write::XmlWriter;
/// use xenolith::xinclude::XIncludeTransformer;
///
/// // A resolver backed by a map. Used in place of a file system or catalog.
/// struct Map;
/// impl UriResolver for Map {
///   fn resolve(&self, request: &EntityRequest) -> xenolith::Result<Option<Box<dyn Read>>> {
///     let body: &[u8] = match request.resolved_uri().as_deref() {
///       Some("file:///part.xml") => b"<p>included</p>",
///       _ => return Ok(None),
///     };
///     Ok(Some(Box::new(body)))
///   }
/// }
///
/// let xml = "<doc><xi:include href='part.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></doc>";
/// let mut map = Map;
/// let mut writer = XmlWriter::new(Vec::new());
/// {
///   let mut include = XIncludeTransformer::new().with_resolver(&mut map).add_consumer(&mut writer);
///   StreamSource::with_system_id(xml.as_bytes(), "file:///doc.xml").add_consumer(&mut include).emit()?;
/// }
/// // Embedded elements have an `xml:base` attribute that specifies the resource (part.xml) into which they are
/// // embedded. This can be disabled using `with_xml_base`.
/// let out = String::from_utf8(writer.into_inner()).unwrap();
/// assert_eq!(out, "<doc><p xml:base=\"file:///part.xml\">included</p></doc>");
/// # Ok::<(), xenolith::Error>(())
/// ```
pub struct XIncludeTransformer<'h, 'r> {
  /// 1st stage. Detects XInclude violations during validation of events coming from upstream.
  constraints: ValidatorSet,
  /// 2nd stage. The main body of the `xi:include` processing. The output is sent to the subsequent consumer via the
  /// [`RootElementConstraints`] in 3rd stage.
  process: XIncludeProcess<'h, 'r>,
}

impl Default for XIncludeTransformer<'_, '_> {
  fn default() -> Self {
    Self::new()
  }
}

impl<'h, 'r> XIncludeTransformer<'h, 'r> {
  /// Create a transformation that does not have a resolver (therefore, if it has `xi:fallback`, it will fall back;
  /// otherwise, it will be rejected).
  #[must_use]
  pub fn new() -> Self {
    Self {
      constraints: ValidatorSet::new().add_schema(&XIncludeSchema).with_max_errors(Some(0)),
      process: XIncludeProcess::new(),
    }
  }

  /// Specify the resolver to be used for all resource retrieval operations.
  #[must_use]
  pub fn with_resolver(mut self, resolver: &'r dyn UriResolver) -> Self {
    self.process.resolver = resolver;
    self
  }

  /// Set the limits for the inclusion process.
  #[must_use]
  pub fn with_limits(mut self, limits: Limits) -> Self {
    self.process.limits = limits;
    self
  }

  /// Specify the parser settings to use when loading a document via `xi:include`.
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.process.config = config;
    self
  }

  /// Specifies whether to validate the imported document using [`StrictXmlConstraints`]. By default, validation is
  /// enabled. If you are certain that the document being read is well-formed XML, you can specify `false` to skip
  /// some validation steps.
  #[must_use]
  pub fn with_strict(mut self, strict: bool) -> Self {
    self.process.strict = strict;
    self
  }

  /// Whether to preserve the effective base URI of the embedded element after embedding. The default is `true`.
  ///
  /// The effective base URI of the included element is determined by the element's own `xml:base` attribute,
  /// inheritance from a parent element, or the URI of the document being included. If this differs from the base URI
  /// at the location of the `xi:include`, the `xml:base` attribute is added or replaced (§4.5.5).
  #[must_use]
  pub fn with_xml_base(mut self, xml_base: bool) -> Self {
    self.process.xml_base = xml_base;
    self
  }

  /// Whether to preserve the effective language of the element being incorporated after it is incorporated. The
  /// default is `true`.
  ///
  /// The effective language of the included element is determined by the element's own `xml:lang` attribute or by
  /// inheritance from a parent element. If this differs from the string at the location of the `xi:include`, the
  /// `xml:lang` attribute is added or replaced (§4.5.6).
  #[must_use]
  pub fn with_xml_lang(mut self, xml_lang: bool) -> Self {
    self.process.xml_lang = xml_lang;
    self
  }
}

/// The main body of the `xi:include` processing. In the 2nd stage of [`XIncludeTransformer`], it receives events that
/// have passed validation against the XInclude specification. Events for each document included from `xi:include`
/// elements detected within the document are passed directly to this instance from nested lanes.
struct XIncludeProcess<'h, 'r> {
  /// Subsequent consumers. They receive both events originating from the document itself and events originating from
  /// the document being incorporated.
  dispatch: Dispatcher<'h>,
  /// Constraints that are validated before an event is sent downstream: If the root element of the top-level document
  /// is `xi:include`, the result after substitution may contain only comments, processing instructions, whitespace, or
  /// a single element (§4.5). If the included text contains anything other than whitespace, if the content of
  /// `xi:fallback` contains multiple elements, or if it contains no elements at all, a fatal error occurs. This
  /// constraint ensures that such invalid events are detected before they are sent downstream.
  outcome_constraints: RootElementConstraints,
  /// A resolver provided by the application that is used to retrieve all resources.
  resolver: &'r dyn UriResolver,
  /// Maximum number of reads.
  limits: Limits,
  /// Parser settings used when loading a document with `xi:include`.
  config: ParserConfig,
  /// Whether to apply [`StrictXmlConstraints`] to the document being included.
  strict: bool,
  /// Whether to assign `xml:base` to the top-level element of the retrieved document (§4.5.5).
  xml_base: bool,
  /// Whether to apply `xml:lang` to the top-level element of the retrieved document (§4.5.6).
  xml_lang: bool,
  /// The number of includes performed in this document. Compare this to [`Limits::max_includes`].
  inclusion_occurrence: usize,
  /// Open capture scopes. The last one is the innermost.
  inclusions: Vec<IncludeScope>,
  /// The count of validation errors reported by downstream consumers that have not yet been returned upstream. Events
  /// from included documents reach downstream components via nested lanes, but those lanes cannot return this count;
  /// therefore, the count accumulates here and is returned by [`XIncludeTransformer`] for the upstream event currently
  /// being processed.
  validity_error_count: usize,
}

impl<'h, 'r> XIncludeProcess<'h, 'r> {
  fn new() -> Self {
    Self {
      dispatch: Dispatcher::new(),
      outcome_constraints: RootElementConstraints::default(),
      resolver: NoResolver::shared(),
      limits: Limits::default(),
      config: ParserConfig::default(),
      strict: true,
      xml_base: true,
      xml_lang: true,
      inclusion_occurrence: 0,
      inclusions: Vec::new(),
      validity_error_count: 0,
    }
  }

  /// Discard the inclusion status of the previous document when starting a new document.
  fn reset(&mut self) {
    self.outcome_constraints = RootElementConstraints::default();
    self.inclusions.clear();
    self.inclusion_occurrence = 0;
    self.validity_error_count = 0;
  }

  /// Reads the content specified by `xi:include` and sends it to the downstream as events at the current position.
  fn include(&mut self, event: StartElementEvent) -> Result<()> {
    if self.limits.max_depth.is_some_and(|max| self.inclusions.len() >= max) {
      let at = event.location.clone();
      let message = format!("xi:include depth exceeds {}; increase Limits::max_depth", self.inclusions.len());
      return Err(Error::xinclude(message).at(at));
    }

    self.inclusion_occurrence += 1;
    if self.limits.max_includes.is_some_and(|max| self.inclusion_occurrence > max) {
      let at = event.location.clone();
      let message = format!(
        "the document made more than {} inclusions; raise xinclude::Limits::max_includes",
        self.inclusion_occurrence
      );
      return Err(Error::xinclude(message).at(at));
    }

    // resolve resource URIs
    let href = get_attribute_value(&event, "href");
    let base = event.base_uri.to_owned().or(event.location.system_id.as_ref().map(|b| b.to_string()));
    let uri = match base.as_deref() {
      Some(base) => &uri::resolve(base, href.unwrap_or(""))?,
      None => href.unwrap_or(""),
    };
    let uri = uri::escape_uri(&uri::UriReference::parse(uri)?.normalize().to_string());

    // §4.1: treating the omission of href as a resource error (because the event pipeline cannot retrieve the document
    // itself)
    if href.is_none() {
      let at = event.location.clone();
      let message = "an xi:include without href refers to the document itself, which the event pipeline cannot fetch";
      return self.fallback(event, uri, base, Error::xinclude(message).at(at));
    };

    // §4.2: "An error in the XPointer is a resource error." A pointer that does not parse is one, and is found before
    // anything is fetched.
    let xpointer = get_attribute_value(&event, "xpointer").map(str::to_owned);
    let at = get_attribute_value_location(&event, "xpointer");
    let pointer = match xpointer.as_deref().map(|pointer| XPointer::parse(pointer, at)).transpose() {
      Ok(pointer) => pointer,
      Err(error) => return self.fallback(event, uri, base, error),
    };

    // §4.2.7 "When recursively processing an xi:include element, it is a fatal error to process another xi:include
    // element with an include location and xpointer attribute value that have already been processed in the inclusion
    // chain." However, the scope in which the same (URI, XPointer) pair is detected is limited to the actual
    // processing of the resource. Since the resource has not yet been retrieved during the fallback phase, detecting
    // the same pair does not constitute a loop.
    if let Some(existing) = self
      .inclusions
      .iter()
      .filter(|i| i.behavior == InclusionInProcessing)
      .find(|i| i.uri == uri && i.xpointer == xpointer)
    {
      let at = get_attribute_value_location(&event, "href");
      let message = format!("xi:include is creating a circular reference: {}", existing.location.clone());
      return Err(Error::xinclude(message).at(at));
    }

    // obtain a stream from the resolver for reading
    let reader = match self.fetch(base.as_deref(), &uri, &event) {
      Ok(reader) => reader,
      Err(error) => return self.fallback(event, uri, base, error),
    };

    // create a character conversion stream; if an encoding is specified, apply it
    let encoding = get_attribute_value(&event, "encoding");
    let mut stream = match encoding {
      Some(label) => match CharStream::with_encoding(label) {
        Ok(stream) => stream,
        // §4.3: "the resource is in an unsupported encoding" is a resource error
        Err(error) => {
          let at = get_attribute_value_location(&event, "encoding");
          return self.fallback(event, uri, base, error.or_at(at));
        }
      },
      None => CharStream::new(),
    };
    stream = stream.with_system_id(uri.to_string());

    let include = IncludeScope {
      location: event.location.clone(),
      uri: uri.clone(),
      xpointer,
      xml_base: base.clone(),
      xml_lang: event.xml_lang.clone(),
      included_root_observed: false,
      depth: 0,
      error: None,
      behavior: InclusionInProcessing,
    };
    self.inclusions.push(include);

    // start including in text or XML format
    let result = match get_attribute_value(&event, "parse") {
      Some("text") => self.include_text(reader, stream),
      Some("xml") | None => self.include_xml(reader, stream, pointer.as_ref()),
      Some(unsupported) => {
        let at = get_attribute_value_location(&event, "parse");
        let message = format!(
          "{unsupported} is not supported as a value for the parse attribute; specify either \"text\" or \"xml\"."
        );
        return Err(Error::xinclude(message).at(at));
      }
    };

    // §4.2: "An error in the XPointer is a resource error." When a failure occurs due to an invalid (or unrecognized)
    // XPointer, it is treated as a resource error, and a fallback is performed.
    if let Err(error @ Error::XPointer { .. }) = result {
      let scope = self.inclusions.last_mut().expect("xi:include is opened");
      scope.error = Some(error);
      scope.behavior = SeekingFallback;
      return Ok(());
    }

    self.inclusions.last_mut().expect("xi:include is opened").behavior =
      if result.is_ok() { IgnoreAll } else { SeekingFallback };

    result
  }

  /// Start the scope by logging an `error` that indicates a resource error. If `xi:fallback` is present, the error is
  /// canceled and replaced with its contents; otherwise, `error` is returned when `xi:include` ends.
  fn fallback(&mut self, event: StartElementEvent, uri: String, base: Option<String>, error: Error) -> Result<()> {
    let include = IncludeScope {
      location: event.location.clone(),
      uri,
      xpointer: get_attribute_value(&event, "xpointer").map(str::to_owned),
      xml_base: base,
      xml_lang: event.xml_lang.clone(),
      included_root_observed: false,
      depth: 0,
      error: Some(error),
      behavior: SeekingFallback,
    };
    self.inclusions.push(include);
    Ok(())
  }

  /// reads the resource as XML and sends it to the downstream as an event at the current position; with a `pointer`,
  /// only the part it identifies
  fn include_xml(&mut self, reader: Box<dyn Read>, stream: CharStream, pointer: Option<&XPointer>) -> Result<()> {
    let config = self.config;
    // *pay attention* to variable scope and evaluation order in this block
    let resolver = self.resolver;
    let mut strict = self.strict.then_some(StrictXmlConstraints::lexical_only());
    let mut validators = ValidatorSet::default().add_schema(&XIncludeSchema).with_max_errors(Some(0));
    // The whole resource is checked, and only the selected part reaches this process.
    let mut filter;
    let consumer: &mut dyn EventConsumer = match pointer {
      Some(pointer) => {
        filter = XPointerFilter::new(pointer).with_max_pending_chars(self.limits.max_pending_chars).add_consumer(self);
        &mut filter
      }
      None => self,
    };
    let mut lane = Dispatcher::new();
    if let Some(strict) = strict.as_mut() {
      lane = lane.add_validator(strict);
    }
    lane = lane.add_validator(&mut validators);
    lane = lane.add_consumer(consumer);

    let mut source = StreamSource::with_document(reader, Entity::document(stream)).with_config(config);
    source = source.with_resolver(resolver);
    source.add_consumer(&mut lane).emit()
  }

  /// Reads the resource as text and sends it to subsequent elements as a text event at the current position.
  ///
  /// Text reading can fail in two cases, and §4.3 classifies both as fatal errors.: "Byte sequences outside the range
  /// allowed by the encoding are a fatal error" and "Characters that are not permitted in XML documents also are a
  /// fatal error." Both sides report via the lower-level stream.
  fn include_text(&mut self, mut reader: Box<dyn Read>, mut stream: CharStream) -> Result<()> {
    // Specifying a fragment length of 0 will cause the process to stall, so a minimum read of 1 byte is guaranteed.
    let buffer_size = self.config.text_fragment_len.max(1);
    let mut buffer = vec![0; buffer_size];
    let mut last = false;
    while !last {
      // read into the buffer
      let len = reader.read(&mut buffer).map_err(|error| {
        let at = stream.location();
        Error::io(error.to_string()).at(at).caused_by(error)
      })?;
      if len > buffer_size {
        return Err(Error::overlong_read(len, buffer_size));
      }
      last = len == 0;

      // convert to text
      stream.feed(&buffer[..len], last).map_err(|error| {
        let at = stream.location();
        error.or_at(at)
      })?;

      // send all the converted text
      while stream.remainder().len() >= buffer_size || (last && !stream.remainder().is_empty()) {
        let at = stream.location();
        let len = fragmentation_length(stream.remainder(), buffer_size);
        let text = &stream.remainder()[..len];

        // Passed on directly: this text is the inclusion itself, not a document whose layout is left out.
        self.passthrough(&EventRef::Characters(CharactersEventRef::new(text, at)))?;
        if self.dispatch.is_stopped() {
          return Ok(());
        }

        stream.advance(len);
      }
    }
    Ok(())
  }

  /// Query the resolver for a resource.
  fn fetch(&mut self, base: Option<&str>, uri: &str, event: &StartElementEvent) -> Result<Box<dyn Read>> {
    let at = get_attribute_value_location(event, "href");
    let request = EntityRequest::new(
      None,
      None,
      uri.to_owned(),
      base.map(str::to_owned),
      RequestKind::XInclude {
        accept: get_attribute_value(event, "accept").map(String::from),
        accept_language: get_attribute_value(event, "accept-language").map(String::from),
      },
    );

    let read = self.resolver.resolve(&request).map_err(|error| error.or_at(at.clone()))?;
    let Some(read) = read else {
      let message = format!("the resolver rejected the URI {uri} reference");
      return Err(Error::xinclude(message).at(at));
    };
    Ok(read)
  }

  /// Send a single event to a downstream consumer. The number of validity errors that the downstream reports is added
  /// to `validity_error_count`.
  fn passthrough(&mut self, event: &EventRef<'_>) -> Result<()> {
    let (Flow::Continue(constraints_count) | Flow::Break(constraints_count)) =
      self.outcome_constraints.consume(event)?;
    let (Flow::Continue(downstream_count) | Flow::Break(downstream_count)) = self.dispatch.consume(event)?;
    // the downstream count may come from an application's consumer, so it saturates rather than overflows
    self.validity_error_count =
      self.validity_error_count.saturating_add(constraints_count).saturating_add(downstream_count);
    Ok(())
  }

  /// If the deepest scope has not yet observed the top-level element of the document, it returns the base URI and
  /// language of the entry point as if it had been observed. If it has already been observed, it returns `None`.
  fn docroot_element_for_current_scope(&mut self) -> Option<(Option<String>, Option<String>)> {
    if let Some(include) = self.inclusions.last_mut() {
      if !include.included_root_observed {
        include.included_root_observed = true;
        return Some((include.xml_base.clone(), include.xml_lang.clone()));
      }
    }
    None
  }

  /// Returns an attribute list with the `xml:base` and `xml:lang` attributes arranged so that the base URI and
  /// language of the top-level element in the document being incorporated are preserved after incorporation.
  ///
  /// `xml_base` and `xml_lang` represent the base URI and language at the import location. If no correction is needed,
  /// return `None` (§4.5.5, §4.5.6).
  fn fixup_attributes<'a>(
    &self,
    start: &StartElementEventRef<'a>,
    xml_base: Option<&str>,
    xml_lang: Option<&str>,
  ) -> Option<Vec<Attribute>> {
    // start.base_uri and start.xml_lang represent effective values that reflect inheritance from parent elements and
    // other factors. In cases where XPointer is used, the corresponding attribute values may not necessarily be
    // present in start.attributes. To preserve the effective values after incorporation, it must explicitly be added
    // the xml:base and xml:lang attributes to existing attributes if necessary.

    // If the effective value of the base URI of the included element differs from that at the xi:include location,
    // that value is explicitly set as the xml:base attribute and retained in the resulting document. If the values are
    // the same, no adjustment is made. If there is no base URI, no adjustment is made, and the base URI of the
    // including element is inherited (§4.5.5).
    let base =
      start.base_uri.filter(|_| self.xml_base && start.base_uri != xml_base).map(|value| xml_attribute("base", value));

    // Languages are compared without distinguishing between uppercase and lowercase letters, and empty strings are
    // treated as if no language were specified. If this effective value differs from that of the importing entity,
    // explicitly set the xml:lang attribute on the imported element to preserve it. If no language is specified,
    // explicitly set xml:lang="" to prevent the language of the importing entity from being inherited (§4.5.6).
    let lang = (self.xml_lang && !same_language(start.xml_lang, xml_lang))
      .then(|| xml_attribute("lang", start.xml_lang.unwrap_or_default()));

    let replace_base = base.is_some();
    let replace_lang = lang.is_some();
    if !replace_base && !replace_lang {
      return None;
    }

    let attributes = start
      .attributes
      .into_iter()
      .filter(|attr| {
        // exclude existing XML attributes with the same name as the attributes to be corrected, and add new attributes
        // in the subsequent chain; attributes not subject to correction are left as is
        let replaced = attr.namespace == Some(XML_NS_URI)
          && match attr.local {
            "base" => replace_base,
            "lang" => replace_lang,
            _ => false,
          };
        !replaced
      })
      .map(Attribute::from)
      .chain(base)
      .chain(lang)
      .collect();

    Some(attributes)
  }

  /// Replaces the behavior of the innermost scope (how events within that scope are handled).
  fn scope_behavior(&mut self, behavior: ScopeBehavior) {
    self.inclusions.last_mut().expect("xi:include is opened").behavior = behavior;
  }

  /// Increases or decreases the depth of the innermost scope by `delta`.
  fn add_scope_depth(&mut self, delta: isize) {
    let depth = &mut self.inclusions.last_mut().expect("xi:include is opened").depth;
    if delta >= 0 {
      *depth += delta as usize;
    } else {
      *depth -= delta.unsigned_abs();
    }
  }

  /// The maximum depth of the scope.
  fn scope_depth(&self) -> usize {
    self.inclusions.last().expect("xi:include is opened").depth
  }
}

impl std::fmt::Debug for XIncludeTransformer<'_, '_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("XIncludeTransformer")
      .field("resolver", &"…")
      .field("limits", &self.process.limits)
      .field("open", &self.process.inclusions.len())
      .field("count", &self.process.inclusion_occurrence)
      .finish_non_exhaustive()
  }
}

impl<'h> EventProducer<'h> for XIncludeTransformer<'h, '_> {
  fn add_consumer(mut self, handler: &'h mut dyn EventConsumer) -> Self {
    self.process.dispatch = self.process.dispatch.add_consumer(handler);
    self
  }
}

impl EventConsumer for XIncludeTransformer<'_, '_> {
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    // only events from upstream are received here; therefore, StartDocument always signifies the beginning of a new
    // document
    if matches!(event, EventRef::StartDocument) {
      self.process.reset();
    }
    let (Flow::Continue(constraints_count) | Flow::Break(constraints_count)) = self.constraints.consume(event)?;
    self.process.receive(event)?;
    // the count also covers the events of the documents included while processing this event
    let event_validity_error_count =
      constraints_count.saturating_add(std::mem::take(&mut self.process.validity_error_count));
    Ok(if self.process.dispatch.is_stopped() {
      Flow::Break(event_validity_error_count)
    } else {
      Flow::Continue(event_validity_error_count)
    })
  }

  fn finish(&mut self, outcome: Outcome<'_>) {
    // this is the only place where the end of execution is signaled; if an error occurs during import, it is also
    // passed on to the subsequent consumer
    self.constraints.finish(outcome);
    self.process.dispatch.finish(outcome);
  }
}

impl XIncludeProcess<'_, '_> {
  /// Processes one event, whether it comes from upstream or from an included document. The number of validity errors
  /// reported downstream is accumulated in `validity_error_count`.
  fn receive(&mut self, event: &EventRef<'_>) -> Result<()> {
    // ignore these events that originate from documents included using xi:include
    if !self.inclusions.is_empty()
      && matches!(event, EventRef::StartDocument | EventRef::EndDocument | EventRef::Doctype(..))
    {
      return Ok(());
    }

    let Some(behavior) = self.inclusions.last().map(|scope| scope.behavior) else {
      return match start_element_of(event, "include") {
        Some(start) => self.include(start.into()),
        None => self.passthrough(event),
      };
    };
    match behavior {
      SeekingFallback if self.scope_depth() == 0 && start_of(event, "fallback") => {
        // since the fallback process serves as an alternative when a resource cannot be retrieved, it does not report
        // an error when the xi:include element ends
        if let Some(scope) = self.inclusions.last_mut() {
          scope.error = None;
        }
        self.scope_behavior(FallbackInProgress);
        self.add_scope_depth(1);
        Ok(())
      }
      FallbackInProgress | InclusionInProcessing if start_of(event, "include") => {
        // An xi:include element within a fallback or in an imported document. It marks the beginning of a new scope
        // and ends with its closing tag. Since neither xi:include nor xi:fallback appears downstream, they are not
        // counted toward the depth of the outer scope.
        let start = start_element_of(event, "include").expect("the event was just read as that element");
        self.include(start.into())
      }
      InclusionInProcessing => {
        // The depth counts the elements of the retrieved document itself. Whitespace outside the root element belongs
        // to the document's layout rather than being a child element, so it is not included in the count (§4.2.1).
        match event {
          EventRef::StartElement(..) => self.add_scope_depth(1),
          EventRef::EndElement(..) => self.add_scope_depth(-1),
          EventRef::Characters(..) if self.scope_depth() == 0 => return Ok(()),
          _ => {}
        }
        if let EventRef::StartElement(start) = event {
          if let Some((base, lang)) = self.docroot_element_for_current_scope() {
            if let Some(fixed_attributes) = self.fixup_attributes(start, base.as_deref(), lang.as_deref()) {
              let mut start = start.clone();
              start.attributes = Attributes::new(&fixed_attributes);
              return self.passthrough(&EventRef::StartElement(start));
            }
          }
        }
        self.passthrough(event)
      }
      FallbackInProgress => match event {
        EventRef::EndElement(..) if self.scope_depth() == 1 => {
          self.add_scope_depth(-1);
          self.scope_behavior(IgnoreAll);
          Ok(())
        }
        EventRef::StartElement(..) => {
          self.add_scope_depth(1);
          self.passthrough(event)
        }
        EventRef::EndElement(..) => {
          self.add_scope_depth(-1);
          self.passthrough(event)
        }
        _ => self.passthrough(event),
      },
      SeekingFallback | IgnoreAll => match event {
        EventRef::StartElement(..) => {
          self.add_scope_depth(1);
          Ok(())
        }
        EventRef::EndElement(..) if self.scope_depth() == 0 => {
          let error = self.inclusions.pop().and_then(|mut scope| scope.error.take());
          match error {
            // The resource errors that lack a fallback mechanism are considered fatal. In the case of an XPointer
            // error, the error is replaced by an error in the inclusion process of the resource containing the
            // affected resource.
            Some(error @ Error::XPointer { .. }) => {
              let at = error.location().clone();
              Err(Error::xinclude(format!("{} and the xi:include has no xi:fallback", error.message())).at(at))
            }
            Some(error) => Err(error),
            None => Ok(()),
          }
        }
        EventRef::EndElement(..) => {
          self.add_scope_depth(-1);
          Ok(())
        }
        _ => Ok(()),
      },
    }
  }
}

impl EventConsumer for XIncludeProcess<'_, '_> {
  /// This call occurs for events of an included document through a nested lane. The validity errors reported
  /// downstream stay in `validity_error_count`, so this returns zero for them.
  fn consume(&mut self, event: &EventRef<'_>) -> Result<Flow> {
    self.receive(event)?;
    Ok(if self.dispatch.is_stopped() { Flow::Break(0) } else { Flow::Continue(0) })
  }

  /// This call occurs when the document included via `xi:include` has finished loading; it does not mark the end of
  /// the entire execution. The end of the entire execution is signaled by `XIncludeTransformer` to the subsequent
  /// consumer.
  fn finish(&mut self, _outcome: Outcome<'_>) {}
}

/// Return the opening tag if the event is it of the `name` element in the XInclude namespace.
#[inline]
fn start_element_of<'a, 'b>(event: &'b EventRef<'a>, name: &str) -> Option<&'b StartElementEventRef<'a>> {
  event.start_element_named(Some(XINCLUDE_NS), name)
}

/// True if the event is the opening tag of the `name` element in the XInclude namespace.
#[inline]
fn start_of(event: &EventRef<'_>, name: &str) -> bool {
  event.start_element_named(Some(XINCLUDE_NS), name).is_some()
}

/// Returns the value of the attribute `name`, which has no namespace.
fn get_attribute_value<'a>(event: &'a StartElementEvent, name: &str) -> Option<&'a str> {
  event.attribute(None, name).map(|attr| attr.value.as_ref())
}

/// The location of the value of the attribute `name`, which has no namespace. If that attribute does not exist, the
/// location of the element.
fn get_attribute_value_location(event: &StartElementEvent, name: &str) -> Location {
  if let Some(attr) = event.attribute(None, name) { attr.value_location.clone() } else { event.location.clone() }
}

/// The length from the beginning of `text` to the character boundary that does not exceed `limit` bytes. If the first
/// character alone exceeds `limit`, the byte length of that single character.
fn fragmentation_length(text: &str, limit: usize) -> usize {
  match text.char_indices().find(|(at, c)| at + c.len_utf8() > limit) {
    Some((0, c)) => c.len_utf8(),
    Some((at, _)) => at,
    None => text.len(),
  }
}

/// Create an attribute in the `xml` namespace. Since the `xml` namespace is bound without a declaration, any processor
/// can read it.
fn xml_attribute(local: &str, value: &str) -> Attribute {
  Attribute {
    prefix: Some(XML_PREFIX.to_owned()),
    local: local.to_owned(),
    namespace: Some(XML_NS_URI.to_owned()),
    value: value.to_owned(),
    // the location within any entity cannot be reported since this attribute was created via import rather than from a
    // document
    location: Location::unknown(),
    value_location: Location::unknown(),
  }
}

/// True if two language tags refer to the same language. RFC 3066 tags are case-insensitive. This value is also true
/// if neither tag specifies a language.
fn same_language(one: Option<&str>, other: Option<&str>) -> bool {
  match (one, other) {
    (Some(one), Some(other)) => one.eq_ignore_ascii_case(other),
    (None, None) => true,
    _ => false,
  }
}
