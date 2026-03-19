//! XML Namespace URI, prefix types, and scope-stack resolver.
//!
//! Provides lightweight types for namespace URIs/prefixes and an
//! [`NsResolver`] that tracks namespace declarations as the parser walks
//! in and out of elements.
//!
//! This crate is `no_std` + `alloc`.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// An interned namespace URI (e.g. `"http://www.w3.org/1999/xhtml"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NsUri(Box<str>);

impl NsUri {
    /// Create from a string slice.
    pub fn new(uri: &str) -> Self {
        Self(uri.into())
    }

    /// The URI as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A namespace prefix (e.g. `"svg"`), or the default namespace when absent.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NsPrefix(Box<str>);

impl NsPrefix {
    /// Create from a string slice.
    pub fn new(prefix: &str) -> Self {
        Self(prefix.into())
    }

    /// The prefix as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A resolved XML name: an optional namespace URI paired with a local name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QName {
    /// Namespace URI, or `None` if the name has no namespace.
    pub ns: Option<NsUri>,
    /// Local (unprefixed) name.
    pub local: Box<str>,
}

impl QName {
    /// Create a name with no namespace.
    pub fn local(name: &str) -> Self {
        Self {
            ns: None,
            local: name.into(),
        }
    }

    /// Create a namespaced name.
    pub fn namespaced(ns: NsUri, local: &str) -> Self {
        Self {
            ns: Some(ns),
            local: local.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// NsError
// ---------------------------------------------------------------------------

/// Errors from namespace resolution.
#[derive(Debug, PartialEq)]
pub enum NsError {
    /// A prefix was used that has no binding in the current scope.
    UnboundPrefix(Box<str>),
}

impl core::fmt::Display for NsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NsError::UnboundPrefix(p) => write!(f, "unbound namespace prefix: {p}"),
        }
    }
}

// ---------------------------------------------------------------------------
// NsResolver
// ---------------------------------------------------------------------------

/// A namespace scope stack.
///
/// Tracks `xmlns` declarations as the parser descends into and out of
/// elements, resolving prefixed names to their full `{uri}local` form.
///
/// # How it works
///
/// ```text
/// <root xmlns="http://example.com"          ← push_element → scope [("", "http://example.com")]
///   xmlns:svg="http://www.w3.org/2000/svg"> ←              + [("svg", "http://www.w3.org/2000/svg")]
///   <svg:circle/>                           ← resolve("svg") → Some("http://www.w3.org/2000/svg")
/// </root>                                   ← pop_element → both bindings gone
/// ```
///
/// The resolver is initialised with the two built-in bindings that the XML
/// Namespaces spec defines without requiring any declaration:
///
/// | Prefix  | URI |
/// |---------|-----|
/// | `xml`   | `http://www.w3.org/XML/1998/namespace` |
/// | `xmlns` | `http://www.w3.org/2000/xmlns/` |
pub struct NsResolver {
    /// Stack of scopes.  Index 0 holds the built-in bindings.  Each subsequent
    /// entry holds the bindings declared on one element.
    ///
    /// Each scope is a list of `(prefix, uri)` pairs.  An empty prefix `""`
    /// represents the default namespace (`xmlns="..."`).
    scopes: Vec<Vec<(Box<str>, NsUri)>>,
}

impl NsResolver {
    /// Create a resolver pre-loaded with the built-in XML namespace bindings.
    pub fn new() -> Self {
        // These two bindings are always in scope — they never need declaring.
        let builtins = alloc::vec![
            (
                Box::from("xml"),
                NsUri::new("http://www.w3.org/XML/1998/namespace"),
            ),
            (
                Box::from("xmlns"),
                NsUri::new("http://www.w3.org/2000/xmlns/"),
            ),
        ];
        Self {
            scopes: alloc::vec![builtins],
        }
    }

    /// Enter an element's scope.
    ///
    /// Pass every `(attribute_name, attribute_value)` pair for the element —
    /// this method extracts the `xmlns` and `xmlns:prefix` declarations and
    /// registers them.  Non-namespace attributes are silently ignored.
    ///
    /// Must be paired with a later call to [`pop_element`].
    ///
    /// [`pop_element`]: NsResolver::pop_element
    pub fn push_element<'a>(&mut self, attrs: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut bindings: Vec<(Box<str>, NsUri)> = Vec::new();
        for (name, value) in attrs {
            if name == "xmlns" {
                // Default namespace: xmlns="http://..."
                bindings.push((Box::from(""), NsUri::new(value)));
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                // Prefixed namespace: xmlns:svg="http://..."
                bindings.push((Box::from(prefix), NsUri::new(value)));
            }
        }
        self.scopes.push(bindings);
    }

