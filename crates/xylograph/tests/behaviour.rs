//! A report of what this build does where the specifications do not say.
//!
//! XML, XPath and their neighbours leave a good deal open. Some of it they leave open on
//! purpose — "implementation-dependent", in so many words — and some they simply never mention.
//! Either way a caller who moves between implementations, or who compares this library with
//! Java, needs to know which of its behaviours are guaranteed and which merely happen to be so.
//!
//! This test writes that down, by *observing* the behaviour rather than restating the
//! documentation, so the report cannot drift away from the code. Run it with:
//!
//! ```text
//! cargo test -p xylograph --test behaviour -- --nocapture
//! ```
//!
//! Three headings, and the difference between them matters:
//!
//! - **Undefined by the specification** — the specification does not say, or says the answer is
//!   implementation-dependent. Another conformant implementation may differ, and a document that
//!   relies on the answer is not portable.
//! - **Chosen by this implementation** — the specification allows a range and this library picks
//!   one point in it. Stable across platforms and releases unless the changelog says otherwise.
//! - **Dependent on the build or the platform** — the answer can differ between two builds of
//!   this same library, so it cannot be relied on without pinning the build.
//!
//! Where a behaviour *is* pinned by a specification it does not belong here; it belongs in a
//! test that asserts it. A few such assertions are made along the way, so that the report cannot
//! claim something is open when it is not.

use xylograph::dom::{Document, build};
use xylograph::serialize::Serializer;
use xylograph::xdm::{DomModel, Model};
use xylograph::xpath::{Value, XPath};

/// Evaluates an XPath expression over a document and renders the result as XPath would.
fn value(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().compile(expression).expect("parses");
  query.evaluate(&model, model.root_node()).expect("evaluates").string(&model)
}

/// The names of the nodes an expression selects, comma-separated in the order they come back.
fn names(xml: &str, expression: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let query = XPath::new().compile(expression).expect("parses");
  match query.evaluate(&model, model.root_node()).expect("evaluates") {
    Value::NodeSet(nodes) => nodes
      .iter()
      .map(|node| model.qualified_name(*node).unwrap_or_else(|| "?".to_owned()))
      .collect::<Vec<_>>()
      .join(", "),
    other => panic!("expected a node-set, got {other:?}"),
  }
}

/// Serializes a parsed document back to text.
fn round_trip(xml: &str) -> String {
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  Serializer::new().to_string(&doc, doc.document_element().expect("a root element"))
}

/// Sorts the `i` elements of `xml` with one `xsl:sort`, and lists them in the order they come out.
fn sorted(xml: &str, sort: &str) -> String {
  let source = format!(
    "<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\
       <xsl:template match=\"/\"><xsl:for-each select=\"//i\">{sort}\
         <xsl:value-of select=\".\"/><xsl:text> </xsl:text></xsl:for-each></xsl:template>\
     </xsl:stylesheet>"
  );
  let stylesheet = xylograph::xslt::Stylesheet::compile(source.as_bytes(), "file:///s.xsl").expect("compiles");
  let doc = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&doc);
  let result = xylograph::xslt::transform(&stylesheet, &model, model.root_node()).expect("transforms");
  result.text().trim_end().to_owned()
}

/// One entry of the report.
struct Item {
  /// Where the question comes from.
  reference: &'static str,
  /// The question itself.
  question: &'static str,
  /// What the specification does, or does not, settle.
  specification: String,
  /// What this build does, observed rather than described.
  observed: Vec<String>,
}

fn item(reference: &'static str, question: &'static str, specification: impl Into<String>) -> Item {
  Item { reference, question, specification: specification.into(), observed: Vec::new() }
}

impl Item {
  fn observe(mut self, label: &str, value: impl Into<String>) -> Self {
    self.observed.push(format!("{label} => {}", value.into()));
    self
  }

  fn print(&self) {
    if self.reference.is_empty() {
      println!("  {}", self.question);
    } else {
      println!("  {} — {}", self.reference, self.question);
    }
    println!("      specification: {}", self.specification);
    for observation in &self.observed {
      println!("      {observation}");
    }
    println!();
  }
}

