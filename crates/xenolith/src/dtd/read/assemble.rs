//! Assembling a DTD from the pieces it arrives in.
//!
//! The text of a DTD comes from several places: the internal subset from the `DOCTYPE`, the external subset from the
//! resource the `DOCTYPE` refers to, and each external parameter entity from the resource its declaration names.
//! [`DtdAssembly`] holds the text gathered so far and runs the parser over it, stopping whenever a piece is missing and
//! going on once the caller supplies it.
//!
//! Fetching is left to the caller, which knows how to reach a resource. The document parser stops and asks its own
//! driver; [`DtdReader`](super::DtdReader) reads the resource through its resolver.
//!

use crate::error::{Location, Result};
use crate::name::NamePool;

use super::{Dtd, DtdOutcome, ExternalPe, Layout, parse_dtd};

/// The DTD text gathered so far, and the parse over it.
///
/// The internal subset comes first and the external subset after it, the order XML reads them in: a declaration in the
/// internal subset takes precedence over one of the same name in the external subset, because the first declaration
/// is binding.
///
/// It also records where each piece of text came from. That decides which text is external: the external subset, and
/// the text of an external parameter entity even where it is referenced from the internal subset. External text is
/// read by the external subset's rules, and a document with `standalone="yes"` may not depend on what it declares. It
/// also gives the line and column of an error, and the resource a relative system identifier is resolved against.
/// Each piece is added with the [`Location`] its text begins at, for that.
///
/// # Examples
///
/// A caller that can fetch what is missing hands [`complete`](Self::complete) a closure and gets the DTD back:
///
/// ```
/// use xenolith::error::Location;
/// use xenolith::name::NamePool;
/// use xenolith::dtd::DtdAssembly;
///
/// // What the `DOCTYPE` carried, then what the resource it referred to held.
/// let mut assembly = DtdAssembly::with_internal_subset("<!ELEMENT note (#PCDATA)>", Location::new());
/// assembly.add_external_subset("<!ENTITY % common SYSTEM 'urn:common'>%common;", Location::new());
///
/// let mut pool = NamePool::new();
/// let dtd = assembly.complete(&mut pool, |pe| {
///   assert_eq!(pe.system_id, "urn:common");
///   Ok(Some(("<!ELEMENT extra EMPTY>".to_owned(), Location::new().with_system_id("urn:common"))))
/// })?;
///
/// assert!(dtd.has_element(pool.get("note").expect("from the internal subset")));
/// assert!(dtd.has_element(pool.get("extra").expect("from the entity that was fetched")));
/// # Ok::<(), xenolith::Error>(())
/// ```
///
/// A caller that cannot fetch inside a closure, such as the document parser, which must return to its driver to have
/// a resource read, steps instead. Each [`advance`](Self::advance) either finishes or says what is missing, and the
/// caller calls it again once it has supplied that piece:
///
/// ```
/// use xenolith::error::Location;
/// use xenolith::name::NamePool;
/// use xenolith::dtd::{DtdAssembly, DtdOutcome};
///
/// let mut assembly = DtdAssembly::new();
/// assembly.add_external_subset("<!ENTITY % common SYSTEM 'urn:common'>%common;", Location::new());
///
/// let mut pool = NamePool::new();
///
/// let DtdOutcome::NeedExternalPe(pe) = assembly.advance(&mut pool)? else { panic!("it stops here") };
/// assert_eq!(pe.name, "common");
///
/// // The caller reaches the resource however it can, then hands the text over and carries on.
/// assembly.provide_parameter_entity("<!ELEMENT extra EMPTY>", Location::new().with_system_id("urn:common"));
///
/// let DtdOutcome::Complete(dtd) = assembly.advance(&mut pool)? else { panic!("nothing is missing now") };
/// assert!(dtd.has_element(pool.get("extra").expect("from the entity that was fetched")));
/// # Ok::<(), xenolith::Error>(())
/// ```
///
#[derive(Clone, Debug, Default)]
pub struct DtdAssembly {
  /// The internal subset followed by the external subset, with entity replacements spliced in as they arrive.
  buf: String,
  /// Where the text of `buf` came from: the boundary of the internal subset, and the ranges spliced in from parameter
  /// entities.
  layout: Layout,
  /// The parameter entity the last pass stopped at, waiting for its replacement text.
  pending: Option<ExternalPe>,
}

impl DtdAssembly {
  /// Creates an assembly with nothing in it yet.
  ///
  #[must_use]
  pub fn new() -> Self {
    Self::default()
  }

  /// Creates an assembly holding `subset` as the internal subset, whose text begins at `at` in the document.
  ///
  #[must_use]
  pub fn with_internal_subset(subset: &str, at: Location) -> Self {
    Self { buf: subset.to_owned(), layout: Layout::with_internal_subset(subset.len(), at), pending: None }
  }

