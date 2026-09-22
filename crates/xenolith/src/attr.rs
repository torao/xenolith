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
//! An [`Attribute`] holds its parts itself, for an attribute that has to outlive whatever it was borrowed from. A list
//! of them is such a backing too.
//!

#[cfg(test)]
mod test;

use crate::error::Location;
use crate::name::{self, XMLNS_PREFIX};

/// One representation of an attribute of an element, borrowed from whatever holds it.
///
/// The borrow lasts only as long as the [`Attributes`] view that yielded it. To keep the [`value`](Self::value) past
/// that, copy it with [`to_owned`](str::to_owned).
///
/// The name arrives as its parts. Two attributes are the same attribute when their [`namespace`](Self::namespace) and
/// [`local`](Self::local) agree, whatever prefix each was written with.
///
#[derive(Clone, Debug)]
pub struct AttributeRef<'a> {
  /// The prefix the attribute was written with, or `None` when it was written without one.
  pub prefix: Option<&'a str>,

  /// The local part of the name.
  pub local: &'a str,

  /// The namespace the prefix is bound to. An unprefixed attribute is in no namespace, never the default one.
  pub namespace: Option<&'a str>,

  /// The value after attribute-value normalization (XML 1.0 §3.3.3), and the tokenized collapse a DTD applies when the
  /// attribute has a tokenized type.
  pub value: &'a str,

  /// Where the name begins, so a fault in the name is reported at the attribute rather than at its element.
  ///
  /// It is unknown for an attribute that was never written in a document: one a tree holds, one supplied by a DTD
  /// default, or one a program built.
  pub location: Location,

  /// Where the value begins, past the quotation mark, so a fault inside the value is reported where it is.
  ///
  /// The value reported has been normalized, so a position within it maps back to the document only as far as that
  /// normalization left it: a reference in the value was replaced by what it stands for, and a line end became one
  /// character. It is unknown wherever [`location`](Self::location) is.
  pub value_location: Location,
}

impl AttributeRef<'_> {
  /// The name as it was written: `prefix:local`, or `local` when it has no prefix.
  #[must_use]
  pub fn lexical(&self) -> String {
    name::lexical(self.prefix, self.local)
  }

  /// Whether the attribute is a namespace declaration: its name is `xmlns`, or has the prefix `xmlns`.
  #[must_use]
  pub fn declares_namespace(&self) -> bool {
    is_declaration(self.prefix, self.local)
  }
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

  /// The attribute with this expanded name, or `None` when the element has no such attribute.
  ///
  /// The prefix is not part of the comparison, since it is the namespace and the local part that identify an
  /// attribute. Pass `None` for `namespace` to look for one written without a prefix.
  ///
  pub fn get_by_name(&self, namespace: Option<&str>, local: &str) -> Option<AttributeRef<'_>> {
    self.iter().find(|attr| attr.namespace == namespace && attr.local == local)
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

/// An attribute of an element that holds its parts itself.
///
/// It is what an [`AttributeRef`] becomes when the attribute has to outlive what it was borrowed from, as the
/// attributes of an owned [`Event`](crate::event::Event) do. A list of them is an [`AttributeList`] of its own, so it
/// is read through an [`Attributes`] view like any other backing.
///
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
  /// The prefix the attribute was written with, or `None` when it was written without one.
  pub prefix: Option<String>,

  /// The local part of the name.
  pub local: String,

  /// The namespace the prefix is bound to. An unprefixed attribute is in no namespace, never the default one.
  pub namespace: Option<String>,

  /// The value, normalized as [`AttributeRef::value`] is.
  pub value: String,

  /// Where the name begins, as [`AttributeRef::location`] gives it.
  pub location: Location,

  /// Where the value begins, as [`AttributeRef::value_location`] gives it.
  pub value_location: Location,
}

impl Attribute {
  /// Whether the attribute is a namespace declaration: its name is `xmlns`, or has the prefix `xmlns`.
  #[must_use]
  pub fn declares_namespace(&self) -> bool {
    is_declaration(self.prefix.as_deref(), &self.local)
  }

  /// This attribute in the borrowed form every consumer reads.
  ///
  #[must_use]
  pub fn as_attribute_ref(&self) -> AttributeRef<'_> {
    AttributeRef {
      prefix: self.prefix.as_deref(),
      local: &self.local,
      namespace: self.namespace.as_deref(),
      value: &self.value,
      location: self.location.clone(),
      value_location: self.value_location.clone(),
    }
  }
}

impl From<AttributeRef<'_>> for Attribute {
  fn from(attribute: AttributeRef<'_>) -> Self {
    Self {
      prefix: attribute.prefix.map(ToOwned::to_owned),
      local: attribute.local.to_owned(),
      namespace: attribute.namespace.map(ToOwned::to_owned),
      value: attribute.value.to_owned(),
      location: attribute.location.clone(),
      value_location: attribute.value_location.clone(),
    }
  }
}

/// Whether a name given as its parts declares a namespace. It is computed from the name rather than held beside it, so
/// no attribute can be marked as a declaration its name does not make it.
fn is_declaration(prefix: Option<&str>, local: &str) -> bool {
  match prefix {
    Some(prefix) => prefix == XMLNS_PREFIX,
    None => local == XMLNS_PREFIX,
  }
}

/// Owned attributes, in document order, lent out one at a time as the borrowed form.
///
impl AttributeList for Vec<Attribute> {
  fn len(&self) -> usize {
    self.as_slice().len()
  }

  fn get(&self, index: usize) -> Option<AttributeRef<'_>> {
    self.as_slice().get(index).map(Attribute::as_attribute_ref)
  }
}