    /// Leave the current element's scope, removing its namespace declarations.
    ///
    /// The built-in scope (index 0) is never popped.
    pub fn pop_element(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Resolve `prefix` to its URI in the current scope.
    ///
    /// Pass `""` (empty string) to look up the default namespace.
    ///
    /// Returns `None` if:
    /// - The prefix has never been bound, **or**
    /// - The default namespace has been explicitly *undeclared* with `xmlns=""`.
    pub fn resolve(&self, prefix: &str) -> Option<&NsUri> {
        // Search from innermost scope outward — the most recent binding wins.
        for scope in self.scopes.iter().rev() {
            for (p, uri) in scope {
                if p.as_ref() == prefix {
                    // `xmlns=""` is an explicit undeclaration of the default
                    // namespace: treat it as "no namespace".
                    return if uri.as_str().is_empty() {
                        None
                    } else {
                        Some(uri)
                    };
                }
            }
        }
        None
    }

    /// Split a potentially-prefixed XML name into `(prefix, local_name)`.
    ///
    /// Returns `("", name)` for unprefixed names.
    ///
    /// # Example
    ///
    /// ```rust
    /// use xml_ns::NsResolver;
    ///
    /// assert_eq!(NsResolver::split_name("svg:circle"), ("svg", "circle"));
    /// assert_eq!(NsResolver::split_name("root"),       ("",    "root"));
    /// ```
    pub fn split_name(name: &str) -> (&str, &str) {
        match name.find(':') {
            Some(i) => (&name[..i], &name[i + 1..]),
            None => ("", name),
        }
    }

    /// Resolve an **element** qualified name to a [`QName`].
    ///
    /// Unprefixed element names inherit the default namespace (if one is in
    /// scope).
    ///
    /// # Errors
    ///
    /// Returns [`NsError::UnboundPrefix`] if the name carries a prefix that
    /// has no binding in the current scope.
    pub fn resolve_element_name(&self, name: &str) -> Result<QName, NsError> {
        let (prefix, local) = Self::split_name(name);
        // Unprefixed elements inherit the default namespace.
        let ns = if prefix.is_empty() {
            self.resolve("").cloned()
        } else {
            Some(
                self.resolve(prefix)
                    .ok_or_else(|| NsError::UnboundPrefix(Box::from(prefix)))?
                    .clone(),
            )
        };
        Ok(QName {
            ns,
            local: Box::from(local),
        })
    }

    /// Resolve an **attribute** qualified name to a [`QName`].
    ///
    /// Per XML Namespaces §6.2, unprefixed attributes are in *no namespace* —
    /// they do **not** inherit the default namespace.
    ///
    /// # Errors
    ///
    /// Returns [`NsError::UnboundPrefix`] if the name carries a prefix that
    /// has no binding in the current scope.
    pub fn resolve_attr_name(&self, name: &str) -> Result<QName, NsError> {
        let (prefix, local) = Self::split_name(name);
        // Unprefixed attributes have no namespace — they never inherit default.
        let ns = if prefix.is_empty() {
            None
        } else {
            Some(
                self.resolve(prefix)
                    .ok_or_else(|| NsError::UnboundPrefix(Box::from(prefix)))?
                    .clone(),
            )
        };
        Ok(QName {
            ns,
            local: Box::from(local),
        })
    }

    /// The number of active scopes (built-in scope + one per open element).
    ///
    /// Primarily useful for testing.
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }
}

