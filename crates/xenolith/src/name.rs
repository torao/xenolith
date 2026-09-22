//! Interned names and qualified names in a [`NamePool`].
//!
//! Identical element names, attribute names, prefixes, and namespace URIs appear repeatedly within a single document
//! and remain unchanged for the duration of the document. By interning these into [`NameId`], we can reduce the cost
//! of generating duplicate names, perform integer equality comparison on them, and keep the size of tree nodes small.
//!

#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::chars;
use crate::error::{Error, Result};

/// The namespace name associated with the `xml` prefix, as defined by the Namespace in XML.
pub const XML_NS_URI: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace name of namespace declaration attributes themselves.
pub const XMLNS_NS_URI: &str = "http://www.w3.org/2000/xmlns/";

/// The prefix bound to [`XML_NS_URI`] by definition. It does not require declaration and cannot be bound to any other
/// namespace.
pub const XML_PREFIX: &str = "xml";

/// The prefix used to declare a namespace, or the name used to declare the default namespace, i.e., `xmlns:p` declares
/// the prefix `p`, while `xmlns` declares the default namespace; thus, this is not a prefix acting as part of a name.
pub const XMLNS_PREFIX: &str = "xmlns";

/// Reserved names.
const RESERVED_NAMES: [&str; 5] = [
  "",           // 0: EMPTY
  XML_PREFIX,   // 1: XML
  XMLNS_PREFIX, // 2: XMLNS
  XML_NS_URI,   // 3: XML_NS
  XMLNS_NS_URI, // 4: XMLNS_NS
];

/// How many names are reserved, as an index bound.
const RESERVED_LEN: u32 = RESERVED_NAMES.len() as u32;

/// The pool every pool descends from. The reserved names are its own, so their handles are the same in every pool.
const ROOT: NonZeroU32 = NonZeroU32::MIN;

/// A handle to a string interned in a [`NamePool`].
///
/// A handle represents a string's index in the pool where it was originally interned. A pool created via
/// [`fork`](NamePool::fork) inherits all strings held by the parent pool at the time of creation, retaining their
/// original indices; consequently, a handle that referred to a valid string in the parent pool remains valid in the
/// forked pool, resolving to the same string. Strings interned after the fork belong to the specific pool where they
/// were interned, and their handles are valid only within that pool. If a handle is passed to a pool where it is not
/// valid, the pool rejects it rather than interpreting it as a different string.
///
/// This value is a temporary ID in memory and should not be treated as persistent.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NameId {
  /// The pool where the string was initially interned.
  ///
  /// When the inner type `T` of `Option<T>` contains a field with a value that is never actually used, the Rust
  /// compiler represents `None` using that unused value instead of adding extra space to distinguish between `Some`
  /// and `None` (a technique known as niche optimization). By using `NonZeroU32` for this field, `NameId` gains an
  /// impossible value `pool == 0`, ensuring that `Option<NameId>` does not exceed the size of `NameId` itself
  /// (whereas a raw `u32` would require an additional 4 bytes).
  ///
  pool: NonZeroU32,
  /// The index of the string within a pool, or within any pools derived from that pool after the string was interned.
  index: u32,
}

impl NameId {
  /// The empty string, interned in every pool.
  pub const EMPTY: Self = Self::reserved(0);
  /// `xml`, interned in every pool.
  pub const XML: Self = Self::reserved(1);
  /// `xmlns`, interned in every pool.
  pub const XMLNS: Self = Self::reserved(2);
  /// [`XML_NS_URI`], interned in every pool.
  pub const XML_NS: Self = Self::reserved(3);
  /// [`XMLNS_NS_URI`], interned in every pool.
  pub const XMLNS_NS: Self = Self::reserved(4);

  /// The handle to the reserved name at `index`.
  const fn reserved(index: u32) -> Self {
    Self { pool: ROOT, index }
  }

