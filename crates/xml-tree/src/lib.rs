//! Arena-based mutable XML DOM.
//!
//! All nodes are stored in a flat `Vec<NodeData>` inside [`Document`] and
//! referenced by [`NodeId`] — a 32-bit index. This avoids per-node heap
//! allocation, is cache-friendly, and makes `Document: Send + Sync` trivially.
//!
//! # Design
//!
//! ```text
//! Document
//! ├── nodes: Vec<NodeData>   ← flat arena; NodeId is an index
//! ├── string_arena: Bump     ← attribute values, text content
//! └── names: IndexSet<str>   ← interned element/attribute names (NameId)
//! ```
//!
//! See `docs/architecture/overview.md` §5.1 for the full rationale.
//!
//! This crate is `no_std` + `alloc`.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;

/// An opaque handle to a node within a [`Document`].
///
/// `NodeId` values are only meaningful within the `Document` that created them.
/// They are `Copy`, 32-bit, and cheap to store anywhere without borrow-checker
/// friction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(u32);

/// An interned name identifier (index into `Document::names`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NameId(u32);

/// The kind of a node, matching `xmlElementType` in libxml2.
///
/// Values are assigned to match libxml2's `xmlElementType` enum for C ABI
/// compatibility.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum NodeKind {
    /// Element node (`<foo/>`)
    Element            = 1,
    /// Attribute node
    Attribute          = 2,
    /// Text node
    Text               = 3,
    /// CDATA section
    CData              = 4,
    /// Entity reference
    EntityRef          = 5,
    /// Processing instruction
    Pi                 = 7,
    /// Comment
    Comment            = 8,
    /// Document node (root)
    Document           = 9,
    /// Document type declaration
    DocumentType       = 10,
    /// Document fragment
    DocumentFragment   = 11,
}

/// Internal storage for a single node. 64 bytes — fits in one cache line.
///
/// All string data (name, value) is stored as a `(offset, len)` range into
/// `Document::string_arena`. Node cross-references use `Option<NodeId>`.
#[repr(C)]
struct NodeData {
    kind:         NodeKind,
    _pad:         [u8; 3],
    name:         NameId,
    parent:       Option<NodeId>,
    first_child:  Option<NodeId>,
    last_child:   Option<NodeId>,
    next_sibling: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    // Text/value content: byte range into string_arena
    value_offset: u32,
    value_len:    u32,
    // Attributes: index range into Document::attrs
    attrs_start:  u32,
    attrs_end:    u32,
}

/// An XML document. Owns all node and string data.
///
/// `Document` is `Send + Sync` — nodes are stored by index, not raw pointer.
pub struct Document {
    nodes:        Vec<NodeData>,
    string_arena: bumpalo::Bump,
    // TODO(Phase 1): add name interning table, attribute storage, ns table
    root:         NodeId,
}

impl Document {
    /// Create a new, empty document with a single document-root node.
    pub fn new() -> Self {
        let mut nodes = Vec::with_capacity(64);
        nodes.push(NodeData {
            kind:         NodeKind::Document,
            _pad:         [0; 3],
            name:         NameId(0),
            parent:       None,
            first_child:  None,
            last_child:   None,
            next_sibling: None,
            prev_sibling: None,
            value_offset: 0,
            value_len:    0,
            attrs_start:  0,
            attrs_end:    0,
        });
        Self {
            nodes,
            string_arena: bumpalo::Bump::new(),
            root: NodeId(0),
        }
    }

    /// The document root node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The kind of the node identified by `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` does not belong to this document.
    pub fn kind(&self, id: NodeId) -> NodeKind {
        self.nodes[id.0 as usize].kind
    }

    /// The first child of `id`, if any.
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].first_child
    }

    /// The next sibling of `id`, if any.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].next_sibling
    }

    /// The parent of `id`, if any.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].parent
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

// Safety: Document contains no raw pointers; all cross-node references are
// integer indices. bumpalo::Bump is Send but not Sync; we only expose &Bump
// internally, never to callers, so Document can be Sync too.
unsafe impl Send for Document {}
unsafe impl Sync for Document {}
