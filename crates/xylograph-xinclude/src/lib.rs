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
//! `parse="text"` includes it as a text node, decoded per the `encoding` attribute. Selecting a
//! sub-resource with `xpointer` is not yet supported — an `xi:include` that asks for one falls
//! back, or fails if it has no fallback.
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