  /// The underlying index used as a dense array key for a set of handles within a single pool.
  #[must_use]
  pub const fn index(self) -> usize {
    self.index as usize
  }
}

impl fmt::Display for NameId {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}#{}", self.pool, self.index)
  }
}

/// A table for interning (registering and deduplicating) names.
///
/// Reserved names are interned in advance, so their [`NameId`]s are compile-time constants. For details, refer to
/// [`NameId::EMPTY`] and the constants adjacent to it.
///
/// # Examples
///
/// ```
/// use xenolith::{NameId, NamePool, XML_NS_URI};
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
///
#[derive(Debug)]
pub struct NamePool {
  /// The unique ID of this pool.
  id: NonZeroU32,
  /// The pool IDs from which this pool originated are listed in chronological order, accompanied by the number of
  /// names held (i.e., final index + 1) at the time the subsequent pool branched off from each. Any index lower than
  /// one of these values belongs to the corresponding source pool. The root pool holding the reserved name is always
  /// listed first.
  lineage: Vec<(NonZeroU32, u32)>,
  names: Vec<Box<str>>,
  index: HashMap<Box<str>, u32>,
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
    let mut pool = Self { id: next_pool_id(), lineage: Vec::new(), names: Vec::new(), index: HashMap::new() };
    for reserved in RESERVED_NAMES {
      pool.intern(reserved);
    }
    // The reserved names will be interned into a root pool that is virtually shared with all other pools.
    pool.lineage.push((ROOT, RESERVED_LEN));
    pool
  }

  /// Creates a pool containing all the names held by this pool, and from that point on, it expands independently.
  ///
  /// Any handle already issued by this pool remains valid after the fork; since the same handle is returned for a
  /// given name in the forked pool, that name can still be read via the original pool. Any name newly interned to
  /// either pool after the fork belongs solely to that pool.
  ///
  /// This is not [`Clone`] because the pool that interns a name after it is forked is no longer commutative.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::NamePool;
  ///
  /// let mut parent = NamePool::new();
  /// let item = parent.intern("item");
  ///
  /// let mut child = parent.fork();
  /// assert_eq!(child.get("item"), Some(item)); // the parent's handle, from the child
  /// assert_eq!(child.resolve(item), "item");
  ///
  /// // After the fork, each pool's new names are its own.
  /// let left = parent.intern("left");
  /// let right = child.intern("right");
  /// assert!(!child.owns(left) && !parent.owns(right));
  /// ```
  ///
  #[must_use]
  pub fn fork(&self) -> Self {
    let held = u32::try_from(self.names.len()).expect("name pool overflow");
    let mut lineage = self.lineage.clone();
    // A pool that does not hold its own names is omitted.
    if lineage.last().is_none_or(|&(_, end)| end < held) {
      lineage.push((self.id, held));
    }
    Self { id: next_pool_id(), lineage, names: self.names.clone(), index: self.index.clone() }
  }

  /// Interns the `name`. If it already exists, return the existing ID.
  ///
  /// # Panics
  ///
  /// If the pooled string exceeds the maximum value of a u32.
  ///
  pub fn intern(&mut self, name: &str) -> NameId {
    if let Some(&index) = self.index.get(name) {
      return NameId { pool: self.origin(index), index };
    }
    // This new name is for this pool and appears after all indices owned by the ancestor.
    let index = u32::try_from(self.names.len()).expect("name pool overflow");
    let boxed: Box<str> = name.into();
    self.names.push(boxed.clone());
    self.index.insert(boxed, index);
    NameId { pool: self.id, index }
  }

  /// Returns the id of `name` if it has been interned.
  #[must_use]
  pub fn get(&self, name: &str) -> Option<NameId> {
    self.index.get(name).map(|&index| NameId { pool: self.origin(index), index })
  }

  /// True if `id` is a handle resolved by this pool (either one issued by this pool or one issued by the source pool
  /// before this pool was forked).
  #[must_use]
  pub fn owns(&self, id: NameId) -> bool {
    id.index() < self.names.len() && id.pool == self.origin(id.index)
  }

  /// Returns the string corresponding to `id`.
  ///
  /// # Panics
  ///
  /// If this pool does not [`own`](Self::owns) this `id` — meaning the `id` was issued by an unrelated pool, or by the
  /// pool from which this one was forked, but *after* the fork.
  ///
  #[must_use]
  pub fn resolve(&self, id: NameId) -> &str {
    assert!(
      self.owns(id),
      "{id} is not a handle of pool {}: it came from an unrelated pool, or from an ancestor after the fork",
      self.id
    );
    &self.names[id.index()]
  }

  /// The pool that interned the name at `index` first, and whose handle for it every descendant gives back.
  fn origin(&self, index: u32) -> NonZeroU32 {
    self.lineage.iter().find(|&&(_, end)| index < end).map_or(self.id, |&(pool, _)| pool)
  }

  /// Interns `name` after checking it against `NCName`.
  ///
  /// # Examples
  ///
  /// ```
  /// use xenolith::{Error, NamePool};
  ///
  /// let mut pool = NamePool::new();
  /// let local = pool.intern_ncname("template")?;
  /// assert_eq!(pool.resolve(local), "template");
  ///
  /// let err = pool.intern_ncname("xsl:template").unwrap_err();
  /// assert!(matches!(err, Error::Name { .. }));
  /// # Ok::<(), xenolith::Error>(())
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

  /// True if the pool does not contain any names other than the reserved name.
  #[must_use]
  pub fn is_empty(&self) -> bool {
    self.names.len() <= RESERVED_NAMES.len()
  }
}

