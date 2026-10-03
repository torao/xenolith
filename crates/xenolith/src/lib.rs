//! An XML 1.0 processor library: a namespace-aware parser, a DOM tree, DTD validation, XInclude, and serialization.
//!
//! **xenolith** (/ˈksɛnəlɪθ/, or /ˈzɛnəlɪθ/) is a Pure Safe-Rust library for reading and writing XML documents. It can
//! read XML documents from byte streams, perform DTD validation and process XIncludes, construct a DOM tree, and write
//! the result back as XML text. The implementation conforms to W3C recommendations, including XML 1.0 (Fifth Edition)
//! and Namespaces in XML 1.0 (see [Specifications](#specs)), and is written entirely in Rust without using `unsafe`.
//!
//! This library likely shares some similarities with JAXP (Java API for XML Processing) because the author has been
//! working with the Java Standard XML Library for many years :)
//!
//! What you can do with this crate:
//!
//! - Read a document as an event sequence. You can either retrieve the nodes one by one (pull), as in StAX, or send
//!   them to your consumer (push), as in SAX.
//! - Build a DOM tree from the event sequence. You can also send the tree as an event sequence.
//! - Validate the document against the DTD it declares or against a DTD maintained separately as a schema.
//! - Replace `xi:include` with the content of the resource it references.
//! - Select the part of a resource an XPointer identifies.
//! - Export a tree or event sequence as well-formed XML text.
//!
//! In addition, I plan to make the following features available in the future:
//!
//! - Compiles XPath 1.0 expressions and evaluates them against the DOM tree. Includes a layer that treats the tree as
//!   an XPath 1.0 data model (seven node types, namespace nodes, and document order).
//! - `xenolith-xslt` (an extension crate built on top of this crate): Reads XSLT 1.0 stylesheets, transforms the
//!   source tree, and generates a result tree. It also provides EXSLT extension functions.
//! - `xenolith-cli` (command-line tool): Performs conversion, XPath evaluation, validation, and formatting from the
//!   command line.
//!
//! # 構成
//!
//! xenolith consists of three layers. The bottom layer contains a *parser* that performs only syntax analysis, without
//! any I/O operations. In the layer above that, the document structure parsed by the parser is considered as a
//! sequence of events (an event stream), and processing components — such as validation, transformation, tree
//! construction, and output — are assembled into a pipeline of consumers. The top-level facade ([`Reader`] and
//! [`Writer`]) pre-assembles commonly used pipelines and makes them easy to use via feature flags. Each layer is built
//! using the layers below it, and applications can use any layer depending on their needs. The following sections
//! also provide a detailed explanation of these layers.
//!
//! ## Reading and Writing with `Reader` and `Writer`
//!
//! Many applications that use XML need to read XML documents efficiently and construct a tree, then write the tree
//! back as XML, without complex procedures. [`Reader`] reads a byte sequence and constructs a DOM tree, while
//! [`Writer`] writes the tree as XML text. [`Reader`] performs strict reading (lexical validation) by default, and you
//! can specify a combination of features — such as resolvers, XInclude, input encoding, and parser settings — using
//! `with_*`. By using the functionality provided by this facade layer, you do not need to assemble the individual
//! components of the library's functionality yourself.
//!
//! The following example loads a document as a DOM tree, accesses the tree's contents, and writes them out.
//!
//! ```
//! use xenolith::{Reader, Writer};
//!
//! let doc = Reader::new().document("<order><item qty='2'>pen</item></order>".as_bytes())?;
//! let root = doc.document_element().unwrap();
//! assert_eq!(doc.text_content(root), "pen");
//!
//! let out = Writer::new().with_xml_declaration(true).write(&doc, Vec::new())?;
//! assert_eq!(
//!   String::from_utf8(out).unwrap(),
//!   "<?xml version=\"1.0\" encoding=\"UTF-8\"?><order><item qty=\"2\">pen</item></order>"
//! );
//! # Ok::<(), xenolith::Error>(())
//! ```
//!
//! ## Event Pipeline
//!
//! Xenolith's core design is an event pipeline based on document structure, and the [`Reader`] and [`Writer`] in the
//! facade layer implement a typical combination of these. By using the event pipeline directly, applications can
//! achieve higher performance and advanced scalability.
//!
//! In this layer, [`EventProducer`](event::EventProducer) generates and sends document structure events, and
//! [`EventConsumer`](event::EventConsumer) receives them. For example, [`io::StreamSource`] reads from any
//! [`std::io::Read`] to drive the parser and sends document structure events to registered consumers. This corresponds
//! to the traditional SAX-style (push-style) design.
//!
//! - **High Performance**: Since no tree (intermediate state) is constructed, operations that don't require the entire
//!   document to be kept in memory can run highly efficiently. The design is zero-copy: events borrow their names and
//!   text from the parser's buffer as `&str`. Once the buffer has been expanded to the required size, no additional
//!   memory is allocated for individual events. Consumers that have obtained the necessary information can stop reading
//!   partway through the document.
//! - **Separation of Responsibilities**: Each consumer has only one role. The parser reads only the document structure,
//!   while separate consumers handle lexical rule checking, validity verification, tree construction, and output
//!   processing. This means certain combinations are possible — for example, omitting lexical rule validation when
//!   reading a document guaranteed to be well-formed XML.
//! - **Extensibility**: You can register multiple consumers for a single producer, and each consumer receives the same
//!   event in sequence. This allows multiple operations — such as [`DomBuilder`](dom::build::DomBuilder) (tree
//!   construction) and [`XmlWriter`](io::write::XmlWriter) (XML writing) — to be accomplished in a single-pass event
//!   stream. A *transformer* that acts as both an `EventConsumer` and an `EventProducer` is placed between the producer
//!   and the consumers to modify events. For example, [`XIncludeTransformer`](xinclude::XIncludeTransformer) resolves
//!   XInclude elements in document events received from upstream and forwards the results to downstream consumers. This
//!   is independent of the components before and after it in the pipeline. Since a transformer has the same event
//!   sequence for both input and output, any number of them can be chained together, allowing you to build a pipeline
//!   as a composition of small functions. Your application can also add its own application's `EventConsumer`
//!   implementations to the same pipeline.
//!
//! The following example directly links the process from reading to writing via events, without building a tree. For
//! an example of using `XInclude`, see the [`xinclude`] module.
//!
//! ```
//! use xenolith::event::{EventCursor, EventProducer};
//! use xenolith::io::StreamSource;
//! use xenolith::io::write::XmlWriter;
//!
//! let mut writer = XmlWriter::new(Vec::new());
//! StreamSource::new("<order><item qty='2'>pen</item></order>".as_bytes()).add_consumer(&mut writer).emit()?;
//! assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<order><item qty=\"2\">pen</item></order>");
//! # Ok::<(), xenolith::Error>(())
//! ```
//!
//! ## StAX-Style Sequential Processing
//!
//! While the event pipeline sends events to consumers, you can also use the StAX-style (pull-style), in which the
//! application processes events one by one according to its own procedure.
//!
//! - **Reading**: *Event cursor*, an event source that supports the pull-type, allows you to access events one by one
//!   — the same event as pipeline — each time you call [`next`](event::EventCursor::next). This corresponds to StAX's
//!   `XMLStreamReader`.
//! - **Writing**: Each time a `write_*` method of [`WriterSource`](io::write::WriterSource) is called, the
//!   corresponding event is sent to the consumers. By combining this with [`XmlWriter`](io::write::XmlWriter), you can
//!   achieve XML writing equivalent to StAX's `XMLStreamWriter`.
//!
//! The same events reach the registered consumers because both push- and pull-style build on the event pipeline
//! mechanism. For example, if you place [`StrictXmlConstraints`](event::strict::StrictXmlConstraints) in the
//! preprocessing stage before [`XmlWriter`](io::write::XmlWriter), you can reject structures or names that
//! unintentionally violate XML rules before they are written to a file.
//!
//! ```
//! use xenolith::event::strict::StrictXmlConstraints;
//! use xenolith::event::{Dispatcher, EventCursor, EventRef, EventProducer};
//! use xenolith::io::StreamSource;
//! use xenolith::io::write::{WriterSource, XmlWriter};
//!
//! // Reading: Extract the names of the opening tags one by one.
//! let mut source = StreamSource::new("<order><item>pen</item></order>".as_bytes());
//! let mut names = Vec::new();
//! while let Some(event) = source.next()? {
//!   if let EventRef::StartElement(start) = event {
//!     names.push(start.local.to_owned());
//!   }
//! }
//! assert_eq!(names, ["order", "item"]);
//!
//! // Writing: Write while checking for lexical rules.
//! let mut strict = StrictXmlConstraints::new();
//! let mut writer = XmlWriter::new(Vec::new());
//! {
//!   let mut lane = Dispatcher::new().add_consumer(&mut strict).add_consumer(&mut writer);
//!   let mut doc = WriterSource::new().add_consumer(&mut lane);
//!   doc.write_start_element("order")?;
//!   doc.write_characters("pen")?;
//!   doc.write_end_element()?;
//!   doc.end_document()?;
//! }
//! assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<order>pen</order>");
//! # Ok::<(), xenolith::Error>(())
//! ```
//!
//! ## XML Parser
//!
//! Under [`io::StreamSource`] in the event pipeline layer, the [`io::Parser`] — which follows a *sans-I/O* design —
//! parses XML. The caller feeds fragments of the byte sequence read from the byte stream to the parser using
//! [`feed`](io::Parser::feed) to retrieve the next event. Even when external entities are required, the parser does
//! not read them itself; it asks the caller to retrieve them. This lets the XML parser pause and resume processing at
//! token boundaries without monopolizing the thread in either a blocking I/O or asynchronous I/O environment, making
//! it adaptable to input sources such as files, sockets, and in-memory byte sequences. [`io::StreamSource`] and
//! `AsyncReader` from the `async` feature add actual I/O on top of this parser.
//!
//! ## DOM Tree
//!
//! The [`dom`] (Document Object Model) tree follows the structure defined in DOM Level 3 Core. This is an *arena*
//! structure in which [`Document`](dom::Document) owns all nodes, and nodes are referenced via [`NodeId`](dom::NodeId),
//! which has a `Copy` property. In Rust, in particular, since there are no reference counts or borrow relationships
//! between nodes, the tree can be modified while being traversed. Strings that frequently appear in the document, such
//! as element and attribute names, are stored in the document's [`NamePool`], and nodes refer to them via integers.
//! You can connect it anywhere in the event pipeline as an intermediate state because the tree is built from events
//! using a [`DomBuilder`](dom::build::DomBuilder) and can be sent back out as events via a
//! [`DomSource`](dom::DomSource).
//!
//! # Security
//!
//! Xenolith is designed to read untrusted XML documents. While the following guidelines are effective against many
//! attacks and unintended incidents, they represent design decisions and do not guarantee the application's security
//! against all threats.
//!
//! - **Retrieving External Resources**: Xenolith does not proactively load resources from URIs referenced in
//!   documents. It uses only authorized byte streams provided through a resolver supplied by the application (CWE-611,
//!   CWE-918). See below.
//! - **Setting Resource Consumption Limits**: By default, there are limits on token length (CWE-770), the depth,
//!   frequency, and character count of entity expansions (CWE-776), the nesting depth of elements (CWE-674), the
//!   depth and frequency of XInclude inclusions, and the characters an XPointer keeps pending while it tries its parts
//!   in order. To remove these limits for trusted imports, explicit configuration is required. See below.
//! - **Non-Panic Loading**: Any errors that occur while loading or parsing documents are returned as `Err` rather than
//!   causing a panic. This has also been verified through fuzzing.
//! - **Pure Safe Rust**: The xenolith codebase does not use `unsafe`, and the compiler guarantees memory safety.
//! - **Low Dependency**: Excessive reliance on external crates can create a vector for supply chain attacks. By
//!   default, the only crates this library depends on are [`encoding_rs`] for character encoding and `thiserror`,
//!   which is used only during the build process.
//!
//! If URIs referenced by untrusted XML documents are retrieved as-is, this can lead to XXE (XML External Entity)
//! attacks — where files on the server are read — or SSRF (Server-Side Request Forgery) attacks, in which requests are
//! sent to the internal network. It is the application's responsibility to determine which URIs are safe to retrieve,
//! and the resolver is the component that implements that determination. This crate does not retrieve external
//! resources unless the caller specifies how to retrieve them. External entities, external DTD subsets, and
//! `xi:include` resources are only allowed to be loaded through the [`UriResolver`](io::resolve::UriResolver)
//! configured by the caller.
//!
//! There are limits on the expansion of entities and the inclusion of `XInclude` files, regardless of the document
//! declaration ([`io::Limits`], [`xinclude::Limits`]). To remove these limits, explicitly specify
//! [`io::Limits::unlimited`] or [`xinclude::Limits::unlimited`].
//!
//! # Modules
//!
//! - [`event`]: Vocabulary related to the event pipeline: [`EventRef`](event::EventRef),
//!   [`EventConsumer`](event::EventConsumer), [`EventProducer`](event::EventProducer), strict validation, and the
//!   [`Validator`] contract implemented by the schema.
//! - [`io`]: Input/output and XML parser processing. Includes decoding, character streams for each entity, XML
//!   declarations and text declarations, entity resolution, and serialization ([`io::write`]).
//! - [`dtd`]: A model that stores declarations as data ([`dtd::model`]), reads a `DOCTYPE` or a standalone DTD
//!   ([`dtd::read`]), and validates based on a DTD ([`dtd::validate`]).
//! - [`dom`]: A tree with an arena structure that implements [DOM Level 3 Core]. It is constructed using
//!   [`dom::build`] and written to events using [`DomSource`](dom::DomSource).
//! - [`xinclude`]: The `XInclude` 1.0 transformer ([`XIncludeTransformer`](xinclude::XIncludeTransformer)) and the
//!   schema for its vocabulary ([`XIncludeSchema`](xinclude::XIncludeSchema)).
//! - [`xpointer`]: A parsed XPointer ([`XPointer`](xpointer::XPointer)) and a filter
//!   ([`XPointerFilter`](xpointer::XPointerFilter)) that passes through the part it identifies.
//!
//! The fundamental elements they share:
//!
//! - [`error`][]: An error and its location within the entity.
//! - [`chars`]: Character classes and name formation rules from [XML 1.0 (Fifth Edition)].
//! - [`name`]: The interned name, [`QName`], and [`ExpandedName`] as determined by the string pool.
//! - [`attr`][]: Attributes independent of how they were generated, and views of them.
//! - [`uri`]: References and resolution based on RFC 3986. Forms the basis for handling base URIs.
//!
//! # Feature flags
//!
//! Each feature allows you to choose whether to include external libraries, other than the core XML functionality, in
//! the compilation.
//!
//! - `encodings` (default): Supports the character encodings corresponding to [`encoding_rs`] in XML documents. If not
//!   specified, only Xenolith's built-in UTF-8, UTF-16, US-ASCII, and ISO-8859-1 are available. Any other encodings
//!   will result in a [`Error::UnsupportedFeature`] error.
//! - `async`: Enables a runtime-independent `AsyncReader` based on `futures_io::AsyncRead`. The application drives
//!   this using any executor, such as `tokio`.
//!
//! # Specification
//!
//! Xenolith was implemented based on the following specifications. The dates listed are the last update dates of each
//! document as of the time of reference.
//!
//! - [XML 1.0 (Fifth Edition)]: W3C Recommendation (November 26, 2008). The entirety of [`io`] and [`dtd`] (documents,
//!   DTDs, entities, and well-formedness constraints), the character classes and production rules of [`chars`], and
//!   [`io::encoding`] implement the rules for determining document encoding specified in §4.3.3.
//! - [Namespaces in XML 1.0 (Third Edition)]: W3C Recommendation (December 8, 2009). Prefix resolution and namespace
//!   constraints. [`name`] is a `QName`, a prefix, and a model for expanded names.
//! - [XML Base (Second Edition)]: W3C Recommendation (January 28, 2009). Calculating the base URI for each node based
//!   on `xml:base` and entity system identifiers. See [`Parser::base_uri`](io::Parser::base_uri).
//! - [xml:id 1.0]: W3C Recommendation (September 9, 2005). `xml:id` as an ID-type attribute with token normalization.
//!   See [`Parser::xml_id`](io::Parser::xml_id).
//! - [XInclude 1.0 (Second Edition)]: W3C Recommendation (November 15, 2006). The processing of [`xinclude`]
//!   inclusions and the constraints on the vocabulary of [`XIncludeSchema`](xinclude::XIncludeSchema).
//! - [XPointer Framework], [XPointer `element()` Scheme] and [XPointer `xmlns()` Scheme]: W3C Recommendations (March
//!   25, 2003). The pointers [`xpointer`] parses and selects by, and the `xpointer` attribute of [`xinclude`].
//! - [DOM Level 3 Core]: W3C Recommendation (April 7, 2004). The structure indicated by [`dom`].
//! - [RFC 3986]: Uniform Resource Identifier (URI): Generic Syntax (January 2005). [`uri`] is resolved according to
//!   the reference resolution described in §5.3.
//!
//! The parser is validated using the [W3C XML Conformance Test Suite]. Please refer to the developer guide for
//! instructions on how to run the tests.
//!
//! [XML 1.0 (Fifth Edition)]: https://www.w3.org/TR/2008/REC-xml-20081126/
//! [Namespaces in XML 1.0 (Third Edition)]: https://www.w3.org/TR/2009/REC-xml-names-20091208/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/
//! [xml:id 1.0]: https://www.w3.org/TR/2005/REC-xml-id-20050909/
//! [XInclude 1.0 (Second Edition)]: https://www.w3.org/TR/2006/REC-xinclude-20061115/
//! [XPointer Framework]: https://www.w3.org/TR/2003/REC-xptr-framework-20030325/
//! [XPointer `element()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-element-20030325/
//! [XPointer `xmlns()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/
//! [DOM Level 3 Core]: https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/
//! [RFC 3986]: https://www.rfc-editor.org/rfc/rfc3986
//! [W3C XML Conformance Test Suite]: https://www.w3.org/XML/Test/
//! [`encoding_rs`]: https://docs.rs/encoding_rs
//!

pub mod _design;
pub mod attr;
pub mod chars;
pub mod dom;
pub mod dtd;
pub mod error;
pub mod event;
pub mod io;
pub mod name;
pub mod uri;
pub mod xinclude;
pub mod xpointer;

mod facade;

pub use attr::{Attribute, AttributeList, AttributeRef, Attributes};
pub use error::{Error, Location, Result};
pub use event::validate::{Schema, Validator, ValidityError};
pub use facade::{Reader, Writer};
pub use name::{ExpandedName, NameId, NamePool, QName, XML_NS_URI, XMLNS_NS_URI};
pub use uri::UriReference;
