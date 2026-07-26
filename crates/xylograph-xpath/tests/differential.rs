//! Differential testing against another XPath 1.0 engine.
//!
//! The tests in this crate were written alongside the code they test, so they can only assert
//! what their author understood the specification to say. This one asks a second, independent
//! implementation the same questions and requires the same answers.
//!
//! # Which implementation, and when
//!
//! The reference that matters for this project is **Java** — `javax.xml.xpath` on the JDK's
//! Xerces — since matching Java's XML behaviour is the goal, not merely being defensible against
//! some other engine. That harness is wired up once the library is complete, so that the whole
//! surface can be compared at once rather than a moving target.
//!
//! Until then the comparison here goes to libxml2 through `xmllint`, as a stand-in, and is
//! **not** run in CI: set `XYLO_XMLLINT` to the `xmllint` binary to run it by hand. What does
//! run everywhere is [`the_corpus_is_evaluable`], which keeps the expression corpus from rotting
//! while it waits — the corpus is the reusable part, and the Java harness will take it over.
//!
//! # What is compared, and what is not
//!
//! Every expression is wrapped as `concat('[', string(E), ']')`, so both engines return one
//! string: the brackets keep whitespace exact while a trailing newline can be trimmed away, and
//! going through `string()` avoids comparing two engines' node serialization, which the
//! specification does not define.
//!
//! The corpus deliberately leaves out numbers whose decimal form is not exact — `1 div 3`, or
//! `0.1 + 0.2`. XPath 1.0 §4.2 does not fix how many digits such a number is written with, so
//! libxml2 (fifteen significant digits) and this crate (the shortest form that reads back
//! exactly) disagree without either being wrong. A differential test that reports differences
//! the specification permits is a test people learn to ignore, so those cases are excluded on
//! purpose. Numbers that *are* pinned down — exact binary fractions, integers, and the special
//! values — are compared.

use std::path::{Path, PathBuf};
use std::process::Command;

use xylograph_dom::build;
use xylograph_xdm::DomModel;
use xylograph_xpath::XPathExpression;

/// A document, and the expressions to ask about it.
struct Case {
  xml: &'static str,
  expressions: &'static [&'static str],
}

const LIBRARY: &str = r#"<?xml version="1.0"?>
<library k="top">
  <book id="b1" year="1999"><title>Alpha</title><price>10.5</price></book>
  <book id="b2" year="2003"><title>Beta</title><price>7</price></book>
  <book id="b3" year="2003"><title>Gamma</title><price>12</price></book>
  <!--a comment-->
  <?process data?>
</library>
"#;

const CASES: &[Case] = &[
  Case {
    xml: LIBRARY,
    expressions: &[
      // Paths, axes and node tests.
      "count(//book)",
      "count(//*)",
      "count(//node())",
      "count(//text())",
      "count(//comment())",
      "count(//processing-instruction())",
      "count(//@*)",
      "count(/library/book[1]/descendant-or-self::node())",
      "count(//title/ancestor::*)",
      "count(//book[1]/following-sibling::book)",
      "count(//book[3]/preceding-sibling::book)",
      "count(//book[1]/following::*)",
      "count(//book[3]/preceding::*)",
      "count(//book/parent::*)",
      "count(//nosuch)",
      // Predicates.
      "count(//book[@year='2003'])",
      "count(//book[position() = 1])",
      "count(//book[last()])",
      "count(//book[position() mod 2 = 1])",
      "count((//book)[2])",
      "count(//book[price > 10])",
      "count(//book[@id])",
      // Names and string-values.
      "string(//book[1]/title)",
      "string(//book[last()]/title)",
      "string(//book[2]/@id)",
      "string(//nosuch)",
      "name(//book[1])",
      "local-name(//book[1])",
      "name(//@k)",
      "name(//processing-instruction())",
      "string(//comment())",
      "string(/library/@k)",
      // Comparisons, including the node-set rules.
      "string(//book/@year = 2003)",
      "string(//book/@year = 1066)",
      "string(//price > 11)",
      "string(//nosuch = 'x')",
      "string(boolean(//nosuch))",
      "string(count(//book) > 2)",
      "string('10' > '9')",
      "string('10' = '9')",
      // Arithmetic and numbers the specification pins down exactly.
      "string(2 + 3 * 4)",
      "string(1 div 2)",
      "string(3 div 4)",
      "string(7 div 2)",
      "string(-5 mod 3)",
      "string(5 mod 3)",
      "string(sum(//price))",
      "string(1 div 0)",
      "string(-1 div 0)",
      "string(0 div 0)",
      "string(number('abc'))",
      "string(number('  42  '))",
      "string(round(-1.5))",
      "string(round(2.5))",
      "string(round(0.5))",
      "string(floor(-1.5))",
      "string(ceiling(-1.5))",
      "string(-(2 + 3))",
      // The string library.
      "string(concat('a', 'b', 'c'))",
      "string(substring('12345', 2))",
      "string(substring('12345', 1.5, 2.6))",
      "string(substring('12345', 0, 3))",
      "string(substring('12345', -42, 1 div 0))",
      "string(substring-before('1999/04', '/'))",
      "string(substring-after('1999/04', '/'))",
      "string(substring-before('abc', 'x'))",
      "string(string-length('hello'))",
      "string(normalize-space('  a  b  '))",
      "string(translate('bar', 'abc', 'ABC'))",
      "string(translate('--aaa--', 'abc-', 'ABC'))",
      "string(starts-with('abcd', 'ab'))",
      "string(contains('abcd', 'bc'))",
      "string(not(1 = 1))",
    ],
  },
  Case {
    xml: "<r><a>1</a><b><a>2</a></b>text<a>3</a></r>",
    expressions: &[
      "count(//a)",
      "string(//a)",
      "string(/r/a)",
      "count(/r/a)",
      "string(sum(//a))",
      "count(/r/node())",
      "string(/r/b/a/../../a)",
      "count(//a[. > 1])",
      "string(//a[2])",
    ],
  },
];

