//! What each layer costs, end to end.
//!
//! ```text
//! cargo bench -p xenolith
//! cargo bench -p xenolith -- xslt      # one group
//! ```
//!
//! The documents are generated here rather than vendored, so the numbers can be reproduced from
//! a checkout and the sizes can be varied without adding megabytes to the repository. They are
//! deterministic: the same run twice measures the same work.
//!
//! Every benchmark reports throughput in bytes of source, so the numbers can be compared across
//! sizes and against other processors, which is what a figure like "12 MB/s" is for.
//!
//! `cargo test --benches` runs each one once. That is what keeps them compiling and correct
//! between the rare occasions anyone benchmarks.

// criterion_group! defines a public function of its own, which the workspace's missing_docs
// cannot see a doc comment for. Nothing here is public API.
#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use xenolith::dom::build;
use xenolith::serialize::Serializer;
use xenolith::xdm::DomModel;
use xenolith::xpath::XPath;
use xenolith::xslt::{Stylesheet, Transform};

/// How many records the documents have. Small enough to run quickly, large enough that the
/// per-document costs — setting up a parser, compiling a stylesheet — do not dominate.
const SIZES: &[usize] = &[64, 1024];

/// A document of `records` entries, in the shape data actually comes in: repeated elements with
/// attributes, text, and a little structure below them.
fn document(records: usize) -> String {
  let mut xml = String::with_capacity(records * 160);
  xml.push_str("<?xml version=\"1.0\"?>\n<catalogue xmlns:m=\"urn:meta\" generated=\"2026-07-31\">\n");
  for index in 0..records {
    let year = 1970 + (index % 56);
    xml.push_str(&format!(
      "  <book id=\"b{index}\" year=\"{year}\" in-print=\"{}\">\n    \
       <title>Title number {index}</title>\n    \
       <author>Author {}</author>\n    \
       <price currency=\"JPY\">{}</price>\n    \
       <m:note>a note about book {index}, with <em>emphasis</em> in it</m:note>\n  \
       </book>\n",
      if index % 3 == 0 { "yes" } else { "no" },
      index % 97,
      1000 + (index * 37) % 9000,
    ));
  }
  xml.push_str("</catalogue>\n");
  xml
}

/// A stylesheet with a template rule per element name, as a real one has: the engine has to
/// choose among them for every node it visits.
const STYLESHEET: &str = r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:m="urn:meta">
  <xsl:output method="xml" omit-xml-declaration="yes"/>
  <xsl:template match="/"><report><xsl:apply-templates select="catalogue/book"/></report></xsl:template>
  <xsl:template match="book">
    <row id="{@id}" decade="{floor(@year div 10) * 10}">
      <xsl:apply-templates select="title | author | price"/>
      <xsl:if test="@in-print = 'yes'"><available/></xsl:if>
    </row>
  </xsl:template>
  <xsl:template match="title"><t><xsl:value-of select="."/></t></xsl:template>
  <xsl:template match="author"><a><xsl:value-of select="normalize-space(.)"/></a></xsl:template>
  <xsl:template match="price"><p><xsl:value-of select="format-number(., '#,##0')"/></p></xsl:template>
  <xsl:template match="m:note"/>
  <xsl:template match="em"><strong><xsl:apply-templates/></strong></xsl:template>
</xsl:stylesheet>
"#;

fn parsing(c: &mut Criterion) {
  let mut group = c.benchmark_group("parse");
  for &records in SIZES {
    let xml = document(records);
    group.throughput(Throughput::Bytes(xml.len() as u64));

    // The parser alone: events, no tree.
    group.bench_with_input(BenchmarkId::new("events", records), &xml, |b, xml| {
      b.iter(|| {
        let mut reader = xenolith::parser::Reader::new(xml.as_bytes());
        let mut events = 0_usize;
        while reader.advance().expect("well-formed").is_some() {
          events += 1;
        }
        black_box(events)
      });
    });

    // And with a DOM built from it, which is what most callers ask for.
    group.bench_with_input(BenchmarkId::new("dom", records), &xml, |b, xml| {
      b.iter(|| black_box(build::parse(xml.as_bytes()).expect("well-formed")));
    });
  }
  group.finish();
}

fn serializing(c: &mut Criterion) {
  let mut group = c.benchmark_group("serialize");
  for &records in SIZES {
    let xml = document(records);
    let tree = build::parse(xml.as_bytes()).expect("well-formed");
    let root = tree.document_element().expect("a document element");
    group.throughput(Throughput::Bytes(xml.len() as u64));
    group.bench_with_input(BenchmarkId::new("to_string", records), &tree, |b, tree| {
      b.iter(|| black_box(Serializer::new().to_string(tree, root)));
    });
  }
  group.finish();
}

