//! Defines utility functions for testing.
//!
pub mod compatibility;

use std::env;
use std::fs::{create_dir_all, remove_dir_all, create_dir};
use std::path::{Path, PathBuf};

pub struct TempDir(PathBuf);

impl AsRef<Path> for TempDir {
  fn as_ref(&self) -> &Path {
    &self.0
  }
}

impl Drop for TempDir {
  fn drop(&mut self) {
    if self.0.exists() {
      remove_dir_all(self.0.as_path()).unwrap();
    }
  }
}

/// Creates a new empty temporary directory that can be used for the specified test case. The directory will be deleted
/// when the scope of the returned `Path`-like value ends.
///
pub fn temp_dir(test_case: &str) -> TempDir {
  let mut dir = env::temp_dir();
  dir.push(format!("{}-{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")));
  dir.push(test_case);
  create_dir_all(&dir).unwrap();
  for i in 0..=u16::MAX {
    let name = format!("{:04X}", i);
    dir.push(name);
    if create_dir(&dir).is_ok() {
      return TempDir(dir);
    }
    dir.pop();
  }
  panic!("The temporary directory namespace is full: {:?}", dir)
}

// use std::rc::{Rc, Weak};

// trait Node {
//   fn parent_node(&self) -> Option<Rc<NodeRef>>;
//   fn child(&self, index: usize) -> Option<Rc<NodeRef>>;
// }

// #[derive(Clone)]
// struct NodeRef {
//   parent: Weak<NodeRef>,
//   children: Vec<Rc<NodeRef>>,
//   entity: NodeEntity,
// }

// impl Node for NodeRef {
//   fn parent_node(&self) -> Option<Rc<NodeRef>> {
//     self.parent.upgrade()
//   }

//   fn child(&self, index: usize) -> Option<Rc<NodeRef>> {
//     if index < self.children.len() {
//       Some(self.children[index].clone())
//     } else {
//       None
//     }
//   }
// }

// impl AsRef<NodeEntity> for NodeRef {
//   fn as_ref(&self) -> &NodeEntity {
//     &self.entity
//   }
// }

// #[derive(Clone)]
// enum NodeEntity {
//   Element(Element),
//   Text(Element),
// }

// #[derive(Clone)]
// struct Element();

// #[derive(Clone)]
// struct Text();

// #[test]
// fn test() {}
