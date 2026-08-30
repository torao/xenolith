//! Interned names and qualified names.
//!
//! Identical element names, attribute names, prefixes, and namespace URIs appear repeatedly within a single document
//! and remain unchanged for the duration of the document. By interning these into [`NameId`], we can reduce the cost
//! of generating duplicate names, perform integer equality comparison on them, and keep the size of tree nodes small.
//!

use std::collections::HashMap;
use std::fmt;

use crate::chars;
use crate::error::{Error, Result};

/// The namespace name associated with the `xml` prefix, as defined by the Namespace in XML.
pub const XML_NS_URI: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace name of namespace declaration attributes themselves.
pub const XMLNS_NS_URI: &str = "http://www.w3.org/2000/xmlns/";

/// Reserved names.
const RESERVED_NAMES: [&str; 5] = [
  "",           // 0: EMPTY
  "xml",        // 1: XML
  "xmlns",      // 2: XMLNS
  XML_NS_URI,   // 3: XML_NS
  XMLNS_NS_URI, // 4: XMLNS_NS
];

/// A handle to a string interned in a [`NamePool`].
///
/// Represents the identifier of a string within a specific pool. This value is a temporary ID in memory and is not
/// persistent. It cannot be mixed IDs from different pools.
///
/// This value is a temporary ID in memory and should not be treat as persistent.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameId(u32);

impl NameId {
  /// The empty string, interned in every pool.
  pub const EMPTY: Self = Self(0);
  /// `xml`, interned in every pool.
  pub const XML: Self = Self(1);
  /// `xmlns`, interned in every pool.
  pub const XMLNS: Self = Self(2);
  /// [`XML_NS_URI`], interned in every pool.
  pub const XML_NS: Self = Self(3);
  /// [`XMLNS_NS_URI`], interned in every pool.
  pub const XMLNS_NS: Self = Self(4);

  /// The underlying index, for use as a dense array key.
  #[must_use]
  pub const fn index(self) -> usize {
    self.0 as usize
  }
}

/// An interning table for names.
///
/// Reserved names are pre-interned so that their [`NameId`] becomes a compile-time constant. For details, see
/// [`NameId::EMPTY`] and the constant beside it.
///
/// # Examples
///
/// ```
/// use xenolith_core::{NameId, NamePool, XML_NS_URI};
///
/// let mut pool = NamePool::new();
/// let item = pool.intern("item");
/// assert_eq!(pool.intern("item"), item); // interning is idempotent
/// assert_eq!(pool.resolve(item), "item");
///
/// // Names not yet seen are absent rather than created.
/// assert_eq!(pool.get("other"), None);
///
/// // The names fixed by the specifications are always present.
/// assert_eq!(pool.resolve(NameId::XML), "xml");
/// assert_eq!(pool.resolve(NameId::XML_NS), XML_NS_URI);
/// ```
#[derive(Debug)]
pub struct NamePool {
  names: Vec<Box<str>>,
  index: HashMap<Box<str>, NameId>,
}

impl Default for NamePool {
  fn default() -> Self {
    Self::new()
  }
}

impl NamePool {
  /// Creates a pool containing only the reserved names.
  #[must_use]
  pub fn new() -> Self {
    let mut pool = Self { names: Vec::new(), index: HashMap::new() };
    for reserved in RESERVED_NAMES {
      pool.intern(reserved);
    }
    debug_assert_eq!(pool.intern(XMLNS_NS_URI), NameId::XMLNS_NS);
    pool
  }

  /// Interns `name`, returning its existing id if it is already present.
  pub fn intern(&mut self, name: &str) -> NameId {
    if let Some(&id) = self.index.get(name) {
      return id;
    }
    let id = NameId(u32::try_from(self.names.len()).expect("name pool overflow"));
    let boxed: Box<str> = name.into();
    self.names.push(boxed.clone());
    self.index.insert(boxed, id);
    id
  }

  /// Returns the id of `name` if it has been interned.
  #[must_use]
  pub fn get(&self, name: &str) -> Option<NameId> {
    self.index.get(name).copied()
  }

  /// Returns the string behind `id`.
  ///
  /// # Panics
  ///
  /// If `id` did not come from this pool.
  ///
  #[must_use]
  pub fn resolve(&self, id: NameId) -> &str {
    &self.names[id.index()]
  }

  /// Interns `name` after checking it against `NCName`.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith_core::{Error, NamePool};
  ///
  /// let mut pool = NamePool::new();
  /// let local = pool.intern_ncname("template")?;
  /// assert_eq!(pool.resolve(local), "template");
  ///
  /// let err = pool.intern_ncname("xsl:template").unwrap_err();
  /// assert!(matches!(err, Error::Name { .. }));
  /// # Ok::<(), xenolith_core::Error>(())
  /// ```
  ///
  /// # Errors
  ///
  /// Returns [`Error::Name`] if `name` is not an `NCName`.
  ///
  pub fn intern_ncname(&mut self, name: &str) -> Result<NameId> {
    if chars::is_ncname(name) { Ok(self.intern(name)) } else { Err(Error::name(format!("not an NCName: {name:?}"))) }
  }

  /// Number of distinct names interned, including the reserved ones.
  #[must_use]
  pub fn len(&self) -> usize {
    self.names.len()
  }

