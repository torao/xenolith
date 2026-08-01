//! `http://exslt.org/regular-expressions` — testing, matching and replacing with a pattern.
//!
//! # Which regular expressions
//!
//! EXSLT says its patterns are ECMAScript's, and this uses the [`regex`] crate, whose syntax is
//! ECMAScript's for everything that can be matched in linear time and **does not have
//! backreferences or lookaround** — `(a)\1` and `(?=a)` are refused rather than matched wrongly.
//! That is a real difference from libxslt, which uses a backtracking engine, and it is the price
//! of a matcher that cannot be made to run for ever by its input. A pattern this cannot compile
//! is reported with what the crate said, so it is never silently treated as no match.
//!
//! # Flags
//!
//! `i` matches without regard to case, `m` makes `^` and `$` match at every line, and `g` means
//! every match rather than the first. Any other letter is refused, since a flag that was meant to
//! do something and quietly did nothing is worse than one that says it is not understood.
//!
//! # Specifications
//!
//! - [`exslt:regular-expressions`](http://exslt.org/regexp/index.html)

use std::rc::Rc;

use regex::{Regex, RegexBuilder};
use xylogue_core::error::{Error, ErrorKind, Result};
use xylogue_dom::Document;
use xylogue_xdm::Model;
use xylogue_xpath::{Context, Functions, Value};
use xylogue_xslt::DocumentSource;

use crate::support::arity;

/// The namespace a stylesheet binds a prefix to for this module.
pub const NAMESPACE: &str = "http://exslt.org/regular-expressions";

/// What `regexp:match` names each piece it finds.
const MATCH: &str = "match";

/// Adds this module's functions, with `trees` for the one that answers with nodes.
#[must_use]
pub fn register<M: Model>(functions: Functions<M>, trees: &Rc<dyn DocumentSource<M::Node>>) -> Functions<M> {
  let for_match = Rc::clone(trees);

  functions
    .with(NAMESPACE, "test", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("regexp:test", &arguments, 2, Some(3))?;
      let text = arguments[0].string(context.model);
      let pattern = compile(&arguments[1].string(context.model), &flags(&arguments, 2, context)?)?;
      Ok(Value::Boolean(pattern.is_match(&text)))
    })
    .with(NAMESPACE, "replace", |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("regexp:replace", &arguments, 4, Some(4))?;
      let text = arguments[0].string(context.model);
      let written = flags(&arguments, 2, context)?;
      let pattern = compile(&arguments[1].string(context.model), &written)?;
      let replacement = arguments[3].string(context.model);
      // The replacement goes in as it stands. EXSLT says nothing about `$1` meaning a captured
      // group, and libxslt does not read one that way, so a `$` here is a dollar sign.
      let replaced = if written.global {
        pattern.replace_all(&text, regex::NoExpand(&replacement))
      } else {
        pattern.replace(&text, regex::NoExpand(&replacement))
      };
      Ok(Value::String(replaced.into_owned()))
    })
    .with(NAMESPACE, "match", move |arguments: Vec<Value<M::Node>>, context: &Context<'_, M>| {
      arity("regexp:match", &arguments, 2, Some(3))?;
      let text = arguments[0].string(context.model);
      let written = flags(&arguments, 2, context)?;
      let pattern = compile(&arguments[1].string(context.model), &written)?;

      // Two quite different answers under one name, which is EXSLT's design: with `g`, one
      // piece per match of the whole pattern; without it, the first match and then what each
      // of its groups captured.
      let pieces: Vec<String> = if written.global {
        pattern.find_iter(&text).map(|found| found.as_str().to_owned()).collect()
      } else {
        match pattern.captures(&text) {
          None => Vec::new(),
          Some(captures) => {
            captures.iter().map(|group| group.map(|group| group.as_str().to_owned()).unwrap_or_default()).collect()
          }
        }
      };
      as_matches(&pieces, &for_match, context)
    })
}

/// What a flags string asked for.
#[derive(Clone, Copy, Debug, Default)]
struct Flags {
  global: bool,
  case_insensitive: bool,
  multi_line: bool,
}

/// Reads the flags argument, if the call has one.
fn flags<M: Model>(arguments: &[Value<M::Node>], at: usize, context: &Context<'_, M>) -> Result<Flags> {
  let Some(value) = arguments.get(at) else { return Ok(Flags::default()) };
  read_flags(&value.string(context.model))
}

/// Reads a flags string.
fn read_flags(written: &str) -> Result<Flags> {
  let mut flags = Flags::default();
  for letter in written.chars() {
    match letter {
      'g' => flags.global = true,
      'i' => flags.case_insensitive = true,
      'm' => flags.multi_line = true,
      other => {
        let message = format!("the regular expression flag {other:?} is not one of g, i or m");
        return Err(Error::new(ErrorKind::Xslt, message));
      }
    }
  }
  Ok(flags)
}

/// Compiles a pattern, saying what was wrong with one that cannot be.
fn compile(pattern: &str, flags: &Flags) -> Result<Regex> {
  RegexBuilder::new(pattern).case_insensitive(flags.case_insensitive).multi_line(flags.multi_line).build().map_err(
    |error| {
      let message = format!("the regular expression {pattern:?} cannot be used: {error}");
      Error::new(ErrorKind::Xslt, message)
    },
  )
}

/// Builds a `match` element for each piece and puts the tree where the model can read it.
fn as_matches<M: Model>(
  pieces: &[String],
  trees: &Rc<dyn DocumentSource<M::Node>>,
  context: &Context<'_, M>,
) -> Result<Value<M::Node>> {
  let mut document = Document::new();
  let root = document.create_document_fragment();
  for piece in pieces {
    let element =
      document.create_element(MATCH).map_err(|error| Error::internal(format!("building a {MATCH}: {error}")))?;
    let text = document.create_text_node(piece);
    document.append_child(element, text).map_err(|error| Error::internal(format!("{error}")))?;
    document.append_child(root, element).map_err(|error| Error::internal(format!("{error}")))?;
  }

  let adopted = trees.adopt(document, root)?.ok_or_else(|| {
    let message = "regexp:match() answers with nodes, and needs somewhere to put them; register \
                   the EXSLT functions with register_with and a TreeSpace sharing the model's \
                   Documents handle"
      .to_owned();
    Error::new(ErrorKind::Xslt, message)
  })?;
  Ok(Value::NodeSet(context.model.children(adopted)))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn flags_are_read_and_an_unknown_one_is_refused() {
    let both = read_flags("gi").expect("two flags");
    assert!(both.global);
    assert!(both.case_insensitive);
    assert!(!both.multi_line);

    assert!(read_flags("").expect("none at all").global.eq(&false));
    assert!(read_flags("m").expect("one flag").multi_line);

    let refused = read_flags("x").expect_err("a flag nobody defined");
    assert!(refused.message().contains("g, i or m"), "{}", refused.message());
  }

  #[test]
  fn a_pattern_that_cannot_be_compiled_says_why() {
    let error = compile("(unclosed", &Flags::default()).expect_err("not a pattern");
    assert!(error.message().contains("cannot be used"), "{}", error.message());
  }

  #[test]
  fn backreferences_are_refused_rather_than_matched_wrongly() {
    // The price of a matcher that cannot be made to run for ever; being told is the point.
    assert!(compile(r"(a)\1", &Flags::default()).is_err());
    assert!(compile("(?=a)b", &Flags::default()).is_err());
  }
}
