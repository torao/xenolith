//! Assembling a DTD from the pieces it arrives in.
//!
//! A DTD is rarely one piece of text. An internal subset comes from the `DOCTYPE`, an external subset from a resource
//! the declaration refers to, and a parameter entity from wherever its own declaration points. [`DtdAssembly`] holds
//! the text gathered so far and drives [`parse_dtd`](crate::parse_dtd) over it, stopping whenever a piece is still
//! missing and resuming once the caller supplies it.
//!
//! Fetching is the caller's business, since only it knows how to reach a resource. A document parser answers by
//! pausing its own read; [`DtdReader`](crate::DtdReader) answers by reading the resource itself.
//!

use xenolith_core::error::{Location, Result};
use xenolith_core::name::NamePool;

use crate::{Dtd, DtdOutcome, ExternalPe, parse_dtd};

/// The DTD text gathered so far and the parse over it.
///
/// The internal subset comes first and the external subset after it, which is the order XML gives them: a declaration
/// in the internal subset takes precedence over one of the same name in the external subset, and the parser keeps the
/// first it meets. The parser tracks the boundary between the two because a document with `standalone="yes"` may not
/// depend on what the external half declares.
///
/// # Examples
///
/// A caller that can fetch what is missing hands [`complete`](Self::complete) a closure and gets the DTD back:
///
/// ```
/// use xenolith_core::error::Location;
/// use xenolith_core::name::NamePool;
/// use xenolith_dtd::DtdAssembly;
///
/// // What the `DOCTYPE` carried, then what the resource it referred to held.
/// let mut assembly = DtdAssembly::with_internal_subset("<!ELEMENT note (#PCDATA)>");
/// assembly.add_external_subset("<!ENTITY % common SYSTEM 'urn:common'>%common;");
///
/// let mut pool = NamePool::new();
/// let dtd = assembly.complete(&mut pool, &Location::unknown(), |pe| {
///   assert_eq!(pe.system_id, "urn:common");
///   Ok(Some("<!ELEMENT extra EMPTY>".to_owned()))
/// })?;
///
/// assert!(dtd.has_element(pool.get("note").expect("from the internal subset")));
/// assert!(dtd.has_element(pool.get("extra").expect("from the entity that was fetched")));
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
/// A caller that cannot stop to fetch, a document parser that must hand control back to its own driver among them,
/// steps instead. Each [`advance`](Self::advance) either finishes or says what is missing, and the caller resumes
/// once it has that piece:
///
/// ```
/// use xenolith_core::error::Location;
/// use xenolith_core::name::NamePool;
/// use xenolith_dtd::{DtdAssembly, DtdOutcome};
///
/// let mut assembly = DtdAssembly::new();
/// assembly.add_external_subset("<!ENTITY % common SYSTEM 'urn:common'>%common;");
///
/// let mut pool = NamePool::new();
/// let at = Location::unknown();
///
/// let DtdOutcome::NeedExternalPe(pe) = assembly.advance(&mut pool, &at)? else { panic!("it stops here") };
/// assert_eq!(pe.name, "common");
///
/// // The caller reaches the resource however it can, then hands the text over and carries on.
/// assembly.provide_parameter_entity("<!ELEMENT extra EMPTY>");
///
/// let DtdOutcome::Complete(dtd) = assembly.advance(&mut pool, &at)? else { panic!("nothing is missing now") };
/// assert!(dtd.has_element(pool.get("extra").expect("from the entity that was fetched")));
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
#[derive(Clone, Debug, Default)]
pub struct DtdAssembly {
  /// The internal subset followed by the external subset, with entity replacements spliced in as they arrive.
  buf: String,
  /// Byte length of the internal subset within `buf`, which a splice inside it moves.
  internal_len: usize,
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

  /// Creates an assembly holding `subset` as the internal subset.
  ///
  #[must_use]
  pub fn with_internal_subset(subset: &str) -> Self {
    Self { buf: subset.to_owned(), internal_len: subset.len(), pending: None }
  }

  /// Adds the external subset after the internal one.
  ///
  pub fn add_external_subset(&mut self, text: &str) {
    if !self.buf.is_empty() {
      self.buf.push('\n');
    }
    self.buf.push_str(text);
  }

  /// Whether any text has been gathered.
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

  /// Splices a parameter entity's replacement text in place of the reference that asked for it.
  ///
  /// The text is surrounded by spaces, as XML requires of a parameter entity expanded in the DTD, so a replacement
  /// cannot fuse with the tokens around it. Where the reference stood in the internal subset, the boundary moves with
  /// it so the two halves stay correctly divided.
  ///
  /// # Panics
  ///
  /// If no [`advance`](Self::advance) stopped for a parameter entity.
  ///
  pub fn provide_parameter_entity(&mut self, text: &str) {
    let pe = self.pending.take().expect("no parameter entity was pending");
    let replacement = format!(" {text} ");
    if pe.at < self.internal_len {
      let removed = pe.end.min(self.internal_len) - pe.at;
      self.internal_len = self.internal_len - removed + replacement.len();
    }
    self.buf.replace_range(pe.at..pe.end, &replacement);
  }

  /// Forgets a pending parameter entity, for a caller that declined to fetch it.
  pub fn discard_pending(&mut self) {
    self.pending = None;
  }

  /// Runs one pass over the text gathered so far.
  ///
  /// A pass either finishes the DTD or stops at an external parameter entity, which the caller fetches and hands back
  /// through [`provide_parameter_entity`](Self::provide_parameter_entity) before calling again. Each pass restarts
  /// from the beginning of the buffer, so nothing has to survive the pause but the text itself.
  ///
  /// # Errors
  ///
  /// The parse error if the DTD is malformed.
  ///
  pub fn advance(&mut self, pool: &mut NamePool, base: &Location) -> Result<DtdOutcome> {
    let outcome = parse_dtd(&mut self.buf, &mut self.internal_len, pool, base)?;
    if let DtdOutcome::NeedExternalPe(pe) = &outcome {
      self.pending = Some(pe.clone());
    }
    Ok(outcome)
  }

  /// Runs passes until the DTD is complete, with `fetch` supplying each external parameter entity.
  ///
  /// `fetch` returns the replacement text for the entity it is given, or `None` to decline it, which leaves the
  /// reference unresolved and fails the parse where it stands.
  ///
  /// # Errors
  ///
  /// The parse error if the DTD is malformed, or whatever `fetch` reports.
  ///
  pub fn complete<F>(&mut self, pool: &mut NamePool, base: &Location, mut fetch: F) -> Result<Dtd>
  where
    F: FnMut(&ExternalPe) -> Result<Option<String>>,
  {
    loop {
      match self.advance(pool, base)? {
        DtdOutcome::Complete(dtd) => return Ok(*dtd),
        DtdOutcome::NeedExternalPe(pe) => match fetch(&pe)? {
          Some(text) => self.provide_parameter_entity(&text),
          None => {
            self.discard_pending();
            return Err(
              xenolith_core::error::Error::well_formedness(format!(
                "the parameter entity \"{}\" could not be resolved",
                pe.name
              ))
              .at(base.clone()),
            );
          }
        },
      }
    }
  }
}