  /// True if the pool holds nothing but the reserved names.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.names.len() <= RESERVED_NAMES.len()
  }
}

/// An expanded name consists of a namespace and a local part.
///
/// This is the identifier used by namespace-aware XML features, such as XPath and XSLT, to perform their comparisons.
/// A prefix is intentionally omitted because it is an alias for a namespace. If different prefixes are bound to the
/// same namespace, they have the same expanded name.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExpandedName {
  /// Namespace, or `None` for a name in no namespace.
  pub namespace: Option<NameId>,
  /// Local part; always an `NCName`.
  pub local: NameId,
}

impl ExpandedName {
  /// Creates an expanded name.
  #[must_use]
  pub const fn new(namespace: Option<NameId>, local: NameId) -> Self {
    Self { namespace, local }
  }

  /// Creates a name in no namespace.
  #[must_use]
  pub const fn local(local: NameId) -> Self {
    Self { namespace: None, local }
  }
}

/// A qualified name as it appears in the document, retaining the prefix.
///
/// Although the prefix is not part of the name identifier, it must be retained because serialization, `name()`, and
/// attribute values of type QName all depend on this prefix.
///
/// # Examples
///
/// Two names that are bound to the same namespace but have different prefixes are considered to be the same name:
///
/// ```
/// use xenolith_core::{NamePool, QName};
///
/// let mut pool = NamePool::new();
/// let ns = pool.intern("http://www.w3.org/1999/XSL/Transform");
/// let local = pool.intern("template");
///
/// let xsl = QName::new(Some(pool.intern("xsl")), Some(ns), local);
/// let t = QName::new(Some(pool.intern("t")), Some(ns), local);
///
/// assert_ne!(xsl, t); // they serialize differently
/// assert_eq!(xsl.expanded, t.expanded); // but match the same templates
/// assert_eq!(xsl.to_lexical(&pool), "xsl:template");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QName {
  /// Prefix, or `None` for an unprefixed name.
  pub prefix: Option<NameId>,
  /// Expanded name.
  pub expanded: ExpandedName,
}

impl QName {
  /// Creates a qualified name.
  #[must_use]
  pub const fn new(prefix: Option<NameId>, namespace: Option<NameId>, local: NameId) -> Self {
    Self { prefix, expanded: ExpandedName::new(namespace, local) }
  }

  /// The local part.
  #[must_use]
  pub const fn local(&self) -> NameId {
    self.expanded.local
  }

  /// The namespace name, if any.
  #[must_use]
  pub const fn namespace(&self) -> Option<NameId> {
    self.expanded.namespace
  }

  /// Renders the lexical form (`prefix:local`, or `local` when unprefixed).
  #[must_use]
  pub fn to_lexical(&self, pool: &NamePool) -> String {
    match self.prefix {
      Some(p) => format!("{}:{}", pool.resolve(p), pool.resolve(self.local())),
      None => pool.resolve(self.local()).to_owned(),
    }
  }
}

/// Wraps a [`QName`] with its pool so it can be formatted.
#[derive(Debug)]
pub struct DisplayQName<'a>(pub &'a QName, pub &'a NamePool);

impl fmt::Display for DisplayQName<'_> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(&self.0.to_lexical(self.1))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reserved_names_have_stable_ids() {
    let pool = NamePool::new();
    assert_eq!(pool.resolve(NameId::EMPTY), "");
    assert_eq!(pool.resolve(NameId::XML), "xml");
    assert_eq!(pool.resolve(NameId::XMLNS), "xmlns");
    assert_eq!(pool.resolve(NameId::XML_NS), XML_NS_URI);
    assert_eq!(pool.resolve(NameId::XMLNS_NS), XMLNS_NS_URI);
    assert_eq!(pool.len(), RESERVED_NAMES.len());
    assert!(pool.is_empty());
  }

  #[test]
  fn interning_is_idempotent() {
    let mut pool = NamePool::new();
    let a = pool.intern("item");
    let b = pool.intern("item");
    assert_eq!(a, b);
    assert_eq!(pool.get("item"), Some(a));
    assert_eq!(pool.get("missing"), None);
    assert_eq!(pool.len(), 6);
    assert!(!pool.is_empty());
  }

  #[test]
  fn ncname_interning_rejects_bad_names() {
    let mut pool = NamePool::new();
    assert!(pool.intern_ncname("item").is_ok());
    let err = pool.intern_ncname("p:item").unwrap_err();
    assert!(matches!(err, Error::Name { .. }));
    assert!(pool.intern_ncname("").is_err());
  }

  #[test]
  fn expanded_name_ignores_prefix() {
    let mut pool = NamePool::new();
    let ns = pool.intern("urn:example");
    let local = pool.intern("item");
    let a = QName::new(Some(pool.intern("a")), Some(ns), local);
    let b = QName::new(Some(pool.intern("b")), Some(ns), local);
    assert_ne!(a, b);
    assert_eq!(a.expanded, b.expanded);
    assert_eq!(a.to_lexical(&pool), "a:item");
    assert_eq!(QName::new(None, None, local).to_lexical(&pool), "item");
    assert_eq!(DisplayQName(&b, &pool).to_string(), "b:item");
  }
}