/// This crate's answer: the value of `expression` over `xml`, as a string.
fn ours(xml: &str, expression: &str) -> Result<String, String> {
  let doc = build::parse(xml.as_bytes()).map_err(|e| e.to_string())?;
  let model = DomModel::new(&doc);
  let query = XPathExpression::compile(expression).map_err(|e| e.to_string())?;
  let value = query.evaluate(&model, model.root_node()).map_err(|e| e.to_string())?;
  Ok(value.string(&model))
}

/// libxml2's answer, or `None` if it declined to evaluate the expression.
fn reference(xmllint: &Path, document: &Path, expression: &str) -> Option<String> {
  let output = Command::new(xmllint).arg("--xpath").arg(expression).arg(document).output().ok()?;
  if !output.status.success() {
    return None;
  }
  Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Every expression in the corpus is one this crate can evaluate.
///
/// This runs whether or not libxml2 is available, so a corpus that had rotted — an expression
/// that stopped parsing, say — is caught here rather than sitting unnoticed until someone
/// happened to have `xmllint` on their path.
#[test]
fn the_corpus_is_evaluable() {
  let mut failures = Vec::new();
  for case in CASES {
    for expression in case.expressions {
      let wrapped = format!("concat('[', string({expression}), ']')");
      match ours(case.xml, &wrapped) {
        // Every wrapped expression is a string, so it must at least be bracketed.
        Ok(value) if value.starts_with('[') && value.ends_with(']') => {}
        Ok(value) => failures.push(format!("{expression} gave {value:?}, which is not a wrapped string")),
        Err(error) => failures.push(format!("{expression} failed: {error}")),
      }
    }
  }
  assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn the_same_expressions_give_the_same_answers_as_libxml2() {
  let Some(xmllint) = std::env::var_os("XYLO_XMLLINT").map(PathBuf::from) else {
    eprintln!("skipped: set XYLO_XMLLINT to an xmllint binary to compare against libxml2");
    return;
  };

  let directory = std::env::temp_dir().join(format!("xylograph-differential-{}", std::process::id()));
  std::fs::create_dir_all(&directory).expect("a temporary directory");

  let (mut checked, mut declined) = (0, 0);
  let mut differences = Vec::new();

  for (index, case) in CASES.iter().enumerate() {
    let document = directory.join(format!("case{index}.xml"));
    std::fs::write(&document, case.xml).expect("writing the document");

    for expression in case.expressions {
      // The brackets keep the value's own whitespace while letting a trailing newline go.
      let wrapped = format!("concat('[', string({expression}), ']')");
      let Some(theirs) = reference(&xmllint, &document, &wrapped) else {
        declined += 1;
        continue;
      };
      checked += 1;
      match ours(case.xml, &wrapped) {
        Ok(mine) if mine == theirs => {}
        Ok(mine) => differences.push(format!("{expression}\n  libxml2: {theirs}\n  ours:    {mine}")),
        Err(error) => differences.push(format!("{expression}\n  libxml2: {theirs}\n  ours:    failed: {error}")),
      }
    }
  }

  let _ = std::fs::remove_dir_all(&directory);
  eprintln!("differential: {checked} compared, {declined} declined by libxml2, {} differed", differences.len());
  assert!(checked > 0, "libxml2 evaluated nothing; is {} an xmllint binary?", xmllint.display());
  assert!(differences.is_empty(), "\n{}", differences.join("\n"));
}
