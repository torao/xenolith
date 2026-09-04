//! A DOM tree for xenolith.
//!
//! # The arena structure
//!
//! <a id="arena"></a>
//! The tree is an **arena**: one [`Document`] owns every node, and they are identified by a small `Copy`-type
//! [`NodeId`]. A node is then cheap to hold and to compare. Unlike a graph of reference-counted objects
//! (`Rc<RefCell<Node>>`), there are no reference cycles through parent pointers and no per-node allocation.
//!
//! This structure does not reclaim storage. Once a node is created, it keeps its place for the life of the document,
//! whether or not it is ever attached, and removing a node detaches it without reclaiming it. This is the arena
//! trade-off: making and comparing nodes stay cheap, but the store only grows.
//!
//! This crate is designed for use cases where a tree is built and read, and the entire document is discarded once the
//! task is complete. When used this way, the document's memory usage is bounded by the tree you build. However,
//! caution is required for a long-lived document that is repeated creating and removing nodes. The internal storage
//! continues to grow as long as the document exists, so it can behave like a memory leak, though all memory is
//! released once the document is discarded. In such scenarios, rather than frequently swapping nodes in place,
//! you should limit the document's lifespan and rebuild it as necessary.
//!
//! # Traversal depth
//!
//! <a id="traversal"></a>
//! The nesting depth of the tree depends on the input. The parser imposes limits on the depth of the structure it
//! reads, but no such limits apply to the traversal of trees constructed using methods like
//! [`create_element`](Document::create_element) or [`append_child`](Document::append_child).
//!
//! The traversal processing in this crate does not use recursion based on depth. Recursion consumes one call stack
//! frame per level of depth, i.e., requiring O(d) stack space for a tree of depth d, which risks exhausting the stack
//! for extremely deep trees. In Rust, a stack overflow triggers an abnormal process termination (abort) rather than a
//! panic that a caller could catch, so the error cannot be reported via standard error-handling paths nor recovered
//! from by the caller. In contrast, the traversal implementation here follows links between nodes, keeping call stack
//! usage to O(1).
//!
//! # Reading and writing the tree
//!
//! This API complies with the W3C DOM specification and adopts equivalent method names (such as `node_type`,
//! `first_child`, `text_content`, and `append_child`); however, its internal structure is designed for an arena
//! rather than a graph of reference-counted objects. A [`NodeId`] serves as an index to identify the node within a
//! specific document, so accessing the document itself is required to read the node's contents:
//!
//! - **read** through [`Document`] with `&self` ([`node_type`](Document::node_type),
//!   [`first_child`](Document::first_child), and the rest), or through a [`NodeRef`], which bundles the document for a
//!   chained walk like `doc.node(id).first_child()`.
//! - **write** through [`Document`] with `&mut self`: the `create_*` methods make nodes,
//!   [`append_child`](Document::append_child) and [`insert_before`](Document::insert_before) join them, and
//!   [`remove_child`](Document::remove_child) takes them out.
//!
//! Reading requires a shared reference to the document in addition to the [`NodeId`], whereas modifying the tree
//! requires `&mut Document`, which implies exclusive access. A typical usage pattern involves building the tree with
//! exclusive access first, and then sharing it as an `Arc<Document>` for subsequent operations that are primarily
//! read-only.
//!
//! # Examples
//!
//! ```
//! use xenolith_dom::{Document, NodeType};
//!
//! // <doc><p>Hello</p></doc>
//! let mut doc = Document::new();
//! let root = doc.create_element("doc")?;
//! let p = doc.create_element("p")?;
//! let text = doc.create_text_node("Hello");
//! doc.append_child(p, text)?;
//! doc.append_child(root, p)?;
//! doc.append_child(doc.document_node(), root)?;
//!
//! assert_eq!(doc.document_element(), Some(root));
//! assert_eq!(doc.node(root).first_child().unwrap().node_name(), "p");
//! assert_eq!(doc.text_content(root), "Hello");
//! # Ok::<(), xenolith_dom::DomException>(())
//! ```
//!
//! # Feature flags
//!
//! The following features are both off by default because a tree built and read in memory needs neither.
//!
//! - `parse`: the `build` module, which turns parsed XML into a tree, and `DomSource`, which turns a tree back into
//!   parser events. It also enables the parser's XML Base and `xml:id` support, so a built tree carries base URIs and
//!   marks its ID-typed attributes.
//! - `encodings`: encodings beyond UTF-8/UTF-16/US-ASCII/ISO-8859-1 while building from XML. It takes effect only
//!   together with `parse`, since a tree built by hand decodes nothing.
//!
//! # Specifications
//!
//! This crate is implemented from the following documents, at the URLs with the date:
//!
//! - [DOM Level 3 Core], W3C Recommendation 7 April 2004. The interfaces this follows: the node kinds and their
//!   `nodeType` codes, the tree operations, and the [`ExceptionCode`] system. Where an interface's shape suits a graph
//!   of reference-counted objects rather than an arena, this crate redesigns it rather than imitating it (see
//!   [the arena](crate#arena)).
//! - [XML Base (Second Edition)], W3C Recommendation 28 January 2009, for [`base_uri`](Document::base_uri).
//!
//! [DOM Level 3 Core]: https://www.w3.org/TR/2004/REC-DOM-Level-3-Core-20040407/
//! [XML Base (Second Edition)]: https://www.w3.org/TR/2009/REC-xmlbase-20090128/

#[cfg(feature = "parse")]
pub mod build;
mod collection;
mod document;
#[cfg(feature = "parse")]
mod emit;
mod exception;
mod node;
mod noderef;
mod walk;

pub use collection::{NamedNodeMap, NodeList};
pub use document::Document;
#[cfg(feature = "parse")]
pub use emit::DomSource;
pub use exception::{DomException, ExceptionCode, Result};
pub use node::{NodeId, NodeType};
pub use noderef::NodeRef;
pub use walk::{Visit, Walk};
