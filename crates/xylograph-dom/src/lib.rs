//! A DOM tree for xylograph.
//!
//! The tree is an **arena**: every node lives in one [`Document`]'s `Vec`, named by a small
//! `Copy` [`NodeId`]. That is what makes a node cheap to hold and to compare, and document order
//! an integer comparison — the shape the later XPath and XSLT layers want. It is the choice
//! `Rc<RefCell<Node>>` is not: no reference cycles through parent pointers, no per-node
//! allocation.
//!
//! # Reading and writing the tree
//!
//! The API follows the W3C DOM where the names carry over — `node_type`, `first_child`,
//! `text_content`, `append_child` — but is shaped for an arena rather than for a graph of
//! reference-counted objects (see `ROADMAP.md`, decision 3). A node is an index, so reading it
//! needs the document too:
//!
//! - **read** through [`Document`] with `&self` — [`node_type`](Document::node_type),
//!   [`first_child`](Document::first_child), and the rest — or through a [`NodeRef`], which
//!   bundles the document for a chained walk like `doc.node(id).first_child()`.
//! - **write** through [`Document`] with `&mut self` — the `create_*` methods make nodes,
//!   [`append_child`](Document::append_child) and [`insert_before`](Document::insert_before) join
//!   them, [`remove_child`](Document::remove_child) takes them out.
//!
//! A node held on its own is a read-only handle; to mutate the tree you need `&mut Document`,
//! which is unique access. Build a tree with that unique access, then share it as `Arc<Document>`
//! for the read-heavy phases that follow.
//!
//! # Examples
//!
//! ```
//! use xylograph_dom::{Document, NodeType};
//!
//! // <doc><p>Hello</p></doc>
//! let mut doc = Document::new();
//! let root = doc.create_element("doc")?;
//! let p = doc.create_element("p")?;
//! let text = doc.create_text_node("Hello");
//! doc.append_child(p, text)?;
//! doc.append_child(root, p)?;
//! doc.append_child(doc.root(), root)?;
//!
//! assert_eq!(doc.document_element(), Some(root));
//! assert_eq!(doc.node(root).first_child().unwrap().node_name(), "p");
//! assert_eq!(doc.text_content(root), "Hello");
//! # Ok::<(), xylograph_dom::DomException>(())
//! ```

#[cfg(feature = "parse")]
pub mod build;
mod collection;
mod document;
mod exception;
mod node;
mod noderef;

pub use collection::{NamedNodeMap, NodeList};
pub use document::Document;
pub use exception::{DomException, ExceptionCode, Result};
pub use node::{NodeId, NodeType};
pub use noderef::NodeRef;