  /// Appends the external subset after the internal one, separated from it by a line break if there is one.
  ///
  /// `text` is the subset's decoded text, with any text declaration at its head already removed, and `at` is where
  /// that text begins in its resource, the resource's system identifier included.
  ///
  pub fn add_external_subset(&mut self, text: &str, at: Location) {
    if !self.buf.is_empty() {
      self.buf.push('\n');
    }
    let start = self.buf.len();
    self.buf.push_str(text);
    self.layout.add_external_subset(start..self.buf.len(), at);
  }

  /// True if no text has been gathered.
  ///
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.buf.is_empty()
  }

  /// The parameter entity the last [`advance`](Self::advance) stopped for, if it stopped for one.
  ///
  #[must_use]
  pub fn pending(&self) -> Option<&ExternalPe> {
    self.pending.as_ref()
  }

  /// Splices the pending parameter entity's replacement text over the reference that asked for it.
  ///
  /// `text` is the entity's decoded text, with any text declaration at its head already removed, and `at` is where that
  /// text begins in its resource, the resource's system identifier included.
  ///
  /// Where the reference stands as markup, a space is added on each side, as XML 1.0 §4.4.8 requires, so the text
  /// cannot fuse with the tokens around it. Where it stands in an entity value literal, the text is included as literal
  /// data (§4.4.5): no spaces are added, and its quotes are escaped as character references so that they do not close
  /// the literal. The text is recorded as external wherever the reference stood: it is read by the external subset's
  /// rules, and a standalone document may not depend on what it declares.
  ///
  /// # Panics
  ///
  /// If no [`advance`](Self::advance) stopped for a parameter entity.
  ///
  pub fn provide_parameter_entity(&mut self, text: &str, at: Location) {
    let pe = self.pending.take().expect("no parameter entity was pending");
    let (replacement, content) = if pe.in_literal {
      let escaped = text.replace('"', "&#34;").replace('\'', "&#39;");
      let len = escaped.len();
      (escaped, 0..len)
    } else {
      (format!(" {text} "), 1..1 + text.len())
    };
    let replaced = self.buf[pe.at..pe.end].chars().count();
    self.layout.splice(pe.at..pe.end, replacement.len(), content, replaced, Some(at));
    self.buf.replace_range(pe.at..pe.end, &replacement);
  }

  /// Forgets the pending parameter entity, for a caller that declined to fetch it.
  ///
  /// The reference stays in the buffer, so the next [`advance`](Self::advance) stops at it again.
  pub fn discard_pending(&mut self) {
    self.pending = None;
  }

  /// Runs one pass over the text gathered so far.
  ///
  /// A pass either finishes the DTD or stops at an external parameter entity, which the caller fetches and hands over
  /// through [`provide_parameter_entity`](Self::provide_parameter_entity) before calling again. Each pass starts again
  /// from the beginning of the buffer, so all that survives between passes is the text and the record of where it came
  /// from.
  ///
  /// # Errors
  ///
  /// The well-formedness error if the DTD is malformed, located where the fault is in the document or resource it was
  /// read from.
  ///
  pub fn advance(&mut self, pool: &mut NamePool) -> Result<DtdOutcome> {
    let outcome = parse_dtd(&mut self.buf, &mut self.layout, pool)?;
    if let DtdOutcome::NeedExternalPe(pe) = &outcome {
      self.pending = Some(pe.clone());
    }
    Ok(outcome)
  }

  /// Runs passes until the DTD is complete, with `fetch` supplying each external parameter entity.
  ///
  /// `fetch` returns the replacement text of the entity it is given and where that text begins, as
  /// [`provide_parameter_entity`](Self::provide_parameter_entity) takes them, or `None` to decline it. A declined
  /// entity fails the DTD with a well-formedness error that names it, located at the reference.
  ///
  /// # Errors
  ///
  /// The well-formedness error if the DTD is malformed or an entity is declined, or whatever `fetch` returns.
  ///
  pub fn complete<F>(&mut self, pool: &mut NamePool, mut fetch: F) -> Result<Dtd>
  where
    F: FnMut(&ExternalPe) -> Result<Option<(String, Location)>>,
  {
    loop {
      match self.advance(pool)? {
        DtdOutcome::Complete(dtd) => return Ok(*dtd),
        DtdOutcome::NeedExternalPe(pe) => match fetch(&pe)? {
          Some((text, at)) => self.provide_parameter_entity(&text, at),
          None => {
            self.discard_pending();
            let message = format!("the parameter entity \"{}\" could not be resolved", pe.name);
            return Err(crate::error::Error::well_formedness(message).at(pe.location));
          }
        },
      }
    }
  }
}
