//! XInclude 1.0 processing for xenolith.
//!
//! XInclude is a post-processing pass over a [DOM](xenolith_dom): it finds `xi:include`
//! elements and replaces each with the resource it names, so a document can be assembled from
//! parts. This crate expands a tree in place, on top of the base URIs the
//! [builder](xenolith_dom::build) recorded — `href` resolves against the base URI in effect
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
//! XInclude elements. This crate does not validate; run `xenolith-validate` over the result.
//!
//! # Examples
//!
//! ```
//! use std::collections::HashMap;
//! use xenolith_core::Error;
//! use xenolith_dom::build;
//! use xenolith_xinclude::{Loader, XInclude};
//!
//! // A loader backed by a map, standing in for a filesystem or a catalogue.
//! struct Map(HashMap<&'static str, &'static [u8]>);
//! impl Loader for Map {
//!   fn load(&mut self, uri: &str) -> Result<Vec<u8>, Error> {
//!     self.0.get(uri).map(|b| b.to_vec()).ok_or_else(|| Error::io("not found"))
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
//! # Ok::<(), xenolith_core::Error>(())
//! ```

//! # Specifications
//!
//! Implemented from these documents, at the versions linked — the dated URLs, so that what was
//! read while writing this can still be found:
//!
//! - [XInclude 1.0 (Second Edition)] — W3C Recommendation 15 November 2006. (There is no third
//!   edition; the second is the latest.) `xi:include`, `xi:fallback`, and the base URI and
//!   language fixups.
//! - [XPointer Framework] — W3C Recommendation 25 March 2003, for the shorthand pointer and the
//!   shape of a scheme-based one.
//! - [XPointer `element()` Scheme] — W3C Recommendation 25 March 2003.
//! - [XPointer `xmlns()` Scheme] — W3C Recommendation 25 March 2003.
//! - [XML Base (Second Edition)] — W3C Recommendation 28 January 2009, which `href` resolution
//!   and the base URI fixup rest on.
//!
//! [XInclude 1.0 (Second Edition)]: https://www.w3.org/TR/2006/REC-xinclude-20061115/
//! [XPointer Framework]: https://www.w3.org/TR/2003/REC-xptr-framework-20030325/
//! [XPointer `element()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-element-20030325/
//! [XPointer `xmlns()` Scheme]: https://www.w3.org/TR/2003/REC-xptr-xmlns-20030325/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/

mod include;
mod loader;
mod xpointer;

pub use include::XInclude;
pub use loader::Loader;
