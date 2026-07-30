//! Differential testing against another XPath 1.0 engine.
//!
//! The tests in this crate were written alongside the code they test, so they can only assert
//! what their author understood the specification to say. This one asks a second, independent
//! implementation the same questions and requires the same answers.
//!
//! # Which implementation
//!
//! **Java** — `javax.xml.xpath`, on whatever engine the JDK ships — since matching Java's XML
//! behaviour is this project's goal, not merely being defensible against some other engine.
//!
//! Point `XYLO_JAVA` at a `java` of version 11 or later to run it:
//!
//! ```text
//! XYLO_JAVA=java cargo test -p xylograph-xpath --test differential -- --nocapture
//! ```
//!
//! Nothing has to be built first: `tests/java/XPathReference.java` is run in the JDK's
//! single-file source mode, and reads expressions from its standard input. Without `XYLO_JAVA`
//! the comparison is skipped and says so, while [`the_corpus_is_evaluable`] still runs — so a
//! corpus that had rotted is caught wherever the tests run, not only where a JDK is installed.
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
//! two engines can disagree without either being wrong. A differential test that reports
//! differences the specification permits is a test people learn to ignore, so those cases are
//! excluded on purpose. Numbers that *are* pinned down — exact binary fractions, integers, and
//! the special values — are compared.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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

/// Where Java's answer is wrong and this crate's is right, with the evidence.
///
/// A differential test is only useful if a difference means something, so a difference that has
/// been looked into and settled is recorded here rather than left to be re-diagnosed every run —
/// and it is reported by name, never quietly skipped.
///
/// Nothing goes in this list because the answers merely differ. Each entry says which paragraph
/// decides it, and why the reference is the one that is wrong.
const KNOWN_DIFFERENCES: &[(&str, &str)] = &[(
  "name(//processing-instruction())",
  "the JDK answers with the document element's name. §4.1 says name() is the expanded-name of \
   the node first in document order in its argument, and §5.7 gives a processing instruction an \
   expanded-name whose local part is its target — so the answer is the target. The JDK \
   contradicts itself here: count() and string() over that same node-set give 1 and the PI's \
   data, and name() answers correctly for `/library/processing-instruction()`, for \
   `descendant::processing-instruction()` and for `//processing-instruction('process')`. Only \
   `//` with a bare processing-instruction() node test goes wrong, and local-name() with it.",
)];

/// What Java said about one expression.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
  /// The value, as `string()` gives it.
  Value(String),
  /// Java refused the expression, and what it said about it.
  Refused(String),
}

/// Asks Java every expression of one case, in one run of the JVM.
///
/// One process for the whole case rather than one per expression: starting a JVM and compiling
/// the reference costs about a second, and doing that ninety times would make this a test nobody
/// runs.
fn java_answers(java: &PathBuf, xml: &str, expressions: &[String]) -> Result<Vec<Answer>, String> {
  let directory = std::env::temp_dir().join(format!("xylograph-differential-{}", std::process::id()));
  std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
  let document = directory.join("case.xml");
  std::fs::write(&document, xml).map_err(|error| error.to_string())?;

  let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/java/XPathReference.java");
  let mut child = Command::new(java)
    .arg(&source)
    .arg(&document)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| format!("{}: {error}", java.display()))?;
  {
    let stdin = child.stdin.as_mut().ok_or("no pipe to java")?;
    for expression in expressions {
      writeln!(stdin, "{expression}").map_err(|error| error.to_string())?;
    }
  }
  let output = child.wait_with_output().map_err(|error| error.to_string())?;
  let _ = std::fs::remove_dir_all(&directory);
  if !output.status.success() {
    return Err(format!("java failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
  }

  let text = String::from_utf8_lossy(&output.stdout);
  let answers: Vec<Answer> = text
    .lines()
    .filter_map(|line| match line.split_once('\t') {
      Some(("ok", value)) => Some(Answer::Value(unescape(value))),
      Some(("error", why)) => Some(Answer::Refused(unescape(why))),
      _ => None,
    })
    .collect();
  if answers.len() != expressions.len() {
    return Err(format!("java answered {} of {} expressions", answers.len(), expressions.len()));
  }
  Ok(answers)
}

/// Undoes the escaping `XPathReference.java` applies, so that one answer can be one line.
fn unescape(text: &str) -> String {
  let mut written = String::with_capacity(text.len());
  let mut characters = text.chars();
  while let Some(character) = characters.next() {
    if character != '\\' {
      written.push(character);
      continue;
    }
    match characters.next() {
      Some('n') => written.push('\n'),
      Some('r') => written.push('\r'),
      Some('t') => written.push('\t'),
      Some('\\') => written.push('\\'),
      // Not an escape this side writes; keep it as it stands rather than lose it.
      Some(other) => {
        written.push('\\');
        written.push(other);
      }
      None => written.push('\\'),
    }
  }
  written
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
fn the_same_expressions_give_the_same_answers_as_java() {
  let Some(java) = std::env::var_os("XYLO_JAVA").map(PathBuf::from) else {
    eprintln!("skipped: set XYLO_JAVA to a java of version 11 or later to compare against the JDK");
    return;
  };

  let mut checked = 0;
  let mut differences = Vec::new();
  let mut known = Vec::new();

  for case in CASES {
    // The brackets keep the value's own whitespace while letting a trailing newline go.
    let wrapped: Vec<String> =
      case.expressions.iter().map(|expression| format!("concat('[', string({expression}), ']')")).collect();
    let answers = java_answers(&java, case.xml, &wrapped).unwrap_or_else(|error| panic!("{error}"));

    for ((expression, wrapped), theirs) in case.expressions.iter().zip(&wrapped).zip(answers) {
      checked += 1;
      let mine = ours(case.xml, wrapped);
      if let Some((_, why)) = KNOWN_DIFFERENCES.iter().find(|(known, _)| known == expression) {
        // Still evaluated on both sides — an entry that had become wrong, because the reference
        // was fixed or this crate changed, should be noticed rather than protect a new bug.
        let agree = matches!((&theirs, &mine), (Answer::Value(theirs), Ok(mine)) if theirs == mine);
        assert!(!agree, "{expression} is recorded as a known difference, but the two now agree");
        known.push(format!("{expression}\n  java: {theirs:?}\n  ours: {mine:?}\n  why:  {why}"));
        continue;
      }
      match (&theirs, &mine) {
        (Answer::Value(theirs), Ok(mine)) if theirs == mine => {}
        (Answer::Value(theirs), Ok(mine)) => {
          differences.push(format!("{expression}\n  java: {theirs}\n  ours: {mine}"));
        }
        (Answer::Value(theirs), Err(error)) => {
          differences.push(format!("{expression}\n  java: {theirs}\n  ours: failed: {error}"));
        }
        // The corpus is plain XPath 1.0, so a refusal from either side is itself a difference:
        // one of the two is refusing something it should evaluate.
        (Answer::Refused(why), Ok(mine)) => {
          differences.push(format!("{expression}\n  java: refused: {why}\n  ours: {mine}"));
        }
        (Answer::Refused(why), Err(error)) => {
          differences.push(format!("{expression}\n  java: refused: {why}\n  ours: failed: {error}"));
        }
      }
    }
  }

  eprintln!(
    "differential against Java: {checked} compared, {} differed, {} known differences",
    differences.len(),
    known.len()
  );
  for entry in &known {
    eprintln!("\n{entry}");
  }
  assert!(checked > 0, "the corpus is empty");
  assert!(differences.is_empty(), "\n{}", differences.join("\n"));
}
