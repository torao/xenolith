//! A source-independent view of an element's attributes.
//!
//! An element's attributes come from multiple places. The parser reports them from its input, a built tree holds them
//! on its element nodes, and a writer receives them when it is called. Code that consumes elements, a validator or a
//! push handler, works the same way, whatever the source. It receives the attributes through [`Attributes`], a
//! borrowing view over any backing that implements [`AttributeList`].
//!
//! A source implements [`AttributeList`] over its own storage, so [`Attributes`] presents the attributes without
//! copying them. The parser's attribute view and a tree's element node are two such backings.
//!

use crate::name::QName;

/// One attribute of an element, borrowed from whatever holds it.
///
/// The borrow lasts only as long as the [`Attributes`] view that yielded it. To keep the [`value`](Self::value) past
/// that, copy it with [`to_owned`](str::to_owned).
///
#[derive(Clone, Copy, Debug)]
pub struct AttributeRef<'a> {
  /// The attribute's name. An unprefixed attribute is in no namespace, never the default one.
  ///
  pub name: QName,

  /// The value after attribute-value normalization (XML 1.0 §3.3.3), and the tokenized collapse a DTD applies when the
  /// attribute has a tokenized type.
  ///
  pub value: &'a str,

  /// True when the attribute is a namespace declaration (`xmlns` or `xmlns:p`).
  ///
  pub declares_namespace: bool,
}

/// A list that holds an element's attributes in document order.
///
/// A source of events implements this over its own storage, letting an [`Attributes`] view read the attributes without
/// copying them. A caller does not use this trait directly. It wraps a backing in an [`Attributes`] with
/// [`Attributes::new`] and reads through that.
///
pub trait AttributeList {
  /// How many attributes there are, namespace declarations included.
  ///
  fn len(&self) -> usize;

  /// The attribute at `index` in document order, or `None` when `index` is out of range.
  ///
  fn get(&self, index: usize) -> Option<AttributeRef<'_>>;

  /// Whether there are no attributes.
  ///
  fn is_empty(&self) -> bool {
    self.len() == 0
  }
}

/// A borrowing view of an element's attributes, in document order.
///
/// It holds a reference to an [`AttributeList`], so it copies nothing and is cheap to pass by value. Iterate it with
/// [`iter`](Self::iter), or index it with [`get`](Self::get).
///
#[derive(Clone, Copy)]
pub struct Attributes<'a> {
  list: &'a dyn AttributeList,
}

impl<'a> Attributes<'a> {
  /// Wraps an [`AttributeList`] list as a view.
  ///
  #[must_use]
  pub fn new(list: &'a dyn AttributeList) -> Self {
    Self { list }
  }

  /// How many attributes there are, namespace declarations included.
  ///
  #[must_use]
  pub fn len(&self) -> usize {
    self.list.len()
  }

  /// Whether there are no attributes.
  ///
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.list.is_empty()
  }

  /// The attribute at `index` in document order, or `None` when `index` is out of range.
  ///
  #[must_use]
  pub fn get(&self, index: usize) -> Option<AttributeRef<'a>> {
    self.list.get(index)
  }

  /// The attribute with the specified name, or `None` when the attribute is not exist.
  ///
  pub fn get_by_name(&self, name: QName) -> Option<AttributeRef<'_>> {
    for i in 0..self.len() {
      let attr = self.get(i)?;
      if attr.name == name {
        return Some(attr);
      }
    }
    None
  }

  /// Iterates the attributes in document order.
  ///
  #[must_use]
  pub fn iter(&self) -> AttributeIter<'a> {
    AttributeIter { list: self.list, index: 0 }
  }
}

impl std::fmt::Debug for Attributes<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_list().entries(self.iter()).finish()
  }
}

impl<'a> IntoIterator for Attributes<'a> {
  type Item = AttributeRef<'a>;
  type IntoIter = AttributeIter<'a>;
  fn into_iter(self) -> AttributeIter<'a> {
    self.iter()
  }
}

/// Iterates an [`Attributes`] view in document order.
pub struct AttributeIter<'a> {
  list: &'a dyn AttributeList,
  index: usize,
}

impl std::fmt::Debug for AttributeIter<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AttributeIter").field("len", &self.list.len()).field("index", &self.index).finish()
  }
}

impl<'a> Iterator for AttributeIter<'a> {
  type Item = AttributeRef<'a>;

  fn next(&mut self) -> Option<AttributeRef<'a>> {
    let list = self.list;
    let item = list.get(self.index)?;
    self.index += 1;
    Some(item)
  }

  fn size_hint(&self) -> (usize, Option<usize>) {
    let remaining = self.list.len().saturating_sub(self.index);
    (remaining, Some(remaining))
  }
}

impl ExactSizeIterator for AttributeIter<'_> {}
