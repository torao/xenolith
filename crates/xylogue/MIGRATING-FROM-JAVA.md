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
| `javax.xml.parsers.DocumentBuilder` | [`dom::build::parse`](xylogue_dom::build::parse) |
| `org.w3c.dom.Document`, `Node`, `Element` | [`dom::Document`](xylogue_dom::Document) and a `Copy` [`NodeId`](xylogue_dom::NodeId) |
| `org.w3c.dom.DOMException` | [`dom::DomException`](xylogue_dom::DomException) |
| `javax.xml.stream.XMLStreamReader` (StAX) | [`parser::Reader`](xylogue_parser::Reader) |
| `org.xml.sax.ContentHandler` + `SAXParser` | [`parser::sax::Handler`](xylogue_parser::sax::Handler) + [`sax::parse`](xylogue_parser::sax::parse) |
| `org.xml.sax.EntityResolver` | [`parser::resolve::UriResolver`](xylogue_parser::resolve::UriResolver) |
| `setValidating(true)`, `javax.xml.validation` | [`validate::validate`](xylogue_validate::validate), [`Validator`](xylogue_validate::Validator) |
| `javax.xml.stream.XMLStreamWriter` | [`serialize::XmlWriter`](xylogue_serialize::XmlWriter) |
| `LSSerializer` / `Transformer` used to print a tree | [`serialize::Serializer`](xylogue_serialize::Serializer) |
| `XPathFactory.newInstance().newXPath()` | [`xpath::XPath::new`](xylogue_xpath::XPath::new) |
| `NamespaceContext` | [`XPath::with_namespace`](xylogue_xpath::XPath::with_namespace) |
| `XPathVariableResolver` | [`xpath::Variables`](xylogue_xpath::Variables) |
| `XPathFunctionResolver` / `XPathFunction` | [`xpath::Functions`](xylogue_xpath::Functions) / [`Function`](xylogue_xpath::Function) |
| `TransformerFactory` / `Transformer` | [`transform::Transformer`](crate::transform::Transformer) |
| `StreamSource` / `DOMSource` | [`transform::Source::bytes`](crate::transform::Source::bytes) / [`Source::document`](crate::transform::Source::document) |
| `URIResolver` | [`transform::Resolver`](crate::transform::Resolver) (which is [`xslt::Loader`](xylogue_xslt::Loader)) |
| `Result` (`StreamResult`, `DOMResult`) | the [`Transformed`](crate::transform::Transformed) the call returns |
| `ErrorListener` | gone — see [below](#what-is-deliberately-different) |
| `xalan` / `xsltproc` on the command line | the `xylogue` binary |

## Reading a document into a tree

```java
DocumentBuilderFactory factory = DocumentBuilderFactory.newInstance();
factory.setNamespaceAware(true);
Document doc = factory.newDocumentBuilder().parse(new ByteArrayInputStream(xml));
Element root = doc.getDocumentElement();
String title = root.getFirstChild().getTextContent();
```

```rust
use xylogue::dom::build;

let doc = build::parse("<book><title>Dune</title></book>".as_bytes())?;
let root = doc.document_element().unwrap();
let title = doc.node(root).first_child().unwrap();

assert_eq!(title.node_name(), "title");
assert_eq!(title.text_content(), "Dune");
# Ok::<(), xylogue::Error>(())
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
use xylogue::parser::{EventKind, Reader};

let mut reader = Reader::new("<doc>one<i>two</i></doc>".as_bytes());
let mut text = String::new();
while let Some(kind) = reader.advance()? {
  if kind == EventKind::Text {
    if let Some(chars) = reader.parser().event_ref().and_then(|e| e.text()) {
      text.push_str(chars);
    }
  }
}
assert_eq!(text, "onetwo");
# Ok::<(), xylogue::Error>(())
```

The accessors borrow the parser's buffers, so an event is readable until the next `advance` and
costs nothing per event. When one has to outlive the call, `Event::capture` copies it and
`Reader::events` gives an iterator of owned events.

## Reading a document as a push of events

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
use std::convert::Infallible;

use xylogue::parser::sax::{CharactersEvent, Handler, StartElementEvent, parse};
use xylogue::parser::Reader;

#[derive(Default)]
struct Titles {
  in_title: bool,
  found: Vec<String>,
}

impl Handler for Titles {
  type Error = Infallible;
  fn start_element(&mut self, event: StartElementEvent<'_>) -> Result<(), Infallible> {
    self.in_title = event.pool.resolve(event.name.local()) == "title";
    Ok(())
  }
  fn characters(&mut self, event: CharactersEvent<'_>) -> Result<(), Infallible> {
    if self.in_title {
      self.found.push(event.text.to_owned());
    }
    Ok(())
  }
}

let mut reader = Reader::new("<books><title>Dune</title><title>Emma</title></books>".as_bytes());
let mut titles = Titles::default();
parse(&mut reader, &mut titles)?;

assert_eq!(titles.found, ["Dune", "Emma"]);
# Ok::<(), xylogue::Error>(())
```

## XPath

```java
XPath xpath = XPathFactory.newInstance().newXPath();
XPathExpression expr = xpath.compile("count(//item)");
double n = (Double) expr.evaluate(doc, XPathConstants.NUMBER);
```

```rust
use xylogue::dom::build;
use xylogue::xdm::{DomModel, Model};
use xylogue::xpath::XPath;

let doc = build::parse("<list><item>a</item><item>b</item></list>".as_bytes())?;
let model = DomModel::new(&doc);

let expression = XPath::new().compile("count(//item)")?;
let value = expression.evaluate(&model, model.root_node())?;

assert_eq!(value.number(&model), 2.0);
# Ok::<(), xylogue::Error>(())
```

Two differences worth knowing. XPath does not run over the DOM directly but over the
[data model](xylogue_xdm) — the seven node kinds, merged text, synthesized namespace nodes —
which `DomModel` presents over a borrowed `Document` without changing it. And there is no
`XPathConstants`: the [`Value`](xylogue_xpath::Value) that comes back *is* one of the four
types, and `boolean()`, `number()`, `string()` and `nodes()` convert it exactly as XPath §4 says,
so an expression yielding the wrong type is not a `ClassCastException` at run time.

Binding a prefix replaces `NamespaceContext`, one binding at a time:

```rust
use xylogue::dom::build;
use xylogue::xdm::{DomModel, Model};
use xylogue::xpath::XPath;

let doc = build::parse("<r xmlns:d='urn:d'><d:a>found</d:a></r>".as_bytes())?;
let model = DomModel::new(&doc);

// The document's prefix and the expression's need not agree — only the namespace does.
let expression = XPath::new().with_namespace("x", "urn:d").compile("//x:a")?;
let value = expression.evaluate(&model, model.root_node())?;

assert_eq!(value.string(&model), "found");
# Ok::<(), xylogue::Error>(())
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
use xylogue::transform::{Source, Transformer};

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
# Ok::<(), xylogue::Error>(())
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
use xylogue::validate::validate;

let xml = "<!DOCTYPE a [<!ELEMENT a (b)>]><a><c/></a>";
let report = validate(xml.as_bytes())?;

// Two violations, not one: `c` is not declared, and `a` was declared to hold a `b`.
assert!(!report.is_valid());
assert!(report.errors().iter().any(|error| error.message().contains("c")));
# Ok::<(), xylogue::Error>(())
```

The distinction Java draws between a fatal error and a recoverable one is kept, but it is in the
types rather than in which `ErrorHandler` method is called: a well-formedness error is the `Err`
of the call and stops the parse, a validity error is collected into the
[`Report`](xylogue_validate::Report) and the document is read to the end. A document with no
`DOCTYPE` is neither valid nor invalid — `had_dtd()` says which case you are in, so "nothing to
check against" cannot be mistaken for a pass.

## Writing XML

From a tree, the way `LSSerializer` or an identity `Transformer` is used in Java:

```rust
use xylogue::dom::build;
use xylogue::serialize::Serializer;

let doc = build::parse("<r><a><b>x</b></a></r>".as_bytes())?;
let root = doc.document_element().unwrap();

assert_eq!(Serializer::new().with_indent("  ").to_string(&doc, root), "<r>\n  <a>\n    <b>x</b>\n  </a>\n</r>");
# Ok::<(), xylogue::Error>(())
```

Call by call, as `XMLStreamWriter`:

```rust
use xylogue::serialize::XmlWriter;

let mut out = Vec::new();
let mut writer = XmlWriter::new(&mut out);
writer.write_start_element("note")?;
writer.write_attribute("lang", "en")?;
writer.write_characters("hi & bye")?;
writer.write_end_element()?;

assert_eq!(String::from_utf8(out).unwrap(), "<note lang=\"en\">hi &amp; bye</note>");
# Ok::<(), std::io::Error>(())
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
listener, lost the diagnostics" path does not exist. Validation keeps a listener-shaped seam —
[`ErrorListener`](xylogue_validate::ErrorListener) decides whether to carry on after a
recoverable violation — because there the choice is real.

**Nothing is fetched unless you say how.** Java resolves external entities by default, which is
why every hardening guide begins by turning that off; here a parser with no
[`UriResolver`](xylogue_parser::resolve::UriResolver) fetches nothing, and so does a
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
