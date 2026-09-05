//! Reading a DTD from a source of its own.
//!

use std::io::Read;

use xenolith_core::decl;
use xenolith_core::error::{Error, Location, Result};
use xenolith_core::name::NamePool;
use xenolith_core::resolve::{EntityRequest, RequestKind, UriResolver};
use xenolith_core::stream::CharStream;

use crate::Dtd;
use crate::assemble::DtdAssembly;

/// Reads a DTD that stands on its own, apart from any document.
///
/// A DTD usually arrives with the document that declares it in its `DOCTYPE`. One kept in its own file has no document,
/// and this reads it directly: it decodes the source, skips the text declaration at its head, and fetches any external
/// parameter entity the DTD references.
///
/// It returns the DTD with the [`NamePool`] that interns its names. The two travel together because a declaration is
/// keyed by an interned name, which says nothing without the pool that interned it.
///
/// Resolving an external parameter entity needs a resolver, which [`with_resolver`](Self::with_resolver) supplies.
/// Without one, it refuses references, the same default a document reader takes to mitigate the XML external entity
/// (XXE) attack surface.
///
/// # Examples
///
/// ```
/// use xenolith_dtd::DtdReader;
///
/// let (dtd, pool) = DtdReader::new("<!ELEMENT note (#PCDATA)>".as_bytes()).read()?;
/// assert!(dtd.has_element(pool.get("note").expect("declared")));
/// # Ok::<(), xenolith_core::Error>(())
/// ```
///
pub struct DtdReader<R> {
  source: R,
  system_id: Option<String>,
  resolver: Option<Box<dyn UriResolver>>,
}

impl<R: Read> std::fmt::Debug for DtdReader<R> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DtdReader")
      .field("system_id", &self.system_id)
      .field("resolver", &self.resolver.is_some())
      .finish_non_exhaustive()
  }
}

impl<R: Read> DtdReader<R> {
  /// Reads a DTD from `source`.
  #[must_use]
  pub fn new(source: R) -> Self {
    Self { source, system_id: None, resolver: None }
  }

  /// Reads a DTD from `source`, with the identifier it was fetched from already known.
  ///
  /// A relative reference inside the DTD resolves against this, so give it whenever the DTD refers to another
  /// resource.
  ///
  #[must_use]
  pub fn with_system_id(source: R, system_id: &str) -> Self {
    Self { source, system_id: Some(system_id.to_owned()), resolver: None }
  }

  /// Supplies the resolver that fetches any external parameter entity the DTD references.
  ///
  #[must_use]
  pub fn with_resolver(mut self, resolver: impl UriResolver + 'static) -> Self {
    self.resolver = Some(Box::new(resolver));
    self
  }

  /// Reads the DTD, returning it with the pool its names are interned in.
  ///
  /// # Errors
  ///
  /// The parse error if the DTD is malformed, an I/O error if reading the source fails, or a well-formedness error if
  /// the DTD references an external parameter entity that no resolver supplies.
  ///
  pub fn read(self) -> Result<(Dtd, NamePool)> {
    let mut pool = NamePool::new();
    let dtd = self.read_into(&mut pool)?;
    Ok((dtd, pool))
  }

  /// Reads the DTD, interning its names into `pool`.
  ///
  /// Use this to read several DTDs into one pool, or to read one into a pool that already holds names.
  ///
  /// # Errors
  ///
  /// As [`read`](Self::read).
  ///
  pub fn read_into(mut self, pool: &mut NamePool) -> Result<Dtd> {
    let base = match &self.system_id {
      Some(id) => Location::unknown().with_system_id(id.clone()),
      None => Location::unknown(),
    };
    let text = decode(&mut self.source, self.system_id.as_deref(), "the DTD")?;

    // A DTD read on its own is all external subset: nothing here came from a document's `DOCTYPE`.
    let mut assembly = DtdAssembly::new();
    assembly.add_external_subset(&text);

    let mut resolver = self.resolver;
    let system_id = self.system_id;
    assembly.complete(pool, &base, move |pe| fetch_parameter_entity(&mut resolver, pe, system_id.as_deref()))
  }
}

/// Fetches one external parameter entity through `resolver`, returning its text, or `None` when it is declined.
fn fetch_parameter_entity(
  resolver: &mut Option<Box<dyn UriResolver>>,
  pe: &crate::ExternalPe,
  base: Option<&str>,
) -> Result<Option<String>> {
  let Some(resolver) = resolver.as_deref_mut() else {
    // Refused rather than resolved, and the message gives the opt-in so a caller knows how to allow it.
    let what = format!("the parameter entity \"{}\"", pe.name);
    return Err(Error::well_formedness(format!("{what} is external; call DtdReader::with_resolver to allow this")));
  };
  let request = EntityRequest::new(
    Some(pe.name.clone()),
    pe.public_id.clone(),
    pe.system_id.clone(),
    base.map(ToOwned::to_owned),
    RequestKind::ParameterEntity,
  );
  let Some(mut source) = resolver.resolve(&request)? else { return Ok(None) };
  let text = decode(&mut source, request.resolved_uri().as_deref(), &format!("the entity \"{}\"", pe.name))?;
  Ok(Some(text))
}

/// Reads `source` to its end and decodes it, stepping over a text declaration at the head.
fn decode(source: &mut dyn Read, system_id: Option<&str>, what: &str) -> Result<String> {
  let mut bytes = Vec::new();
  source.read_to_end(&mut bytes).map_err(|e| Error::io(format!("cannot read {what}: {e}")).caused_by(e))?;
  let mut stream = CharStream::new();
  if let Some(id) = system_id {
    stream = stream.with_system_id(id);
  }
  stream.feed(&bytes, true)?;
  decl::strip_text_declaration(&mut stream)?;
  Ok(stream.remainder().to_owned())
}
