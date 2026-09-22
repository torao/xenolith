This library is written for people who already know Java's XML APIs. Where a Java name carries
meaning, it is kept — `XPathExpression` is still the compiled expression, `Transformer` still
holds a stylesheet and its parameters — so what you know transfers. Where a Java name describes
a Java problem, it is gone: there are no factories, no service lookup, and no listener to
register.

Every Rust example below is compiled and run as part of the test suite, so what is written here
is what the code does.

## Where things live

| Java | Here |
|---|---|
| `javax.xml.parsers.DocumentBuilder` | [`dom::build::DomBuilder`](xenolith_core::dom::build::DomBuilder), driven by [`parser::StreamSource`](xenolith_core::io::StreamSource) |
| `org.w3c.dom.Document`, `Node`, `Element` | [`dom::Document`](xenolith_core::dom::Document) and a `Copy` [`NodeId`](xenolith_core::dom::NodeId) |
| `org.w3c.dom.DOMException` | [`dom::DomException`](xenolith_core::dom::DomException) |
| `javax.xml.stream.XMLStreamReader` (StAX) | [`parser::StreamSource`](xenolith_core::io::StreamSource) |
| `org.xml.sax.ContentHandler` + `SAXParser` | [`event::EventHandler`](xenolith_core::event::EventHandler) + [`EventCursor::emit`](xenolith_core::event::EventCursor::emit) |
| `org.xml.sax.EntityResolver` | [`parser::resolve::UriResolver`](xenolith_core::io::resolve::UriResolver) |
| `org.xml.sax.ErrorHandler` | the `Result` of [`EventCursor::emit`](xenolith_core::event::EventCursor::emit); a validity violation is a [`ValidityError`](xenolith_core::event::validate::ValidityError) in a report |
| `org.xml.sax.ext.LexicalHandler` | the `Comment`, `Cdata` and `Doctype` events |
| `org.xml.sax.DTDHandler`, `ext.DeclHandler` | the parsed [`Dtd`](xenolith_core::dtd::Dtd) the `Doctype` event carries |
| `setValidating(true)`, `javax.xml.validation` | [`ValidatorSet`](xenolith_core::event::validate::ValidatorSet), [`Validator`](xenolith_core::event::validate::Validator) |
| `javax.xml.stream.XMLStreamWriter` | [`io::write::WriterSource`](xenolith_core::io::write::WriterSource) + [`io::write::XmlWriter`](xenolith_core::io::write::XmlWriter) |
| `LSSerializer` / `Transformer` used to print a tree | [`Writer`](xenolith_core::Writer), or [`DomSource`](xenolith_core::dom::DomSource) + [`io::write::XmlWriter`](xenolith_core::io::write::XmlWriter) |
| `XPathFactory.newInstance().newXPath()` | [`xpath::XPath::new`](xenolith_xpath::XPath::new) |
| `NamespaceContext` | [`XPath::with_namespace`](xenolith_xpath::XPath::with_namespace) |
| `XPathVariableResolver` | [`xpath::Variables`](xenolith_xpath::Variables) |
| `XPathFunctionResolver` / `XPathFunction` | [`xpath::Functions`](xenolith_xpath::Functions) / [`Function`](xenolith_xpath::Function) |
| `TransformerFactory` / `Transformer` | [`transform::Transformer`](crate::transform::Transformer) |
| `StreamSource` / `DOMSource` | [`transform::Source::bytes`](crate::transform::Source::bytes) / [`Source::document`](crate::transform::Source::document) |
| `URIResolver` | [`transform::Resolver`](crate::transform::Resolver) (which is [`xslt::Loader`](xenolith_xslt::Loader)) |
| `Result` (`StreamResult`, `DOMResult`) | the [`Transformed`](crate::transform::Transformed) the call returns |
| `ErrorListener` | gone — see [below](#what-is-deliberately-different) |
| `xalan` / `xsltproc` on the command line | the `xenolith` binary |

## Reading a document into a tree

```java
DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
factory.setNamespaceAware(true);
Document doc = factory.newDocumentBuilder().parse(new ByteArrayInputStream(xml));
Element root = doc.getDocumentElement();
String title = root.getFirstChild().getTextContent();
```

```rust
use xenolith::dom::build::DomBuilder;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;

let mut builder = DomBuilder::new();
StreamSource::new("<book><title>Dune</title></book>".as_bytes()).with_handler(&mut builder).emit()?;
let doc = builder.into_document().map_err(xenolith::Error::internal)?;
let root = doc.document_element().unwrap();
let title = doc.node(root).first_child().unwrap();

assert_eq!(title.node_name(), "title");
assert_eq!(title.text_content(), "Dune");
# Ok::<(), xenolith::Error>(())
```

There is no factory and no `setNamespaceAware`: parsing is namespace-aware always, because a
parser that is not is a source of bugs rather than a configuration choice.

A node is a `Copy` index into the document's arena rather than an object holding a pointer back
to its parent, so it needs the document to be read: `doc.node_name(id)`, or
`doc.node(id).node_name()` for a chained walk. Mutating the tree needs `&mut Document` — unique
access — which is what rules out the aliasing bugs `Node` objects invite.

## Reading a document as a stream

`XMLStreamReader`'s loop, with `int` event codes replaced by an enum and the `next()`/`hasNext()`
pair by one call that yields `None` at the end:

```java
XMLStreamReader reader = XMLInputFactory.newInstance().createXMLStreamReader(in);
StringBuilder text = new StringBuilder();
while (reader.hasNext()) {
  if (reader.next() == XMLStreamConstants.CHARACTERS) {
    text.append(reader.getText());
  }
}
```

```rust
use xenolith::io::{TokenKind, StreamSource};

let mut reader = StreamSource::new("<doc>one<i>two</i></doc>".as_bytes());
let mut text = String::new();
while let Some(kind) = reader.advance()? {
  if kind == TokenKind::Text {
    if let Some(chars) = reader.parser().token_ref().and_then(|e| e.text()) {
      text.push_str(chars);
    }
  }
}
assert_eq!(text, "onetwo");
# Ok::<(), xenolith::Error>(())
```

The accessors borrow the parser's buffers, so an event is readable until the next `advance` and
costs nothing per event. An event that has to outlive the call is copied by the handler that
receives it, which keeps only the parts it needs.

## Reading a document as a push of events

The parser itself is pull: you ask for the next event. Push is the same parser with the asking
done for you — [`emit`](xenolith_core::event::EventCursor::emit) runs a source to the end and
hands each event to an `EventHandler`. Both shapes are one parser, so it is a choice of shape,
not of capability. Note that this is SAX-*style* push, not an implementation of the
[SAX API](http://www.saxproject.org/): of the SAX handlers, only `ContentHandler` has a
counterpart of its own, and the rest are answered by data you can query.

`ContentHandler`, without the ceremony of `DefaultHandler` — every method already does nothing,
so a handler overrides what it cares about:

```java
class Titles extends DefaultHandler {
  boolean inTitle;
  final List<String> found = new ArrayList<>();
  public void startElement(String uri, String local, String qName, Attributes atts) {
    inTitle = local.equals("title");
  }
  public void characters(char[] ch, int start, int length) {
    if (inTitle) found.add(new String(ch, start, length));
  }
}
```

```rust
use xenolith::io::StreamSource;
use xenolith::event::{EventRef, EventCursor, EventHandler, EventSource};

#[derive(Default)]
struct Titles {
  in_title: bool,
  found: Vec<String>,
}

impl EventHandler for Titles {
  fn handle(&mut self, event: &EventRef<'_>) -> xenolith::Result<()> {
    match event {
      EventRef::StartElement(event) => self.in_title = event.local == "title",
      EventRef::Characters(event) if self.in_title => self.found.push(event.text.to_owned()),
      _ => {}
    }
    Ok(())
  }
}

let mut reader = StreamSource::new("<books><title>Dune</title><title>Emma</title></books>".as_bytes());
let mut titles = Titles::default();
reader.with_handler(&mut titles).emit()?;

assert_eq!(titles.found, ["Dune", "Emma"]);
# Ok::<(), xenolith::Error>(())
```

### `EntityResolver`

Resolving an external entity is a reader's concern, not a content callback: implement
`UriResolver` and hand it to the source with `with_resolver`. Nothing is resolved until you do,
since fetching what a document names is the XML external-entity (XXE) attack surface.

### `ErrorHandler`

There is no handler to register and no severity to inspect: the line SAX draws between a fatal
violation and a recoverable one falls out of the types. A well-formedness violation is an `Err`
from `emit` and the run stops; a validity violation is recorded as a `ValidityError` that a
`Validator` keeps while the run goes on. A handler has the same channel: returning `Err` from
`handle` refuses the document, and that error is what `emit` returns. Where nothing is wrong and
the handler simply has all it wanted, it returns `false` from `should_continue` instead, and the
run ends successfully.

### `DTDHandler` and `DeclHandler`

Notations, unparsed entities, and the element, attribute and entity declarations are not pushed
one at a time. The parser reads the whole DTD into a `Dtd` and hands it to the `Doctype` event,
which it reports once the `DOCTYPE` and both subsets are read — so the DTD is complete when it
arrives, and a handler that wanted only the DTD can stop there:

```rust
fn should_continue(&self) -> bool {
  !self.done // set on the Doctype event; the document body is never read
}
```

## XPath

```java
XPath xpath = XPathFactory.newInstance().newXPath();
XPathExpression expr = xpath.compile("count(//item)");
double n = (Double) expr.evaluate(doc, XPathConstants.NUMBER);
```

```rust
use xenolith::dom::build::DomBuilder;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;
use xenolith::xdm::{DomModel, Model};
use xenolith::xpath::XPath;

let mut builder = DomBuilder::new();
StreamSource::new("<list><item>a</item><item>b</item></list>".as_bytes()).with_handler(&mut builder).emit()?;
let doc = builder.into_document().map_err(xenolith::Error::internal)?;
let model = DomModel::new(&doc);

let expression = XPath::new().compile("count(//item)")?;
let value = expression.evaluate(&model, model.root_node())?;

assert_eq!(value.number(&model), 2.0);
# Ok::<(), xenolith::Error>(())
```

Two differences worth knowing. XPath does not run over the DOM directly but over the
[data model](xenolith_xdm) — the seven node kinds, merged text, synthesized namespace nodes —
which `DomModel` presents over a borrowed `Document` without changing it. And there is no
`XPathConstants`: the [`Value`](xenolith_xpath::Value) that comes back *is* one of the four
types, and `boolean()`, `number()`, `string()` and `nodes()` convert it exactly as XPath §4 says,
so an expression yielding the wrong type is not a `ClassCastException` at run time.

Binding a prefix replaces `NamespaceContext`, one binding at a time:

```rust
use xenolith::dom::build::DomBuilder;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;
use xenolith::xdm::{DomModel, Model};
use xenolith::xpath::XPath;

let mut builder = DomBuilder::new();
StreamSource::new("<r xmlns:d='urn:d'><d:a>found</d:a></r>".as_bytes()).with_handler(&mut builder).emit()?;
let doc = builder.into_document().map_err(xenolith::Error::internal)?;
let model = DomModel::new(&doc);

// The document's prefix and the expression's need not agree — only the namespace does.
let expression = XPath::new().with_namespace("x", "urn:d").compile("//x:a")?;
let value = expression.evaluate(&model, model.root_node())?;

assert_eq!(value.string(&model), "found");
# Ok::<(), xenolith::Error>(())
```

## XSLT

```java
TransformerFactory factory = TransformerFactory.newInstance();
Transformer transformer = factory.newTransformer(new StreamSource(stylesheet));
transformer.setParameter("greeting", "Good day");
StringWriter out = new StringWriter();
transformer.transform(new StreamSource(document), new StreamResult(out));
```

```rust
use xenolith::transform::{Source, Transformer};

let stylesheet = br#"<xsl:stylesheet version="1.0"
    xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="text"/>
  <xsl:param name="greeting">Hello</xsl:param>
  <xsl:template match="/">
    <xsl:value-of select="concat($greeting, ', ', /doc/name)"/>
  </xsl:template>
</xsl:stylesheet>"#;

let transformer = Transformer::compile(Source::bytes(stylesheet))?.with_parameter("greeting", "Good day");
let result = transformer.transform(Source::bytes(b"<doc><name>world</name></doc>"))?;

assert_eq!(result.text(), "Good day, world");
# Ok::<(), xenolith::Error>(())
```

A `Transformer` is compiled once and used over as many documents as you like, as in Java — but
unlike Java it is not mutated between runs, so there is no question of whether it is safe to
share one. `setParameter` becomes `with_parameter`, which consumes and returns the transformer.

What `xsl:message` wrote is on the result: `result.messages()`. Nothing has to be registered to
see it.

Where the stylesheet's own modules and the trees `document()` names come from is
[`with_resolver`](crate::transform::Transformer::with_resolver), Java's `URIResolver`. Its
default serves nothing: a transformation reads no more than it was handed.

## Validating

```java
DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
factory.setValidating(true);
factory.newDocumentBuilder().parse(in); // errors reach the ErrorHandler
```

```rust
use xenolith::event::validate::ValidatorSet;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;

let xml = "<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>";
let mut validation = ValidatorSet::new().validating_dtd(true);
StreamSource::new(xml.as_bytes()).with_handler(&mut validation).emit()?;
let report = validation.report();

// Two violations, not one: `c` is not declared, and `a` was declared to hold a `b`.
assert!(!report.is_valid());
assert!(report.errors().iter().any(|error| error.message().contains("c")));
# Ok::<(), xenolith::Error>(())
```

The distinction Java draws between a fatal error and a recoverable one is kept, but it is in the
types rather than in which `ErrorHandler` method is called: a well-formedness error is the `Err`
of the call and stops the parse, a validity error is collected into the
[`Report`](xenolith_core::event::validate::Report) and the document is read to the end. A document with no
`DOCTYPE` is neither valid nor invalid — `had_dtd()` says which case you are in, so "nothing to
check against" cannot be mistaken for a pass.

## Writing XML

From a tree, the way `LSSerializer` or an identity `Transformer` is used in Java. A walk over the tree reports it as
events, and the writer writes them:

```rust
use xenolith::dom::DomSource;
use xenolith::dom::build::DomBuilder;
use xenolith::event::{EventCursor, EventSource};
use xenolith::io::StreamSource;
use xenolith::io::write::XmlWriter;

let mut builder = DomBuilder::new();
StreamSource::new("<r><a><b>x</b></a></r>".as_bytes()).with_handler(&mut builder).emit()?;
let doc = builder.into_document().map_err(xenolith::Error::internal)?;

let mut writer = XmlWriter::new(Vec::new());
DomSource::new(&doc).with_handler(&mut writer).emit()?;
assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<r><a><b>x</b></a></r>");
# Ok::<(), xenolith::Error>(())
```

Writing without a tree, the way `XMLStreamWriter` is used: the calls go to a `WriterSource`, and the handlers installed
on it decide where they land — an `XmlWriter` for XML text, a `DomBuilder` for a tree, a validator to check them on the
way.

```rust
use xenolith::event::EventSource;
use xenolith::io::write::{WriterSource, XmlWriter};

let mut writer = XmlWriter::new(Vec::new());
{
  let mut doc = WriterSource::new().with_handler(&mut writer);
  doc.write_start_element("r")?;
  doc.write_attribute("x", "1");
  doc.write_characters("t & u")?;
  doc.write_end_element()?;
  doc.end_document()?;
}
assert_eq!(String::from_utf8(writer.into_inner()).unwrap(), "<r x=\"1\">t &amp; u</r>");
# Ok::<(), xenolith::Error>(())
```

Call by call, as `XMLStreamWriter`:

```rust
use xenolith::io::write::XmlWriter;

let mut out = Vec::new();
let mut writer = XmlWriter::new(&mut out);
writer.write_start_element("note")?;
writer.write_attribute("lang", "en")?;
writer.write_characters("hi & bye")?;
writer.write_end_element()?;

assert_eq!(String::from_utf8(out).unwrap(), "<note lang=\"en\">hi &amp; bye</note>");
# Ok::<(), xenolith::Error>(())
```

## What is deliberately different

**No factories, no service lookup.** `DocumentBuilderFactory.newInstance()` exists to let a
system property swap the implementation underneath you. Nothing here is chosen at run time by a
property file on the classpath: you call the function you meant.

**Builders instead of setters.** A configured object is built by consuming methods —
`Transformer::compile(…)?.with_parameter(…)` — rather than by mutating a shared one. What
follows is that a value handed to you cannot be reconfigured behind your back.

**A `Result` instead of an exception.** Every call that can fail says so in its type, and the
error carries the location it happened at. There is no unchecked
`TransformerFactoryConfigurationError` to discover in production.

**No `ErrorListener` and no `ErrorHandler` to register.** Anything fatal is the `Err` of the
call, and what `xsl:message` said comes back beside the result. The "forgot to register the
listener, lost the diagnostics" path does not exist. Validation is no exception: a
[`Validator`](xenolith_core::event::validate::Validator) keeps each recoverable violation itself and hands
them back from [`errors`](xenolith_core::event::validate::Validator::errors), and one that would rather stop
at the first refuses the document with `Err` like any other handler.

**Nothing is fetched unless you say how.** Java resolves external entities by default, which is
why every hardening guide begins by turning that off; here a parser with no
[`UriResolver`](xenolith_core::io::resolve::UriResolver) fetches nothing, and so does a
transformation with no resolver. XXE is not a setting you have to remember.

**An arena, not a graph of objects.** `NodeId` is a `Copy` index; document order is an integer
comparison; there are no parent pointers to leak through. The cost is that reading a node needs
the document in hand.

## Not here

XSLT 2.0 and 3.0, XQuery, XML Schema and RELAX NG validation, and the DOM's optional modules
(Load & Save, Traversal, Ranges) are not implemented. Within XSLT 1.0 and EXSLT, what is missing
is listed in `ROADMAP.md` rather than silently skipped — an instruction this engine does not
carry out is reported as such, and `element-available()` and `function-available()` answer from
what was actually built.
