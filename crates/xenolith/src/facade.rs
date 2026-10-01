//! Reads and writes XML using common configuration patterns. While Xenolith allows for the construction of processing
//! logic via an event model and pipeline-capable handlers, many applications simply require standard XML reading and
//! writing capabilities. This module acts as a facade, offering a simple, unified interface for such applications.

use std::io;

use crate::dom::build::DomBuilder;
use crate::dom::{Document, DomSource};
use crate::dtd::model::Dtd;
use crate::error::Result;
use crate::event::strict::StrictXmlValidator;
use crate::event::{Dispatch, EventCursor, EventHandler, EventSource};
use crate::io::resolve::{NoResolver, UriResolver};
use crate::io::write::{LineBreak, XmlWriter};
use crate::io::{ParserConfig, StreamSource};
use crate::name::NamePool;
use crate::xinclude::XIncludeTransform;

/// Reads XML from a file or a byte sequence in memory and either passes it to a handler as events or constructs a DOM
/// to return to the application.
///
/// The reading process operates in strict by default; therefore, in accordance with XML specifications, XML documents
/// that are not well-formed are rejected. Setting [`with_strict`](Self::with_strict) to `false` allows the application
/// to skip certain validation overhead when reading XML known to be strict. However, this does not guarantee the
/// ability to load "loose" (non-strict) XML.
///
/// # Examples
///
/// ```
/// use xenolith::Reader;
///
/// let doc = Reader::new().document("<a><b>text</b></a>".as_bytes())?;
/// assert_eq!(doc.text_content(doc.document_element().unwrap()), "text");
///
/// // A comment holding `--` is not XML: a strict read refuses it, and a read that is not strict keeps it as written.
/// let loose = "<a><!-- one -- two --></a>";
/// assert!(Reader::new().document(loose.as_bytes()).is_err());
/// let doc = Reader::new().with_strict(false).document(loose.as_bytes())?;
/// let comment = doc.first_child(doc.document_element().unwrap()).unwrap();
/// assert_eq!(doc.node_value(comment), Some(" one -- two "));
/// # Ok::<(), xenolith::Error>(())
/// ```
pub struct Reader<'r> {
  system_id: Option<String>,
  strict: bool,
  encoding: Option<String>,
  config: ParserConfig,
  /// Whether `xi:include` elements are replaced with what they name.
  xinclude: bool,
  /// The resolver every external resource is fetched through, lent by the application. A shared reference, since one
  /// read has more than one borrower: the parser, and the XInclude stage when it is on.
  resolver: &'r dyn UriResolver,
}

impl std::fmt::Debug for Reader<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // Written by hand because a resolver is the application's own type and need not be printable.
    f.debug_struct("Reader")
      .field("system_id", &self.system_id)
      .field("strict", &self.strict)
      .field("encoding", &self.encoding)
      .field("xinclude", &self.xinclude)
      .field("resolver", &"…")
      .finish_non_exhaustive()
  }
}

impl Default for Reader<'_> {
  fn default() -> Self {
    Self::new()
  }
}

impl<'r> Reader<'r> {
  /// Creates a reader that performs a strict read without using a system identifier.
  #[must_use]
  pub fn new() -> Self {
    Self {
      system_id: None,
      strict: true,
      encoding: None,
      config: ParserConfig::default(),
      xinclude: false,
      resolver: NoResolver::shared(),
    }
  }

  /// Sets the system identifier for the document read by this reader.
  ///
  /// This identifier appears in the location of any errors and serves as the base URI for resolving relative
  /// references.
  #[must_use]
  pub fn with_system_id(mut self, system_id: &str) -> Self {
    self.system_id = Some(system_id.to_owned());
    self
  }

  /// Sets whether reading is performed in *strict* (strict reading is the default).
  ///
  /// In strict reading, events pass through the [`StrictXmlValidator`](crate::event::strict::StrictXmlValidator),
  /// and XML documents that are not well-formed are rejected. In non-strict reading, only structures that the parser
  /// itself cannot read (such as unclosed elements) are rejected, though the specific cases that trigger rejection are
  /// undefined. Enabling strict reading triggers checks for issues such as names that are not valid `QName`s,
  /// duplicate attributes, comments containing `--` or ending with `-`, and processing instruction targets that are
  /// not valid `Name`s.
  #[must_use]
  pub fn with_strict(mut self, strict: bool) -> Self {
    self.strict = strict;
    self
  }

