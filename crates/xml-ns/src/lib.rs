//! XML Namespace URI and prefix types.
//!
//! Provides lightweight, internable types for namespace URIs and prefixes
//! used throughout the libxml2-rs workspace.
//!
//! This crate is `no_std` + `alloc`.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;
use alloc::boxed::Box;

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

/// A namespace prefix (e.g. `"xhtml"`), or the default namespace when absent.
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

/// A qualified XML name: an optional namespace URI paired with a local name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QName {
    /// Namespace URI, if in a namespace.
    pub ns: Option<NsUri>,
    /// Local (unprefixed) name.
    pub local: Box<str>,
}

impl QName {
    /// Create a name with no namespace.
    pub fn local(name: &str) -> Self {
        Self { ns: None, local: name.into() }
    }

    /// Create a namespaced name.
    pub fn namespaced(ns: NsUri, local: &str) -> Self {
        Self { ns: Some(ns), local: local.into() }
    }
}