impl Default for NsResolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(s: &str) -> NsUri {
        NsUri::new(s)
    }

    // --- split_name ---

    #[test]
    fn split_prefixed() {
        assert_eq!(NsResolver::split_name("svg:circle"), ("svg", "circle"));
    }

    #[test]
    fn split_unprefixed() {
        assert_eq!(NsResolver::split_name("root"), ("", "root"));
    }

    #[test]
    fn split_xml_prefix() {
        assert_eq!(NsResolver::split_name("xml:lang"), ("xml", "lang"));
    }

    // --- built-in bindings ---

    #[test]
    fn xml_prefix_always_bound() {
        let r = NsResolver::new();
        assert_eq!(
            r.resolve("xml"),
            Some(&uri("http://www.w3.org/XML/1998/namespace"))
        );
    }

    #[test]
    fn xmlns_prefix_always_bound() {
        let r = NsResolver::new();
        assert_eq!(
            r.resolve("xmlns"),
            Some(&uri("http://www.w3.org/2000/xmlns/"))
        );
    }

    #[test]
    fn unknown_prefix_unbound() {
        let r = NsResolver::new();
        assert_eq!(r.resolve("svg"), None);
    }

    #[test]
    fn default_ns_unbound_initially() {
        let r = NsResolver::new();
        assert_eq!(r.resolve(""), None);
    }

    // --- push / pop ---

    #[test]
    fn push_default_namespace() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns", "http://example.com")]);
        assert_eq!(r.resolve(""), Some(&uri("http://example.com")));
    }

    #[test]
    fn push_prefixed_namespace() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:svg", "http://www.w3.org/2000/svg")]);
        assert_eq!(r.resolve("svg"), Some(&uri("http://www.w3.org/2000/svg")));
    }

    #[test]
    fn non_xmlns_attrs_ignored() {
        let mut r = NsResolver::new();
        r.push_element([("href", "x"), ("class", "y")]);
        assert_eq!(r.resolve("href"), None);
    }

    #[test]
    fn pop_removes_bindings() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:foo", "http://foo.com")]);
        assert_eq!(r.resolve("foo"), Some(&uri("http://foo.com")));
        r.pop_element();
        assert_eq!(r.resolve("foo"), None);
    }

    #[test]
    fn pop_never_removes_builtins() {
        let mut r = NsResolver::new();
        r.pop_element(); // extra pop — should be a no-op
        r.pop_element();
        assert_eq!(
            r.resolve("xml"),
            Some(&uri("http://www.w3.org/XML/1998/namespace"))
        );
        assert_eq!(r.depth(), 1); // only the built-in scope remains
    }

    // --- nested scopes ---

    #[test]
    fn inner_scope_overrides_outer() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:ns", "http://outer.com")]);
        r.push_element([("xmlns:ns", "http://inner.com")]);
        assert_eq!(r.resolve("ns"), Some(&uri("http://inner.com")));
        r.pop_element();
        assert_eq!(r.resolve("ns"), Some(&uri("http://outer.com")));
    }

    #[test]
    fn outer_binding_visible_in_inner_scope() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:outer", "http://outer.com")]);
        r.push_element([]); // no new declarations
        assert_eq!(r.resolve("outer"), Some(&uri("http://outer.com")));
    }

    // --- default namespace undeclaration ---

    #[test]
    fn empty_uri_undeclares_default_namespace() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns", "http://example.com")]);
        assert!(r.resolve("").is_some());
        r.push_element([("xmlns", "")]); // undeclare
        assert_eq!(r.resolve(""), None);
    }

    // --- resolve_element_name ---

    #[test]
    fn element_unprefixed_inherits_default_ns() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns", "http://example.com")]);
        let q = r.resolve_element_name("root").unwrap();
        assert_eq!(q.ns, Some(uri("http://example.com")));
        assert_eq!(q.local.as_ref(), "root");
    }

    #[test]
    fn element_prefixed_resolves() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:svg", "http://www.w3.org/2000/svg")]);
        let q = r.resolve_element_name("svg:circle").unwrap();
        assert_eq!(q.ns, Some(uri("http://www.w3.org/2000/svg")));
        assert_eq!(q.local.as_ref(), "circle");
    }

    #[test]
    fn element_no_ns_no_default() {
        let r = NsResolver::new();
        let q = r.resolve_element_name("root").unwrap();
        assert_eq!(q.ns, None);
        assert_eq!(q.local.as_ref(), "root");
    }

    #[test]
    fn element_unbound_prefix_is_error() {
        let r = NsResolver::new();
        assert_eq!(
            r.resolve_element_name("foo:bar"),
            Err(NsError::UnboundPrefix(Box::from("foo")))
        );
    }

    // --- resolve_attr_name ---

    #[test]
    fn attr_unprefixed_has_no_ns_even_with_default() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns", "http://example.com")]);
        // Unprefixed attrs are never in the default namespace (NS spec §6.2).
        let q = r.resolve_attr_name("href").unwrap();
        assert_eq!(q.ns, None);
        assert_eq!(q.local.as_ref(), "href");
    }

    #[test]
    fn attr_prefixed_resolves() {
        let mut r = NsResolver::new();
        r.push_element([("xmlns:xlink", "http://www.w3.org/1999/xlink")]);
        let q = r.resolve_attr_name("xlink:href").unwrap();
        assert_eq!(q.ns, Some(uri("http://www.w3.org/1999/xlink")));
        assert_eq!(q.local.as_ref(), "href");
    }

    #[test]
    fn attr_unbound_prefix_is_error() {
        let r = NsResolver::new();
        assert_eq!(
            r.resolve_attr_name("foo:bar"),
            Err(NsError::UnboundPrefix(Box::from("foo")))
        );
    }

    // --- depth ---

    #[test]
    fn depth_tracks_push_pop() {
        let mut r = NsResolver::new();
        assert_eq!(r.depth(), 1); // just built-ins
        r.push_element([]);
        assert_eq!(r.depth(), 2);
        r.push_element([]);
        assert_eq!(r.depth(), 3);
        r.pop_element();
        assert_eq!(r.depth(), 2);
        r.pop_element();
        assert_eq!(r.depth(), 1);
    }
}