  /// Sets the input encoding. This takes precedence over any declarations or the interpretation of the actual byte
  /// sequence.
  ///
  /// The label specified here is based on parser specifications, so the acceptable values depend on the `encodings`
  /// feature. Specifying an unknown label will not result in a rejection at this stage; the error will occur when
  /// reading begins.
  #[must_use]
  pub fn with_encoding(mut self, label: &str) -> Self {
    self.encoding = Some(label.to_owned());
    self
  }

  /// Sets the parser's operational limits. Specifically, this includes settings such as restrictions to prevent
  /// excessive process memory consumption by documents and the extensions to be applied.
  ///
  /// [`Extensions::xml_id`](crate::io::Extensions) also affects the tree; specifically, since the IDs recognized by
  /// the [`document`](Self::document) are those generated by this extension, a single setting determines the behavior
  /// for both parsing and tree construction.
  #[must_use]
  pub fn with_config(mut self, config: ParserConfig) -> Self {
    self.config = config;
    self
  }

  /// Lends the resolver that every external resource is fetched through.
  ///
  /// Without one, a reference to an external entity is refused and an `xi:include` has nothing to include, which is
  /// the safe default: fetching what a document names is the XML external-entity (XXE) attack surface. The resolver is
  /// lent rather than given, so one with a cache or a catalogue in it serves every document read through this reader,
  /// and the same one serves the parser and the XInclude stage within a read.
  #[must_use]
  pub fn with_resolver(mut self, resolver: &'r dyn UriResolver) -> Self {
    self.resolver = resolver;
    self
  }

  /// Whether `xi:include` elements are replaced with what they name, which they are not by default.
  ///
  /// Inclusion happens between the parser and the handler, so what the handler is given is the assembled document. It
  /// needs a resolver: an `xi:include` in a read with none is refused, unless the element says what to do instead with
  /// an `xi:fallback`. See [`xinclude`](crate::xinclude) for what is included and what is not, and wire an
  /// [`XIncludeTransform`] by hand for the settings this flag does not reach.
  #[must_use]
  pub fn with_xinclude(mut self, on: bool) -> Self {
    self.xinclude = on;
    self
  }

