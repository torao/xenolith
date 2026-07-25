//! XInclude 1.0 processing for xylograph.
//!
//! XInclude is a post-processing pass over a [DOM](xylograph_dom): it finds `xi:include`
//! elements and replaces each with the resource it names, so a document can be assembled from
//! parts. This crate expands a tree in place, on top of the base URIs the
//! [builder](xylograph_dom::build) recorded — `href` resolves against the base URI in effect
//! where the `xi:include` sits.
//!
//! Fetching a resource is I/O, and the same attack surface as an external entity, so it is not
//! built in: you pass a [`Loader`], and nothing is fetched without one. On a failed inclusion an
//! `xi:fallback` child, if present, is used instead; without one the failure is fatal.
//! Inclusion loops are detected, and the depth and count of inclusions are bounded.
//!
//! `parse="xml"` (the default) parses the resource and includes its document element;
//! `parse="text"` includes it as a text node, decoded per the `encoding` attribute. An
//! `xpointer` narrows the inclusion to one element of the resource (a shorthand pointer, or the
//! `element()` scheme); with no `href` it selects from the document that holds the `xi:include`.
//!
//! An included element keeps the base URI and language it had in its source: base URI fixup
//! writes an `xml:base`, and language fixup an `xml:lang`, where these differ from what is in
//! effect at the inclusion point. Both are on by default and can be turned off.
//!
//! # Order with validation
//!
//! XInclude is a pass over a tree, separate from parsing and from validation, so the caller
//! chooses the order. The usual one is **parse → expand → validate**: expand first, then
//! validate the assembled document, so a schema sees the whole thing. Validating *before*
//! expansion checks only the skeleton with its `xi:include` elements in place, which a schema
//! written for the assembled document will reject; do that only with a schema that permits the
//! XInclude elements. This crate does not validate; run `xylograph-validate` over the result.
//!
//! # Examples
//!
//! ```
//! use std::collections::HashMap;
//! use xylograph_core::Error;
//! use xylograph_dom::build;
//! use xylograph_xinclude::{Loader, XInclude};
//!
//! // A loader backed by a map, standing in for a filesystem or a catalogue.
//! struct Map(HashMap<&'static str, &'static [u8]>);
//! impl Loader for Map {
//!   fn load(&mut self, uri: &str) -> Result<Vec<u8>, Error> {
//!     self.0.get(uri).map(|b| b.to_vec()).ok_or_else(|| Error::new(xylograph_core::ErrorKind::Io, "not found"))
//!   }
//! }
//!
//! let mut doc = build::parse_with_system_id(
//!   "<doc><xi:include href='part.xml' xmlns:xi='http://www.w3.org/2001/XInclude'/></doc>".as_bytes(),
//!   "file:///doc.xml",
//! )?;
//! let mut loader = Map([("file:///part.xml", &b"<p>included</p>"[..])].into_iter().collect());
//! XInclude::new().with_base_fixup(false).expand(&mut doc, &mut loader)?;
//!
//! let root = doc.document_element().unwrap();
//! assert_eq!(doc.node_name(doc.first_child(root).unwrap()), "p");
//! assert_eq!(doc.text_content(root), "included");
//! # Ok::<(), xylograph_core::Error>(())
//! ```

mod include;
mod loader;
mod xpointer;

pub use include::XInclude;
pub use loader::Loader;