fn heading(title: &str, explanation: &str) {
  println!("{title}");
  println!("{}", "-".repeat(title.len()));
  println!("{explanation}\n");
}

#[test]
fn report_of_behaviour_the_specifications_leave_open() {
  println!("\n============================================================");
  println!(" xylograph — behaviour where the specifications do not say");
  println!("============================================================\n");

  heading(
    "1. UNDEFINED BY THE SPECIFICATION",
    "The specification does not settle these. Another conformant implementation may\n\
     answer differently, and a document that depends on the answer is not portable.",
  );
  for entry in undefined_by_the_specification() {
    entry.print();
  }

  heading(
    "2. CHOSEN BY THIS IMPLEMENTATION",
    "The specification allows a range; this library picks one point in it. Stable across\n\
     platforms, and across releases unless the changelog says otherwise.",
  );
  for entry in chosen_by_this_implementation() {
    entry.print();
  }

  heading(
    "3. DEPENDENT ON THE BUILD OR THE PLATFORM",
    "These can differ between two builds of this same library, so they cannot be relied\n\
     on without pinning the build.",
  );
  for entry in dependent_on_the_build() {
    entry.print();
  }

  println!("============================================================\n");
}

fn undefined_by_the_specification() -> Vec<Item> {
  vec![
    item(
      "XPath 1.0 §4.2",
      "how many digits a number with no exact decimal form is written with",
      "requires a plain decimal with a digit either side of the point, but does not fix the \
       number of significant digits",
    )
    .observe("string(1 div 3)", value("<r/>", "string(1 div 3)"))
    .observe("string(0.1 + 0.2)", value("<r/>", "string(0.1 + 0.2)"))
    .observe("string(1 div 7)", value("<r/>", "string(1 div 7)"))
    .observe("this build writes", "the shortest form that reads back exactly")
    .observe("libxml2 writes", "fifteen significant digits, so 1 div 3 is 0.333333333333333"),
    item(
      "XPath 1.0 §5.3",
      "the order of attribute nodes on the attribute axis",
      "says the relative order of attribute nodes is implementation-dependent",
    )
    .observe("<r b='1' a='2' c='3'/> — /r/@*", names("<r b='1' a='2' c='3'/>", "/r/@*"))
    .observe("this build uses", "the order the attributes were written in"),
    item(
      "XPath 1.0 §5.4",
      "the order of namespace nodes on the namespace axis",
      "says the relative order of namespace nodes is implementation-dependent",
    )
    // `/*` rather than `/r`: the element is in the default namespace, and an unprefixed name
    // test matches only a name in no namespace.
    .observe(
      "<r xmlns:z='urn:z' xmlns:a='urn:a' xmlns='urn:d'/> — /*/namespace::*",
      names("<r xmlns:z='urn:z' xmlns:a='urn:a' xmlns='urn:d'/>", "/*/namespace::*"),
    )
    .observe("(the empty name is the default namespace)", "and `xml` is always in scope")
    .observe("this build uses", "the default namespace first, then prefixes in name order"),
    item(
      "XML 1.0 §3.1",
      "the order attributes are reported and written in",
      "says the order of attributes in a start tag is not significant",
    )
    .observe("<r b='1' a='2'/> serialized", round_trip("<r b='1' a='2'/>"))
    .observe("this build uses", "the order they were written in, on the way in and out"),
    item(
      "XPath 1.0 §4.1",
      "what id() finds when nothing has said which attribute is an ID",
      "selects elements by their unique ID, but an ID is only an ID because a DTD or a schema \
       said so",
    )
    .observe("no DTD: count(id('a')) over <r><i k='a'/></r>", value("<r><i k='a'/></r>", "string(count(id('a')))"))
    .observe("this build uses", "only attributes a DTD or xml:id typed as ID"),
    item(
      "XSLT 1.0 §10",
      "the order two strings sort in for a text xsl:sort",
      "says the sort uses the collating sequence for the language, without defining one; two \
       conformant processors may put the same two strings the other way round",
    )
    .observe("a, z, \u{e4} — no lang", sorted("<r><i>\u{e4}</i><i>z</i><i>a</i></r>", "<xsl:sort/>"))
    .observe("a, z, \u{e4} — lang='sv'", sorted("<r><i>\u{e4}</i><i>z</i><i>a</i></r>", "<xsl:sort lang='sv'/>"))
    .observe("a, z, \u{e4} — lang='de'", sorted("<r><i>\u{e4}</i><i>z</i><i>a</i></r>", "<xsl:sort lang='de'/>"))
    .observe("this build uses", if cfg!(feature = "icu") { "CLDR, through ICU4X" } else { "Unicode code point order" }),
    item(
      "XSLT 1.0 §10",
      "where a sort key that is not a number goes in a numeric sort",
      "converts the key with number(), which gives NaN for a key that is not a number, but does \
       not say where NaN sorts",
    )
    .observe(
      "2, oops, 1 — data-type='number'",
      sorted("<r><i>2</i><i>oops</i><i>1</i></r>", "<xsl:sort data-type='number'/>"),
    )
    .observe("this build puts", "NaN first, which is what XSLT 2.0 later settled on")
    .observe(
      "descending reverses it",
      sorted("<r><i>2</i><i>oops</i><i>1</i></r>", "<xsl:sort data-type='number' order='descending'/>"),
    ),
  ]
}

