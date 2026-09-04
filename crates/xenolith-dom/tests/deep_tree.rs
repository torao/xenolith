//! Traversals must not recurse on tree depth.
//!
//! Nesting depth follows the input, and nothing caps a tree assembled by hand, so every walk in the crate keeps its
//! own stack and leaves call stack use flat. Each test here runs on a deliberately small thread stack, which one call
//! frame per level could not fit into. A walk that recursed would abort the process rather than fail this test, so a
//! crash in this file is the failure it reports.

use xenolith_dom::{Document, NodeId};

/// Levels of nesting to build. Recursion would spend one call frame per level, well past what `STACK` holds.
const DEPTH: usize = 5_000;

/// The thread stack each test runs on. Flat code needs very little of it.
const STACK: usize = 128 * 1024;

/// Runs `test` on a thread with a small stack.
fn on_a_small_stack(test: impl FnOnce() + Send + 'static) {
  std::thread::Builder::new().stack_size(STACK).spawn(test).expect("the thread starts").join().expect("no overflow");
}

/// Builds `<e><e>... deep ...</e></e>` nested `DEPTH` levels, and returns it with its root element.
fn deep_document() -> (Document, NodeId) {
  let mut doc = Document::new();
  let root = doc.create_element("e").expect("a legal name");
  doc.append_child(doc.document_node(), root).expect("a root element");
  let mut deepest = root;
  for _ in 1..DEPTH {
    let child = doc.create_element("e").expect("a legal name");
    doc.append_child(deepest, child).expect("a child");
    deepest = child;
  }
  let text = doc.create_text_node("deep");
  doc.append_child(deepest, text).expect("the text at the bottom");
  (doc, root)
}

#[test]
fn text_content_does_not_recurse_on_depth() {
  on_a_small_stack(|| {
    let (doc, root) = deep_document();
    assert_eq!(doc.text_content(root), "deep");
  });
}

#[test]
fn a_deep_clone_does_not_recurse_on_depth() {
  on_a_small_stack(|| {
    let (mut doc, root) = deep_document();
    let copy = doc.clone_node(root, true).expect("a deep copy");
    assert_ne!(copy, root);
    assert_eq!(doc.text_content(copy), "deep", "the copy carries the whole subtree");
  });
}

#[test]
fn descending_does_not_recurse_on_depth() {
  on_a_small_stack(|| {
    let (doc, root) = deep_document();
    // `get_elements_by_tag_name` walks every descendant.
    assert_eq!(doc.get_elements_by_tag_name("e").length(), DEPTH);
    assert_eq!(doc.node_name(root), "e");
  });
}

#[cfg(feature = "parse")]
#[test]
fn emitting_a_deep_tree_does_not_recurse_on_depth() {
  use xenolith_dom::DomSource;
  use xenolith_parser::sax::{EventSource, Handler, StartElementEvent};

  #[derive(Default)]
  struct CountElements(usize);
  impl Handler for CountElements {
    fn start_element(&mut self, _event: StartElementEvent<'_>) {
      self.0 += 1;
    }
  }

  on_a_small_stack(|| {
    let (doc, _root) = deep_document();
    let mut count = CountElements::default();
    DomSource::new(&doc).emit(&mut count).expect("emitted");
    assert_eq!(count.0, DEPTH);
  });
}