  /// Reads the `input` and passes each event to the `handler`.
  ///
  /// # Errors
  ///
  /// Aborted and an error is returned if the input cannot be read or is not well-formed.
  ///
  /// # Examples
  ///
  /// The following example reads the text of each `title` element while streaming the document, without building a
  /// tree.
  ///
  /// ```
  /// use xenolith::Reader;
  /// use xenolith::event::{EventHandler, EventRef, Flow};
  ///
  /// #[derive(Default)]
  /// struct Titles {
  ///   inside: bool,
  ///   found: Vec<String>,
  /// }
  ///
  /// impl EventHandler for Titles {
  ///   fn handle(&mut self, event: &EventRef<'_>) -> xenolith::Result<Flow> {
  ///     match event {
  ///       EventRef::StartElement(start) if start.local == "title" => {
  ///         self.inside = true;
  ///         self.found.push(String::new());
  ///       }
  ///       // One run of text may arrive in several events, so each is appended to the title being read.
  ///       EventRef::Characters(text) if self.inside => {
  ///         if let Some(title) = self.found.last_mut() {
  ///           title.push_str(text.text);
  ///         }
  ///       }
  ///       EventRef::EndElement(end) if end.local == "title" => self.inside = false,
  ///       _ => {}
  ///     }
  ///     Ok(Flow::Continue(0))
  ///   }
  /// }
  ///
  /// let xml = "<library><book><title>Dune</title></book><book><title>Solaris</title></book></library>";
  /// let mut titles = Titles::default();
  /// Reader::new().events(xml.as_bytes(), &mut titles)?;
  /// assert_eq!(titles.found, ["Dune", "Solaris"]);
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  ///
  /// A transformation process can be interposed, defined by the application (one that acts as both an [`EventHandler`]
  /// and an [`EventSource`]), between the reading process and the handler. In this example, the
  /// application-implemented transformation process filters out all comments, while the underlying [`DomBuilder`]
  /// constructs the tree:
  ///
  /// ```
  /// use xenolith::Reader;
  /// use xenolith::dom::build::DomBuilder;
  /// use xenolith::event::{Dispatch, EventHandler, EventRef, EventSource, Flow, Outcome};
  ///
  /// /// Passes every event on except a comment.
  /// #[derive(Default)]
  /// struct DropComments<'h>(Dispatch<'h>);
  ///
  /// impl EventHandler for DropComments<'_> {
  ///   fn handle(&mut self, event: &EventRef<'_>) -> xenolith::Result<Flow> {
  ///     if matches!(event, EventRef::Comment(_)) { Ok(Flow::Continue(0)) } else { self.0.handle(event) }
  ///   }
  ///   fn finish(&mut self, outcome: Outcome<'_>) {
  ///     self.0.finish(outcome);
  ///   }
  /// }
  ///
  /// impl<'h> EventSource<'h> for DropComments<'h> {
  ///   fn with_handler(mut self, handler: &'h mut dyn EventHandler) -> Self {
  ///     self.0.add(handler);
  ///     self
  ///   }
  /// }
  ///
  /// let mut builder = DomBuilder::new();
  /// {
  ///   let mut transform = DropComments::default().with_handler(&mut builder);
  ///   Reader::new().events("<a>one<!-- note -->two</a>".as_bytes(), &mut transform)?;
  /// }
  /// let doc = builder.into_document();
  /// let root = doc.document_element().unwrap();
  /// assert_eq!(doc.text_content(root), "onetwo");
  /// assert_eq!(doc.children(root).count(), 1, "the comment is gone, and the text around it is one node");
  /// # Ok::<(), xenolith::Error>(())
  /// ```
  pub fn events<R: io::Read>(&self, input: R, handler: &mut dyn EventHandler) -> Result<()> {
    // Everything here is built before the source, which is given the handlers and the resolver and therefore has to be
    // dropped before them.
    //
    // Events are emitted by the parser; however, since the parser rejects invalid structures, namespaces, and
    // characters themselves, the validator is responsible only for verifying lexical rules. As the validator is
    // positioned upstream in the processing flow, any violation causes processing to halt before the events reach the
    // handler — and before the XInclude stage fetches anything for it.
    let mut strict = StrictXmlValidator::lexical_only();
    let mut xinclude = self.xinclude.then(|| {
      let mut xinclude = XIncludeTransform::new().with_config(self.config).with_strict(self.strict);
      // A handle of the one resolver, which the parser below is given as well.
      xinclude = xinclude.with_resolver(self.resolver);
      xinclude
    });

    let mut lane = Dispatch::new();

    // Strictness is evaluated before filters or transformers are invoked, and execution halts at the first sign of a
    // problem. The XInclude vocabulary is judged by the transform itself.
    if self.strict {
      lane = lane.with_validator(&mut strict);
    }

    match xinclude.as_mut() {
      Some(include) => {
        // The inclusion reports what it read to the handler, so the handler is behind it rather than beside it.
        *include = std::mem::take(include).with_handler(handler);
        lane = lane.with_handler(include);
      }
      None => lane = lane.with_handler(handler),
    }

    self.source(input, self.resolver)?.with_handler(&mut lane).emit()
  }

  /// Reads `input` into a tree.
  ///
  /// # Errors
  ///
  /// As [`events`](Self::events), and [`Dom`](crate::Error::Dom) if the events cannot form a tree.
  pub fn document<R: io::Read>(&self, input: R) -> Result<Document> {
    // `Extensions::xml_id` applies to both the parser and the document build.
    let mut builder = DomBuilder::new().with_xml_id(self.config.extensions.xml_id);

    // Since processing terminates when a DOM exception occurs, the read operation reports the exception and returns
    // the fully loaded document.
    self.events(input, &mut builder)?;
    Ok(builder.into_document())
  }

  /// A source over `input`, carrying what this reader was configured with.
  ///
  /// The resolver is passed in rather than read from the reader, because the source borrows it for as long as it lives
  /// and so must be handed a borrow the caller keeps alive.
  fn source<'h, R: io::Read>(&self, input: R, resolver: &'h dyn UriResolver) -> Result<StreamSource<'h, R>> {
    let mut source = match &self.system_id {
      Some(system_id) => StreamSource::with_system_id(input, system_id),
      None => StreamSource::new(input),
    };
    if let Some(label) = &self.encoding {
      source = source.with_encoding(label)?;
    }
    source = source.with_resolver(resolver);
    Ok(source.with_config(self.config))
  }
}