fn chosen_by_this_implementation() -> Vec<Item> {
  // Both zeros print as "0" (XPath 1.0 §4.2 is explicit), so the sign of a rounded zero shows
  // only through division. That the sign is negative is required, not a choice — asserted here
  // so the report cannot claim otherwise.
  assert_eq!(value("<r/>", "string(1 div round(-0.5))"), "-Infinity", "§4.4: round(-0.5) is -0");

  vec![
    item(
      "XML 1.0 §2.4",
      "which characters are escaped when writing text",
      "requires `<` and `&` to be escaped, and `>` only where it would close a CDATA section",
    )
    .observe("text a > b", round_trip("<r>a &gt; b</r>"))
    .observe("this build", "escapes every `>`, which is always correct and simpler to read"),
    item(
      "XML 1.0 §3.1",
      "whether an element with no content is written <a/> or <a></a>",
      "says the two are equivalent",
    )
    .observe("empty element", round_trip("<r><a></a></r>"))
    .observe("this build uses", "the self-closing form"),
    item("XML 1.0 §3.1", "which quotation mark encloses an attribute value", "allows either ' or \"")
      .observe("attribute written with '", round_trip("<r a='1'/>"))
      .observe("this build uses", "a double quote, escaping any in the value"),
    item(
      "XPath 1.0 §4.3",
      "how lang() compares a language tag",
      "says the comparison is without regard to case, and that a sublanguage answers to its \
       language",
    )
    .observe("xml:lang='en-GB', lang('EN')", value("<r xml:lang='en-GB'><a/></r>", "string(count(/r/a[lang('EN')]))"))
    .observe("this build uses", "ASCII case folding, which is enough for the tags RFC 5646 allows"),
    item(
      "XML 1.0 §4.4.3",
      "how much entity expansion is allowed before a document is refused",
      "lets an implementation impose limits, and says nothing about what they should be",
    )
    .observe("max nested entity depth", xylograph::parser::Limits::default().max_depth.to_string())
    .observe("max expansions", xylograph::parser::Limits::default().max_expansions.to_string())
    .observe("max expanded characters", xylograph::parser::Limits::default().max_expansion_chars.to_string())
    .observe("max element nesting depth", xylograph::parser::Limits::default().max_element_depth.to_string())
    .observe("these are", "raisable through Limits, and removable with Limits::unlimited()"),
    item("DOM Level 3 Core", "how many nodes one document may hold", "does not say; the DOM has no stated bound")
      .observe("this build allows", format!("{} nodes, the range of the u32 handle", u32::MAX))
      .observe("beyond that", "creating a node panics rather than silently aliasing one"),
  ]
}