/// Generates the identity of a new pool. The identity is a `NonZeroU32` that is guaranteed to be unique within the
/// process, and it wraps around after 2^32 pools have been created.
fn next_pool_id() -> NonZeroU32 {
  // The counter starts at 2. A value of 0 serves as a niche (a reserved empty value), allowing the size of
  // `Option<NameId>` to remain the same as that of `NameId`, while a value of 1 is assigned to the root associated
  // with the reserved name. Although this mechanism does not completely eliminate identifier collisions — since the
  // counter wraps around and identifiers are re-used once the number of pools in the process reaches 2^32 — it is
  // intended to prevent virtually all instances of misuse.
  static NEXT: AtomicU32 = AtomicU32::new(2);
  let raw = NEXT.fetch_add(1, Ordering::Relaxed);
  NonZeroU32::new(raw.max(2)).expect("two is not zero")
}

/// An expanded name consists of a namespace and a local part.
///
/// This is the identifier used by namespace-aware XML features, such as XPath and XSLT, to perform their comparisons.
/// A prefix is intentionally omitted because it is an alias for a namespace. If different prefixes are bound to the
/// same namespace, they have the same expanded name.
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
    Self::new(None, local)
  }
}

/// A qualified name with a prefix that appears in the document.
///
/// Although the prefix is not part of the name identifier itself, it must be preserved because serialization,
/// `name()`, and attribute values of the QName type all depend on it.
///
/// # Examples
///
/// Two names that are bound to the same namespace but have different prefixes are considered to be the same name:
///
/// ```
/// use xenolith::{NamePool, QName};
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

/// The lexical form of a name is represented by the components `prefix:local` (if a prefix is present) or `local`
/// (if no prefix is present).
///
/// The event holds the name as its constituent parts rather than as an internalized [`QName`]. Consequently, this
/// determines the document's written form and the method for schema lookups keyed by the lexical name.
///
/// # Examples
///
/// ```
/// use xenolith::name::lexical;
///
/// assert_eq!(lexical(Some("xsl"), "template"), "xsl:template");
/// assert_eq!(lexical(None, "item"), "item");
/// ```
#[must_use]
pub fn lexical(prefix: Option<&str>, local: &str) -> String {
  match prefix {
    Some(prefix) => format!("{prefix}:{local}"),
    None => local.to_owned(),
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