fn xpath(c: &mut Criterion) {
  let mut group = c.benchmark_group("xpath");
  // Compiling does not depend on the document, so it is measured once and on its own: an
  // expression compiled per evaluation is the mistake this number is here to make visible.
  group.bench_function("compile", |b| {
    b.iter(|| black_box(XPath::new().compile("//book[@year > 2000]/title/text()").expect("parses")));
  });

  for &records in SIZES {
    let xml = document(records);
    let tree = build::parse(xml.as_bytes()).expect("well-formed");
    let model = DomModel::new(&tree);
    group.throughput(Throughput::Bytes(xml.len() as u64));

    // A descendant search with a predicate: the shape of nearly every real query.
    let query = XPath::new().compile("//book[@year > 2000]/title/text()").expect("parses");
    group.bench_with_input(BenchmarkId::new("descendant-with-predicate", records), &model, |b, model| {
      b.iter(|| black_box(query.evaluate(model, model.root_node()).expect("evaluates")));
    });

    // An aggregate over every element, which walks the whole tree rather than a slice of it.
    let counting = XPath::new().compile("count(//*[string-length(name()) > 2])").expect("parses");
    group.bench_with_input(BenchmarkId::new("count-over-all", records), &model, |b, model| {
      b.iter(|| black_box(counting.evaluate(model, model.root_node()).expect("evaluates")));
    });
  }
  group.finish();
}

fn xslt(c: &mut Criterion) {
  let mut group = c.benchmark_group("xslt");
  group.bench_function("compile", |b| {
    b.iter(|| black_box(Stylesheet::compile(STYLESHEET.as_bytes(), "urn:bench").expect("compiles")));
  });

  let stylesheet = Stylesheet::compile(STYLESHEET.as_bytes(), "urn:bench").expect("compiles");
  for &records in SIZES {
    let xml = document(records);
    let tree = build::parse(xml.as_bytes()).expect("well-formed");
    let model = DomModel::new(&tree);
    group.throughput(Throughput::Bytes(xml.len() as u64));

    // The transformation alone, over a tree that is already built.
    group.bench_with_input(BenchmarkId::new("transform", records), &model, |b, model| {
      b.iter(|| {
        let result = Transform::new().run(&stylesheet, model, model.root_node()).expect("transforms");
        black_box(result.serialize())
      });
    });
  }

  // Everything a caller does with one document: parse it, run the stylesheet, write the result.
  for &records in SIZES {
    let xml = document(records);
    group.throughput(Throughput::Bytes(xml.len() as u64));
    group.bench_with_input(BenchmarkId::new("end-to-end", records), &xml, |b, xml| {
      b.iter(|| {
        let tree = build::parse(xml.as_bytes()).expect("well-formed");
        let model = DomModel::new(&tree);
        let result = Transform::new().run(&stylesheet, &model, model.root_node()).expect("transforms");
        black_box(result.serialize())
      });
    });
  }
  group.finish();
}

/// A stylesheet with `rules` template rules, of which only the handful that name the document's
/// own elements can ever match.
///
/// Choosing a rule for a node means testing it against the rules; whether that costs the whole
/// set or only the plausible ones is what this measures. A stylesheet of a few hundred rules is
/// ordinary — DocBook's is thousands — so the answer decides whether an index is worth having.
fn stylesheet_with_rules(rules: usize) -> String {
  let mut xsl = String::from(
    r#"<?xml version="1.0"?>
<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform">
  <xsl:output method="xml" omit-xml-declaration="yes"/>
  <xsl:template match="/"><report><xsl:apply-templates select="//book"/></report></xsl:template>
  <xsl:template match="book"><row><xsl:apply-templates select="title | author | price"/></row></xsl:template>
  <xsl:template match="title"><t><xsl:value-of select="."/></t></xsl:template>
  <xsl:template match="author"><a><xsl:value-of select="."/></a></xsl:template>
  <xsl:template match="price"><p><xsl:value-of select="."/></p></xsl:template>
"#,
  );
  for index in 0..rules {
    // Rules that match nothing in the document, so the result is the same however many there
    // are and only the cost of choosing changes.
    xsl.push_str(&format!("  <xsl:template match=\"absent{index}\"><x{index}/></xsl:template>\n"));
  }
  xsl.push_str("</xsl:stylesheet>\n");
  xsl
}

fn rule_choice(c: &mut Criterion) {
  let mut group = c.benchmark_group("xslt-rules");
  let xml = document(256);
  let tree = build::parse(xml.as_bytes()).expect("well-formed");
  let model = DomModel::new(&tree);

  for &rules in &[0_usize, 64, 512] {
    let stylesheet = Stylesheet::compile(stylesheet_with_rules(rules).as_bytes(), "urn:bench").expect("compiles");
    group.throughput(Throughput::Bytes(xml.len() as u64));
    group.bench_with_input(BenchmarkId::new("transform", rules), &model, |b, model| {
      b.iter(|| {
        let result = Transform::new().run(&stylesheet, model, model.root_node()).expect("transforms");
        black_box(result.serialize())
      });
    });
  }
  group.finish();
}

criterion_group!(benches, parsing, serializing, xpath, xslt, rule_choice);
criterion_main!(benches);