fn dependent_on_the_build() -> Vec<Item> {
  let encodings = ["UTF-8", "UTF-16", "US-ASCII", "ISO-8859-1", "Shift_JIS", "EUC-JP", "windows-1252"];
  let available: Vec<&str> =
    encodings.into_iter().filter(|label| xylograph::encoding::lookup(label).is_some()).collect();

  vec![
    item(
      "XML 1.0 §4.3.3",
      "which character encodings a document may be written in",
      "requires UTF-8 and UTF-16 and permits others, without saying which",
    )
    .observe("available in this build", available.join(", "))
    .observe("this depends on", "the `encodings` feature, which adds everything encoding_rs knows")
    .observe("without it", "an unsupported encoding is an error naming the feature, never a wrong decoding"),
    item(
      "",
      "which optional layers are compiled in",
      "not a specification question, but it changes what the library will do with a document",
    )
    .observe("xml:base (feature xml-base)", cfg!(feature = "xml-base").to_string())
    .observe("xml:id (feature xml-id)", cfg!(feature = "xml-id").to_string())
    .observe("XInclude (feature xinclude)", cfg!(feature = "xinclude").to_string())
    .observe("DOM building (feature parse)", cfg!(feature = "parse").to_string())
    .observe("language-aware collation (feature icu)", xylograph::xslt::language_aware_collation().to_string()),
    item(
      "XSLT 1.0 §10",
      "how far apart two builds can be on the same xsl:sort",
      "leaves the collating sequence open, so the `icu` feature changes the answer rather than \
       merely making it faster",
    )
    .observe("with icu", "a language's own conventions, from CLDR — \u{e4} beside a in German, after z in Swedish")
    .observe("without icu", "Unicode code point order, which is stable but is nobody's alphabet")
    .observe("this build", if xylograph::xslt::language_aware_collation() { "has it" } else { "does not" })
    .observe("a, z, \u{e4} with lang='sv'", sorted("<r><i>\u{e4}</i><i>z</i><i>a</i></r>", "<xsl:sort lang='sv'/>")),
    item(
      "",
      "what does *not* depend on the platform, though it might be expected to",
      "worth stating, since floating point is where portability is usually lost",
    )
    .observe(
      "number formatting",
      "identical on every target; Rust's f64 formatting is not locale- or platform-dependent",
    )
    .observe("string comparison", "by Unicode scalar value, as XPath 1.0 requires; no collation is involved")
    .observe("document order", "assigned when the model is built, so it does not vary with pointer width"),
  ]
}

/// The report describes a real document; if the observations stop being computable the report
/// has rotted, and this says so without anyone having to read the output.
#[test]
fn every_observation_in_the_report_can_be_made() {
  let sections = [undefined_by_the_specification(), chosen_by_this_implementation(), dependent_on_the_build()];
  for section in sections {
    for entry in section {
      assert!(!entry.observed.is_empty(), "{:?} observes nothing", entry.question);
      for observation in &entry.observed {
        assert!(!observation.ends_with("=> "), "{:?} has an empty observation", entry.question);
      }
      assert!(!entry.specification.is_empty(), "{:?} does not say what the specification settles", entry.question);
    }
  }
}

/// A document held in a `Document` built by hand — not parsed — has no base URI and no ID
/// typing, and the report says so; this checks the claim.
#[test]
fn a_hand_built_document_has_neither_base_uri_nor_ids() {
  let mut doc = Document::new();
  let root = doc.create_element("r").expect("a valid name");
  doc.set_attribute(root, "id", "x").expect("an element");
  doc.append_child(doc.root(), root).expect("a root element");
  assert_eq!(doc.base_uri(root), None);
  assert_eq!(doc.get_element_by_id("x"), None, "an attribute named id is not an ID until it is typed as one");
}
