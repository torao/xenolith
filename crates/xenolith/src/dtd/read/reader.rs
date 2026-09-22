//! Reading a DTD kept in a resource of its own, apart from any document.
//!

use std::io::Read;

use crate::error::{Error, Location, Result};
use crate::io::decl;
use crate::io::resolve::{EntityRequest, RequestKind, UriResolver};
use crate::io::stream::CharStream;
use crate::name::NamePool;

use super::Dtd;
use super::assemble::DtdAssembly;

/// Reads a DTD kept in a resource of its own, apart from any document.
///
/// A DTD usually arrives through the `DOCTYPE` of a document. This reads one with no document, such as a DTD to use as
/// a [`DtdSchema`](crate::dtd::DtdSchema). The whole source is read as an external subset: it is decoded, a text
/// declaration at its head is removed, and each external parameter entity it references is fetched.
///
/// It returns the DTD with the [`NamePool`] its names are interned in, since a declaration is keyed by an interned name
/// that means nothing without the pool.
///
/// An external parameter entity is fetched only through a resolver given by [`with_resolver`](Self::with_resolver).
/// Without one, a reference to an external parameter entity is an error, the same default a document reader takes
/// against XML external entity (XXE) attacks.
///
/// # Examples
///
/// ```
/// use xenolith::dtd::DtdReader;
///
/// let (dtd, pool) = DtdReader::new("<!ELEMENT note (#PCDATA)>".as_bytes()).read()?;
/// assert!(dtd.has_element(pool.get("note").expect("declared")));
/// # Ok::<(), xenolith::Error>(())
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

  /// Reads a DTD from `source`, giving the system identifier it was read from.
  ///
  /// The identifier is the base a relative system identifier in the DTD is resolved against, and the location errors
  /// are reported at. Give it whenever the DTD refers to another resource.
  ///
  #[must_use]
  pub fn with_system_id(source: R, system_id: &str) -> Self {
    Self { source, system_id: Some(system_id.to_owned()), resolver: None }
  }

  /// Sets the resolver that fetches the external parameter entities the DTD references.
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
  /// An I/O error if reading a source fails, an encoding error if one cannot be decoded, or a well-formedness error if
  /// the DTD is malformed or references an external parameter entity that is not fetched, because there is no
  /// resolver or the resolver declines it.
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
    let (text, at) = decode(&mut self.source, self.system_id.as_deref(), "the DTD")?;

    // A DTD read on its own is all external subset: none of it came from a document's `DOCTYPE`.
    let mut assembly = DtdAssembly::new();
    assembly.add_external_subset(&text, at);

    let mut resolver = self.resolver;
    let system_id = self.system_id;
    assembly.complete(pool, move |pe| fetch_parameter_entity(&mut resolver, pe, system_id.as_deref()))
  }
}

/// Fetches one external parameter entity through `resolver`, returning its decoded text and where that text begins,
/// or `None` when the resolver declines it.
///
/// A relative system identifier is resolved against the resource the entity was declared in, or against `base`, the
/// system identifier of the DTD, where that resource has none.
fn fetch_parameter_entity(
  resolver: &mut Option<Box<dyn UriResolver>>,
  pe: &super::ExternalPe,
  base: Option<&str>,
) -> Result<Option<(String, Location)>> {
  let Some(resolver) = resolver.as_deref_mut() else {
    // Refused, with the message naming the call that allows it.
    let what = format!("the parameter entity \"{}\"", pe.name);
    let message = format!("{what} is external; call DtdReader::with_resolver to allow this");
    return Err(Error::well_formedness(message).at(pe.location.clone()));
  };
  let request = EntityRequest::new(
    Some(pe.name.clone()),
    pe.public_id.clone(),
    pe.system_id.clone(),
    pe.base.clone().or_else(|| base.map(ToOwned::to_owned)),
    RequestKind::ParameterEntity,
  );
  let resolved = resolver.resolve(&request).map_err(|error| error.or_at(pe.location.clone()))?;
  let Some(mut source) = resolved else { return Ok(None) };
  let what = format!("the entity \"{}\"", pe.name);
  decode(&mut source, request.resolved_uri().as_deref(), &what).map(Some)
}

/// Reads `source` to its end, decodes it with the encoding it declares or is detected as, and removes a text
/// declaration at its head. Returns the text with the location it begins at. `what` names the source in an I/O error
/// message.
fn decode(source: &mut dyn Read, system_id: Option<&str>, what: &str) -> Result<(String, Location)> {
  let mut bytes = Vec::new();
  source.read_to_end(&mut bytes).map_err(|e| Error::io(format!("cannot read {what}: {e}")).caused_by(e))?;
  let mut stream = CharStream::new();
  if let Some(id) = system_id {
    stream = stream.with_system_id(id);
  }
  stream.feed(&bytes, true)?;
  decl::strip_text_declaration(&mut stream)?;
  Ok((stream.remainder().to_owned(), stream.location()))
}