/// Writes a document as XML to any destination that implements [`io::Write`].
///
/// # What it does not write
///
/// A `Writer` writes what the tree holds and nothing more. A namespace declaration reaches the output only as an
/// attribute, which a tree a [`Reader`] built carries; an element made with a namespace and no such attribute is
/// written without a declaration. There is no indentation. The document type declaration is written only when
/// [`with_doctype`](Self::with_doctype) asks for it, since a tree keeps no parsed DTD and the walk passes that node
/// over by default.
///
/// # Examples
///
/// ```
/// use xenolith::{Reader, Writer};
///
/// let doc = Reader::new().document("<a x='1'><b>t &amp; u</b></a>".as_bytes())?;
/// let out = Writer::new().with_xml_declaration(true).write(&doc, Vec::new())?;
/// assert_eq!(
///   String::from_utf8(out).unwrap(),
///   "<?xml version=\"1.0\" encoding=\"UTF-8\"?><a x=\"1\"><b>t &amp; u</b></a>"
/// );
/// # Ok::<(), xenolith::Error>(())
/// ```
///
#[derive(Clone, Debug, Default)]
pub struct Writer {
  declaration: bool,
  standalone: Option<bool>,
  declaration_line_break: Option<LineBreak>,
  doctype: bool,
  encoding: Option<String>,
  declared_encoding: Option<String>,
}

impl Writer {
  /// Creates a writer that writes no XML declaration.
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Whether the output begins with `<?xml version="1.0" encoding="UTF-8"?>`.
  #[must_use]
  pub fn with_xml_declaration(mut self, on: bool) -> Self {
    self.declaration = on;
    self
  }

  /// Sets the `standalone` attribute of the XML declaration. If set to `None`, the attribute is omitted. The default
  /// is `None`.
  ///
  /// This attribute is included in the output only if the XML declaration is specified to be output via
  /// [`with_xml_declaration`](Self::with_xml_declaration).
  #[must_use]
  pub fn with_standalone(mut self, standalone: Option<bool>) -> Self {
    self.standalone = standalone;
    self
  }

  /// Sets the line break to be written immediately after the XML declaration. The default is `None`, meaning nothing
  /// is written (the XML declaration and the start of the root element or DOCTYPE will appear on the same line).
  ///
  /// This setting is effective only when the output of an XML declaration has been specified via
  /// [`with_xml_declaration`](Self::with_xml_declaration).
  #[must_use]
  pub fn with_declaration_line_break(mut self, line_break: Option<LineBreak>) -> Self {
    self.declaration_line_break = line_break;
    self
  }

  /// Sets the encoding used for writing bytes. The default value is UTF-8. Which labels are accepted depends on the
  /// `encodings` feature.
  #[must_use]
  pub fn with_encoding(mut self, label: &str) -> Self {
    self.encoding = Some(label.to_owned());
    self
  }

  /// Specify this setting when the encoding name declared in the XML declaration differs from the actual encoding of
  /// the byte stream. This feature is intended for cases where the reader expects an encoding name different from the
  /// one used by the writer — for example, expecting `Shift_JIS` for a byte stream written in `windows-31j`.
  #[must_use]
  pub fn with_declared_encoding(mut self, label: &str) -> Self {
    self.declared_encoding = Some(label.to_owned());
    self
  }

  /// Whether the document type (DOCTYPE) declaration held by the tree is output.
  ///
  /// Only the name and identifiers held by the tree are output (e.g., `<!DOCTYPE note SYSTEM "note.dtd">`). Since the
  /// internal subset is not output, the content of the declaration is not preserved during the process of reading and
  /// writing via the tree.
  #[must_use]
  pub fn with_doctype(mut self, on: bool) -> Self {
    self.doctype = on;
    self
  }

  /// Writes `document` to `out` and returns `out` upon completion.
  ///
  /// # Errors
  ///
  /// Any error raised by `out`, a label [`with_encoding`](Self::with_encoding) named that the writer does not have,
  /// and a character that encoding cannot hold where no character reference stands for it.
  pub fn write<W: io::Write>(&self, document: &Document, out: W) -> Result<W> {
    let mut writer = XmlWriter::new(out);
    if let Some(label) = &self.encoding {
      writer = writer.with_encoding(label)?;
    }
    if let Some(label) = &self.declared_encoding {
      writer = writer.with_declared_encoding(label);
    }
    writer = writer
      .with_declaration(self.declaration)
      .with_standalone(self.standalone)
      .with_declaration_line_break(self.declaration_line_break);
    let mut source = DomSource::new(document);
    if self.doctype {
      // The writer reads the declaration's name and identifiers from the event and nothing else, so an empty DTD is
      // enough to carry it.
      source = source.with_doctype(Dtd::default(), NamePool::new());
    }
    source.with_handler(&mut writer).emit()?;
    Ok(writer.into_inner())
  }
}
