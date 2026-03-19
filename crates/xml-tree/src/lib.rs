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
//! ├── nodes: Vec<NodeData>        ← flat arena; NodeId is an index
//! ├── strings: String             ← append-only text/value storage
//! ├── attrs: Vec<AttrData>        ← flat attribute storage (ranges in NodeData)
//! └── names: IndexSet<Box<str>>   ← interned element/attribute names AND
//!                                    namespace URIs (NameId(0) = "" = no namespace)
//! ```
//!
//! Namespace resolution is performed during tree construction by [`Builder`],
//! which drives an [`xml_ns::NsResolver`] scope-stack.  Each element node
//! stores its **local name** (via `NameId`) and its **namespace URI** (also a
//! `NameId` into the same interning table; `NameId(0)` = no namespace).
//!
//! See `docs/architecture/overview.md` §5.1 for the full rationale.
//!
//! This crate is `no_std` + `alloc`.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use indexmap::IndexSet;

// ---------------------------------------------------------------------------
// Public ID types
// ---------------------------------------------------------------------------

/// An opaque handle to a node within a [`Document`].
///
/// `NodeId` values are only meaningful within the `Document` that created them.
/// They are `Copy`, 32-bit, and cheap to store anywhere without borrow-checker
/// friction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(u32);

/// An interned name identifier (index into `Document::names`).
///
/// NameId(0) is reserved as a sentinel meaning "no name" — it maps to the
/// empty string `""` in the names table.
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
    Element = 1,
    /// Attribute node
    Attribute = 2,
    /// Text node
    Text = 3,
    /// CDATA section
    CData = 4,
    /// Entity reference
    EntityRef = 5,
    /// Processing instruction
    Pi = 7,
    /// Comment
    Comment = 8,
    /// Document node (root)
    Document = 9,
    /// Document type declaration
    DocumentType = 10,
    /// Document fragment
    DocumentFragment = 11,
}

// ---------------------------------------------------------------------------
// Internal node storage — 64 bytes, one cache line
// ---------------------------------------------------------------------------

/// Internal storage for a single node.
///
/// 68 bytes (`repr(C)`): `kind` (1 byte) + 3 bytes implicit padding +
/// `name`/`ns` (4+4) + five `Option<NodeId>` links (5×8) + four `u32` range
/// fields (4×4).  Approximately one cache line on common architectures.
/// String data (name, value) is stored by offset into `Document::strings`.
#[repr(C)]
struct NodeData {
    kind: NodeKind,
    // 3 bytes of implicit repr(C) padding follow `kind` to align `name` (u32).
    /// Interned local name: element tag, PI target, etc.  Index into
    /// `Document::names`.  `NameId(0)` = `""` (nameless nodes: text, comment).
    name: NameId,
    /// Interned namespace URI for element/attribute nodes.  Index into
    /// `Document::names`.  `NameId(0)` = `""` = no namespace.
    ns: NameId,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    next_sibling: Option<NodeId>,
    prev_sibling: Option<NodeId>,
    /// Byte offset of this node's string value in `Document::strings`.
    value_offset: u32,
    /// Byte length of this node's string value.
    value_len: u32,
    /// First index (inclusive) of this node's attributes in `Document::attrs`.
    attrs_start: u32,
    /// Last index (exclusive) of this node's attributes in `Document::attrs`.
    attrs_end: u32,
}

// ---------------------------------------------------------------------------
// Attribute storage
// ---------------------------------------------------------------------------

/// A single attribute — name and namespace URI interned, value stored in
/// `Document::strings`.
pub struct AttrData {
    name: NameId,
    /// Interned namespace URI.  `NameId(0)` = `""` = no namespace.
    ns: NameId,
    value_offset: u32,
    value_len: u32,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// An XML document. Owns all node and string data.
///
/// `Document` is `Send + Sync` — nodes are stored by index, not raw pointer.
pub struct Document {
    nodes: Vec<NodeData>,
    /// Append-only string storage.  Offsets in `NodeData` and `AttrData` index
    /// into this buffer.  The buffer never shrinks, so existing offsets are
    /// always valid.
    strings: String,
    /// Flat attribute storage.  Each element's attributes are a contiguous
    /// range `[attrs_start, attrs_end)` in this vec.
    attrs: Vec<AttrData>,
    /// Interned element/attribute names.  `NameId(i)` is the index.
    /// `NameId(0)` is always `""` (sentinel for nameless nodes).
    names: IndexSet<Box<str>>,
    root: NodeId,
}

impl Document {
    /// Create a new, empty document with a single document-root node.
    pub fn new() -> Self {
        // Reserve NameId(0) = "" (sentinel for nameless nodes: text, comment…)
        let mut names: IndexSet<Box<str>> = IndexSet::new();
        names.insert(Box::from(""));

        let mut nodes = Vec::with_capacity(64);
        nodes.push(NodeData {
            kind: NodeKind::Document,
            name: NameId(0),
            ns: NameId(0),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset: 0,
            value_len: 0,
            attrs_start: 0,
            attrs_end: 0,
        });

        Self {
            nodes,
            strings: String::new(),
            attrs: Vec::new(),
            names,
            root: NodeId(0),
        }
    }

    // -----------------------------------------------------------------------
    // Read-only navigation
    // -----------------------------------------------------------------------

    /// The document root node.
    pub fn root(&self) -> NodeId {
        self.root
    }

    /// The kind of node `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` is out of range for this document.
    pub fn kind(&self, id: NodeId) -> NodeKind {
        self.nodes[id.0 as usize].kind
    }

    /// The parent of `id`, if any.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].parent
    }

    /// The first child of `id`, if any.
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].first_child
    }

    /// The last child of `id`, if any.
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].last_child
    }

    /// The next sibling of `id`, if any.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].next_sibling
    }

    /// The previous sibling of `id`, if any.
    pub fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes[id.0 as usize].prev_sibling
    }

    /// The local name of node `id`.
    ///
    /// - For element nodes: the local part of the tag name (e.g. `"circle"` for
    ///   `svg:circle` after namespace resolution, or `"div"` for unprefixed names).
    /// - For processing instructions: the PI target.
    /// - For text, comment, and CDATA nodes: their conventional pseudo-names
    ///   (`"#text"`, `"#comment"`, `"#cdata-section"`).
    /// - For the document root: `"#document"`.
    pub fn name(&self, id: NodeId) -> &str {
        match self.nodes[id.0 as usize].kind {
            NodeKind::Text => "#text",
            NodeKind::Comment => "#comment",
            NodeKind::CData => "#cdata-section",
            NodeKind::Document => "#document",
            _ => self
                .names
                .get_index(self.nodes[id.0 as usize].name.0 as usize)
                .map(|s| s.as_ref())
                .unwrap_or(""),
        }
    }

    /// The namespace URI of element node `id`, or `""` if the element has no namespace.
    ///
    /// For non-element nodes this always returns `""`.
    pub fn ns_uri(&self, id: NodeId) -> &str {
        self.names
            .get_index(self.nodes[id.0 as usize].ns.0 as usize)
            .map(|s| s.as_ref())
            .unwrap_or("")
    }

    /// The string value of node `id`.
    ///
    /// - For text, comment, CDATA, and PI nodes: the raw content.
    /// - For element and document nodes: empty string (use [`children`] to
    ///   recurse instead).
    ///
    /// [`children`]: Document::children
    pub fn value(&self, id: NodeId) -> &str {
        let n = &self.nodes[id.0 as usize];
        &self.strings[n.value_offset as usize..][..n.value_len as usize]
    }

    /// The attributes of element node `id`.
    ///
    /// Returns an empty slice for non-element nodes.
    pub fn attrs(&self, id: NodeId) -> &[AttrData] {
        let n = &self.nodes[id.0 as usize];
        &self.attrs[n.attrs_start as usize..n.attrs_end as usize]
    }

    /// The local name of an attribute.
    pub fn attr_name<'a>(&'a self, attr: &'a AttrData) -> &'a str {
        self.names
            .get_index(attr.name.0 as usize)
            .map(|s| s.as_ref())
            .unwrap_or("")
    }

    /// The namespace URI of an attribute, or `""` if the attribute has no namespace.
    pub fn attr_ns_uri<'a>(&'a self, attr: &'a AttrData) -> &'a str {
        self.names
            .get_index(attr.ns.0 as usize)
            .map(|s| s.as_ref())
            .unwrap_or("")
    }

    /// The value of an attribute.
    pub fn attr_value<'a>(&'a self, attr: &'a AttrData) -> &'a str {
        &self.strings[attr.value_offset as usize..][..attr.value_len as usize]
    }

    /// An iterator over the children of `id` in document order.
    pub fn children(&self, id: NodeId) -> ChildIter<'_> {
        ChildIter {
            doc: self,
            next: self.first_child(id),
        }
    }

    // -----------------------------------------------------------------------
    // Mutation — used by Builder; low-level
    // -----------------------------------------------------------------------

    /// Intern `name` into the name table, returning its [`NameId`].
    ///
    /// Subsequent calls with the same string return the same `NameId` without
    /// allocating.
    pub fn intern_name(&mut self, name: &str) -> NameId {
        // Avoid allocating a Box if the name is already interned.
        if let Some(idx) = self.names.get_index_of(name) {
            return NameId(idx as u32);
        }
        let idx = self.names.len();
        self.names.insert(Box::from(name));
        NameId(idx as u32)
    }

    /// Copy `s` into the string arena, returning `(offset, len)`.
    fn alloc_str(&mut self, s: &str) -> (u32, u32) {
        let offset = self.strings.len() as u32;
        let len = s.len() as u32;
        self.strings.push_str(s);
        (offset, len)
    }

    /// Allocate a raw node and return its [`NodeId`].
    fn push_node(&mut self, data: NodeData) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(data);
        id
    }

    /// Link `child` as the last child of `parent`, maintaining the
    /// doubly-linked sibling list.
    ///
    /// `child` must not already be attached to a parent — call
    /// [`unlink_node`] first if needed.
    ///
    /// [`unlink_node`]: Document::unlink_node
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        // Rust note: we can't hold two mutable references into `self.nodes`
        // simultaneously, so we read what we need first, then write.
        let old_last = self.nodes[parent.0 as usize].last_child;

        self.nodes[child.0 as usize].parent = Some(parent);
        self.nodes[child.0 as usize].prev_sibling = old_last;

        match old_last {
            Some(prev) => self.nodes[prev.0 as usize].next_sibling = Some(child),
            None => self.nodes[parent.0 as usize].first_child = Some(child),
        }
        self.nodes[parent.0 as usize].last_child = Some(child);
    }

    // -----------------------------------------------------------------------
    // Node unlinking
    // -----------------------------------------------------------------------

    /// Detach `node` from its parent and siblings.
    ///
    /// The node (and its entire subtree) remains in the arena with its
    /// [`NodeId`] intact — it is simply disconnected from the tree.  Use
    /// [`append_child`] to reattach it elsewhere.
    ///
    /// Unlinking the document root is a no-op.
    ///
    /// [`append_child`]: Document::append_child
    pub fn unlink_node(&mut self, node: NodeId) {
        let parent = match self.nodes[node.0 as usize].parent {
            Some(p) => p,
            None => return, // already detached or document root
        };

        let prev = self.nodes[node.0 as usize].prev_sibling;
        let next = self.nodes[node.0 as usize].next_sibling;

        // Fix up the previous sibling (or parent's first_child).
        match prev {
            Some(p) => self.nodes[p.0 as usize].next_sibling = next,
            None => self.nodes[parent.0 as usize].first_child = next,
        }

        // Fix up the next sibling (or parent's last_child).
        match next {
            Some(n) => self.nodes[n.0 as usize].prev_sibling = prev,
            None => self.nodes[parent.0 as usize].last_child = prev,
        }

        // Clear the node's own links.
        self.nodes[node.0 as usize].parent = None;
        self.nodes[node.0 as usize].prev_sibling = None;
        self.nodes[node.0 as usize].next_sibling = None;
    }

    // -----------------------------------------------------------------------
    // Higher-level append helpers
    // -----------------------------------------------------------------------

    /// Append an element child to `parent` and return its [`NodeId`].
    ///
    /// `name` is stored as-is (no namespace resolution). Use
    /// [`append_element_ns`] when the local name and namespace URI are already
    /// known (e.g. from the [`Builder`]).
    ///
    /// Attributes should be added immediately after with [`add_attr`].
    ///
    /// [`append_element_ns`]: Document::append_element_ns
    /// [`add_attr`]: Document::add_attr
    pub fn append_element(&mut self, parent: NodeId, name: &str) -> NodeId {
        self.append_element_ns(parent, name, "")
    }

    /// Append a namespace-resolved element child to `parent`.
    ///
    /// `local` is the local part of the name; `ns_uri` is the namespace URI
    /// (pass `""` for no namespace).  Used by [`Builder`] after resolving
    /// namespace prefixes via [`xml_ns::NsResolver`].
    pub fn append_element_ns(&mut self, parent: NodeId, local: &str, ns_uri: &str) -> NodeId {
        let name_id = self.intern_name(local);
        let ns_id = self.intern_name(ns_uri);
        let attrs_start = self.attrs.len() as u32;
        let id = self.push_node(NodeData {
            kind: NodeKind::Element,
            name: name_id,
            ns: ns_id,
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset: 0,
            value_len: 0,
            attrs_start,
            attrs_end: attrs_start,
        });
        self.append_child(parent, id);
        id
    }

    /// Append an attribute to element `elem`.
    ///
    /// Attributes must be added before any child nodes are appended to `elem`
    /// to preserve the contiguous layout in `Document::attrs`.
    /// Uses no namespace (equivalent to `add_attr_ns(elem, name, "", value)`).
    pub fn add_attr(&mut self, elem: NodeId, name: &str, value: &str) {
        self.add_attr_ns(elem, name, "", value);
    }

    /// Append a namespace-resolved attribute to element `elem`.
    ///
    /// `local` is the local part of the attribute name; `ns_uri` is the
    /// namespace URI (pass `""` for no namespace).  Used by [`Builder`] after
    /// namespace resolution.
    pub fn add_attr_ns(&mut self, elem: NodeId, local: &str, ns_uri: &str, value: &str) {
        debug_assert_eq!(
            self.nodes[elem.0 as usize].attrs_end as usize,
            self.attrs.len(),
            "add_attr_ns: attributes must be added before child nodes"
        );
        let name_id = self.intern_name(local);
        let ns_id = self.intern_name(ns_uri);
        let (value_offset, value_len) = self.alloc_str(value);
        self.attrs.push(AttrData {
            name: name_id,
            ns: ns_id,
            value_offset,
            value_len,
        });
        self.nodes[elem.0 as usize].attrs_end += 1;
    }

    /// Append a text node to `parent`.
    pub fn append_text(&mut self, parent: NodeId, content: &str) -> NodeId {
        let (value_offset, value_len) = self.alloc_str(content);
        let id = self.push_node(NodeData {
            kind: NodeKind::Text,
            name: NameId(0),
            ns: NameId(0),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset,
            value_len,
            attrs_start: 0,
            attrs_end: 0,
        });
        self.append_child(parent, id);
        id
    }

    /// Append a comment node to `parent`.
    pub fn append_comment(&mut self, parent: NodeId, content: &str) -> NodeId {
        let (value_offset, value_len) = self.alloc_str(content);
        let id = self.push_node(NodeData {
            kind: NodeKind::Comment,
            name: NameId(0),
            ns: NameId(0),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset,
            value_len,
            attrs_start: 0,
            attrs_end: 0,
        });
        self.append_child(parent, id);
        id
    }

    /// Append a CDATA section node to `parent`.
    pub fn append_cdata(&mut self, parent: NodeId, content: &str) -> NodeId {
        let (value_offset, value_len) = self.alloc_str(content);
        let id = self.push_node(NodeData {
            kind: NodeKind::CData,
            name: NameId(0),
            ns: NameId(0),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset,
            value_len,
            attrs_start: 0,
            attrs_end: 0,
        });
        self.append_child(parent, id);
        id
    }

    /// Append a processing instruction node to `parent`.
    pub fn append_pi(&mut self, parent: NodeId, target: &str, data: &str) -> NodeId {
        let name_id = self.intern_name(target);
        let (value_offset, value_len) = self.alloc_str(data);
        let id = self.push_node(NodeData {
            kind: NodeKind::Pi,
            name: name_id,
            ns: NameId(0),
            parent: None,
            first_child: None,
            last_child: None,
            next_sibling: None,
            prev_sibling: None,
            value_offset,
            value_len,
            attrs_start: 0,
            attrs_end: 0,
        });
        self.append_child(parent, id);
        id
    }
}

// ---------------------------------------------------------------------------
// Subtree copy helpers (private)
// ---------------------------------------------------------------------------

/// Intermediate representation for copying a subtree.
///
/// Produced by [`collect_subtree_data`] (immutable borrow) and consumed by
/// [`paste_subtree_data`] (mutable borrow).  The two-phase approach avoids
/// the borrow-checker conflict that would arise from reading and writing
/// `Document::strings` / `Document::names` at the same time.
struct NodeCopy {
    kind: NodeKind,
    /// Actual interned name — local element name or PI target.
    /// For text/comment/CDATA this holds the pseudo-name; it is ignored on paste.
    name: String,
    /// Namespace URI of the element/attribute, or `""`.
    ns: String,
    /// String value (text, comment, CDATA content, PI data).
    value: String,
    /// `(local, ns_uri, value)` for each attribute.
    attrs: Vec<(String, String, String)>,
    /// Copied children, in document order.
    children: Vec<NodeCopy>,
}

/// Phase 1 — collect the subtree at `node` into an owned [`NodeCopy`].
///
/// Only uses the public read API of [`Document`]; the immutable borrow is
/// released when this function returns.
fn collect_subtree_data(doc: &Document, node: NodeId) -> NodeCopy {
    let kind = doc.kind(node);
    // `name()` returns the real name for Element/Pi; pseudo-names otherwise.
    let name = doc.name(node).to_owned();
    let ns = doc.ns_uri(node).to_owned();
    let value = doc.value(node).to_owned();
    let attrs = doc
        .attrs(node)
        .iter()
        .map(|a| {
            (
                doc.attr_name(a).to_owned(),
                doc.attr_ns_uri(a).to_owned(),
                doc.attr_value(a).to_owned(),
            )
        })
        .collect();
    let children = doc
        .children(node)
        .map(|c| collect_subtree_data(doc, c))
        .collect();
    NodeCopy {
        kind,
        name,
        ns,
        value,
        attrs,
        children,
    }
}

/// Phase 2 — paste `copy` as the last child of `parent`, returning the new node.
fn paste_subtree_data(doc: &mut Document, copy: &NodeCopy, parent: NodeId) -> NodeId {
    let new_id = match copy.kind {
        NodeKind::Element => {
            let id = doc.append_element_ns(parent, &copy.name, &copy.ns);
            for (aname, ans, aval) in &copy.attrs {
                doc.add_attr_ns(id, aname, ans, aval);
            }
            id
        }
        NodeKind::Text => doc.append_text(parent, &copy.value),
        NodeKind::Comment => doc.append_comment(parent, &copy.value),
        NodeKind::CData => doc.append_cdata(parent, &copy.value),
        NodeKind::Pi => doc.append_pi(parent, &copy.name, &copy.value),
        // Other node kinds (EntityRef, DocumentType, …): create a structural
        // placeholder so the subtree shape is preserved.
        _ => {
            let name_id = doc.intern_name(&copy.name);
            let ns_id = doc.intern_name(&copy.ns);
            let (vo, vl) = doc.alloc_str(&copy.value);
            let attrs_start = doc.attrs.len() as u32;
            let id = doc.push_node(NodeData {
                kind: copy.kind,
                name: name_id,
                ns: ns_id,
                parent: None,
                first_child: None,
                last_child: None,
                next_sibling: None,
                prev_sibling: None,
                value_offset: vo,
                value_len: vl,
                attrs_start,
                attrs_end: attrs_start,
            });
            doc.append_child(parent, id);
            id
        }
    };
    for child in &copy.children {
        paste_subtree_data(doc, child, new_id);
    }
    new_id
}

impl Document {
    // -----------------------------------------------------------------------
    // Subtree copying and document cloning
    // -----------------------------------------------------------------------

    /// Copy the subtree rooted at `src`, appending the copy as the last child
    /// of `dest`.  Returns the [`NodeId`] of the new root.
    ///
    /// String data is cloned; the copy shares no arena storage with the
    /// original.  All existing [`NodeId`] values remain valid after this call.
    pub fn copy_subtree(&mut self, src: NodeId, dest: NodeId) -> NodeId {
        // Phase 1: collect into owned data (immutable borrow of `self`).
        let subtree = collect_subtree_data(self, src);
        // Phase 2: paste (mutable borrow of `self`).  No simultaneous borrows.
        paste_subtree_data(self, &subtree, dest)
    }

    /// Create a structurally identical, fully independent clone of this document.
    ///
    /// All nodes, attributes, and string data are duplicated.
    pub fn deep_clone(&self) -> Document {
        let mut dst = Document::new();
        let dst_root = dst.root();
        // Collect children while `self` is borrowed immutably.
        let children: Vec<NodeId> = self.children(self.root()).collect();
        for child in children {
            let subtree = collect_subtree_data(self, child);
            paste_subtree_data(&mut dst, &subtree, dst_root);
        }
        dst
    }

    // -----------------------------------------------------------------------
    // Attribute mutation
    // -----------------------------------------------------------------------

    /// Set attribute `local` (no namespace) to `value` on element `elem`.
    ///
    /// If an attribute with this name already exists, its value is updated
    /// in place.  If not, a new attribute is appended to `elem`'s list.
    pub fn set_attr(&mut self, elem: NodeId, local: &str, value: &str) {
        self.set_attr_ns(elem, local, "", value);
    }

    /// Set attribute `local` in namespace `ns_uri` to `value` on element `elem`.
    ///
    /// If an attribute with this `(local, ns_uri)` pair already exists its
    /// value is updated; otherwise a new attribute is appended.
    pub fn set_attr_ns(&mut self, elem: NodeId, local: &str, ns_uri: &str, value: &str) {
        // ── Update existing attribute in-place ───────────────────────────────
        if let Some(pos) = self.find_attr_pos(elem, local, ns_uri) {
            // Append new value to the arena; old value is an orphan (harmless).
            let (vo, vl) = self.alloc_str(value);
            self.attrs[pos].value_offset = vo;
            self.attrs[pos].value_len = vl;
            return;
        }

        // ── Insert new attribute at elem.attrs_end ───────────────────────────
        let name_id = self.intern_name(local);
        let ns_id = self.intern_name(ns_uri);
        let (vo, vl) = self.alloc_str(value);
        let insert_pos = self.nodes[elem.0 as usize].attrs_end as usize;
        self.attrs.insert(
            insert_pos,
            AttrData {
                name: name_id,
                ns: ns_id,
                value_offset: vo,
                value_len: vl,
            },
        );
        self.fixup_attrs_after_insert(insert_pos as u32, elem);
    }

    /// Remove attribute `local` (no namespace) from element `elem`.
    ///
    /// Returns `true` if the attribute was found and removed, `false` if it
    /// did not exist.
    pub fn remove_attr(&mut self, elem: NodeId, local: &str) -> bool {
        self.remove_attr_ns(elem, local, "")
    }

    /// Remove attribute `local` in namespace `ns_uri` from element `elem`.
    ///
    /// Returns `true` if the attribute was found and removed, `false` if it
    /// did not exist.
    pub fn remove_attr_ns(&mut self, elem: NodeId, local: &str, ns_uri: &str) -> bool {
        let Some(pos) = self.find_attr_pos(elem, local, ns_uri) else {
            return false;
        };
        self.attrs.remove(pos);
        self.fixup_attrs_after_remove(pos as u32);
        true
    }

    /// Return the index of attribute `(local, ns_uri)` within `elem`'s attribute
    /// slice, or `None` if it does not exist.
    fn find_attr_pos(&self, elem: NodeId, local: &str, ns_uri: &str) -> Option<usize> {
        let n = &self.nodes[elem.0 as usize];
        // If the name or namespace has never been interned the attr can't exist.
        let local_id = self.names.get_index_of(local)? as u32;
        let ns_id = self.names.get_index_of(ns_uri)? as u32;
        (n.attrs_start as usize..n.attrs_end as usize)
            .find(|&pos| self.attrs[pos].name.0 == local_id && self.attrs[pos].ns.0 == ns_id)
    }

    /// Fixup every node's `attrs_start`/`attrs_end` after inserting one slot
    /// at `pos` in `self.attrs`.
    ///
    /// `elem` is the element that owns the new attr — only its `attrs_end` is
    /// incremented.  Every other node whose range starts at or after `pos` is
    /// shifted right by one.
    fn fixup_attrs_after_insert(&mut self, pos: u32, elem: NodeId) {
        let elem_idx = elem.0 as usize;
        for (i, node) in self.nodes.iter_mut().enumerate() {
            if i == elem_idx {
                // Expand the owning element's range.
                node.attrs_end += 1;
            } else if node.attrs_start >= pos {
                // Range starts at or after the insert point — shift both ends.
                node.attrs_start += 1;
                node.attrs_end += 1;
            } else if node.attrs_end > pos {
                // Range spans the insert point (defensive; shouldn't occur with
                // non-overlapping attr ranges).
                node.attrs_end += 1;
            }
        }
    }

    /// Fixup every node's `attrs_start`/`attrs_end` after removing the slot
    /// at `pos` from `self.attrs`.
    fn fixup_attrs_after_remove(&mut self, pos: u32) {
        for node in self.nodes.iter_mut() {
            if node.attrs_start > pos {
                node.attrs_start -= 1;
            }
            if node.attrs_end > pos {
                node.attrs_end -= 1;
            }
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

// Safety: Document contains no raw pointers; all cross-node references are
// integer indices.  `strings: String` is `Send + Sync`.  `IndexSet` is
// `Send + Sync` when its key type is.
unsafe impl Send for Document {}
unsafe impl Sync for Document {}

// ---------------------------------------------------------------------------
// ChildIter — forward iterator over a node's children
// ---------------------------------------------------------------------------

/// An iterator over the direct children of a node, in document order.
///
/// Created by [`Document::children`].
pub struct ChildIter<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl<'a> Iterator for ChildIter<'a> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.doc.next_sibling(id);
        Some(id)
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Controls when the `<?xml version="1.0" encoding="…"?>` declaration is emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlDeclMode {
    /// Always emit the declaration.
    Emit,
    /// Never emit the declaration (default).
    Omit,
}

/// Options that control XML serialization.
#[derive(Debug, Clone)]
pub struct SerializeOptions {
    /// Indentation string for each nesting level (e.g. `"  "`).
    /// `None` (the default) produces compact single-line output.
    pub indent: Option<String>,
    /// Whether to emit the XML declaration.  Default: [`XmlDeclMode::Omit`].
    pub xml_decl: XmlDeclMode,
    /// Encoding label written in the XML declaration.  Default: `"UTF-8"`.
    pub declared_encoding: String,
}

impl Default for SerializeOptions {
    fn default() -> Self {
        Self {
            indent: None,
            xml_decl: XmlDeclMode::Omit,
            declared_encoding: String::from("UTF-8"),
        }
    }
}

impl Document {
    /// Serialize this document to an XML string.
    ///
    /// All text content is properly escaped.  Namespace declarations are
    /// re-emitted from the stored namespace URI metadata using synthetic
    /// prefixes (`ns0`, `ns1`, …).
    pub fn to_xml_string(&self, opts: &SerializeOptions) -> String {
        Serializer::new(self, opts).run_document()
    }

    /// Serialize a single node (and its subtree) to an XML string.
    ///
    /// The XML declaration is not emitted regardless of `opts.xml_decl`.
    pub fn serialize_node(&self, node: NodeId, opts: &SerializeOptions) -> String {
        let mut s = Serializer::new(self, opts);
        s.node(node, 0);
        s.out
    }
}

/// Internal XML serializer.
struct Serializer<'d> {
    doc: &'d Document,
    opts: &'d SerializeOptions,
    /// Output buffer.
    out: String,
    /// Flat namespace scope: `(uri, prefix)` pairs pushed when a namespace is
    /// first declared; truncated back when the declaring element closes.
    ns_scope: Vec<(String, String)>,
    /// Counter for generating synthetic namespace prefixes.
    ns_idx: u32,
}

impl<'d> Serializer<'d> {
    fn new(doc: &'d Document, opts: &'d SerializeOptions) -> Self {
        Self {
            doc,
            opts,
            out: String::new(),
            ns_scope: Vec::new(),
            ns_idx: 0,
        }
    }

    fn run_document(mut self) -> String {
        if self.opts.xml_decl == XmlDeclMode::Emit {
            self.out.push_str("<?xml version=\"1.0\" encoding=\"");
            self.out.push_str(&self.opts.declared_encoding);
            self.out.push_str("\"?>");
            if self.opts.indent.is_some() {
                self.out.push('\n');
            }
        }
        let children: Vec<NodeId> = self.doc.children(self.doc.root()).collect();
        for child in children {
            self.node(child, 0);
        }
        self.out
    }

    /// Look up the prefix currently in scope for `uri`.
    fn find_prefix(&self, uri: &str) -> Option<String> {
        self.ns_scope
            .iter()
            .rev()
            .find(|(u, _)| u == uri)
            .map(|(_, p)| p.clone())
    }

    /// Return the prefix for `uri`, assigning a new one if needed and
    /// recording the declaration in `new_decls`.
    fn ensure_prefix(&mut self, uri: &str, new_decls: &mut Vec<(String, String)>) -> String {
        if let Some(p) = self.find_prefix(uri) {
            return p;
        }
        // Already queued for this element?
        if let Some((_, p)) = new_decls.iter().find(|(u, _)| u == uri) {
            return p.clone();
        }
        let p = alloc::format!("ns{}", self.ns_idx);
        self.ns_idx += 1;
        new_decls.push((uri.to_string(), p.clone()));
        p
    }

    /// Dispatch serialization of a single node.
    fn node(&mut self, id: NodeId, depth: usize) {
        match self.doc.kind(id) {
            NodeKind::Element => self.element(id, depth),
            NodeKind::Text => {
                push_text_escaped(&mut self.out, self.doc.value(id));
            }
            NodeKind::Comment => {
                self.out.push_str("<!--");
                self.out.push_str(self.doc.value(id));
                self.out.push_str("-->");
                if self.opts.indent.is_some() {
                    self.out.push('\n');
                }
            }
            NodeKind::CData => {
                self.out.push_str("<![CDATA[");
                push_cdata_content(&mut self.out, self.doc.value(id));
                self.out.push_str("]]>");
            }
            NodeKind::Pi => {
                let target = self.doc.name(id);
                let data = self.doc.value(id);
                self.out.push_str("<?");
                self.out.push_str(target);
                if !data.is_empty() {
                    self.out.push(' ');
                    self.out.push_str(data);
                }
                self.out.push_str("?>");
                if self.opts.indent.is_some() {
                    self.out.push('\n');
                }
            }
            // Other node kinds (document, entity-ref, …) are not serialized.
            _ => {}
        }
    }

    fn element(&mut self, id: NodeId, depth: usize) {
        let local_name = self.doc.name(id).to_string();
        let ns_uri = self.doc.ns_uri(id).to_string();
        // Save scope position so we can restore it on exit.
        let scope_start = self.ns_scope.len();

        let mut new_decls: Vec<(String, String)> = Vec::new();

        // ── Element namespace ────────────────────────────────────────────────
        let elem_prefix: Option<String> = if ns_uri.is_empty() {
            None
        } else {
            Some(self.ensure_prefix(&ns_uri, &mut new_decls))
        };

        // ── Attributes ───────────────────────────────────────────────────────
        // Pre-collect attribute data to avoid borrow-checker friction.
        struct PreparedAttr {
            prefix: String, // empty = no namespace
            local: String,
            value: String,
            skip: bool, // xmlns declarations are re-emitted from ns metadata
        }
        let raw_attrs: Vec<_> = self
            .doc
            .attrs(id)
            .iter()
            .map(|a| {
                let ns = self.doc.attr_ns_uri(a).to_string();
                let name = self.doc.attr_name(a).to_string();
                let val = self.doc.attr_value(a).to_string();
                (ns, name, val)
            })
            .collect();

        let mut prepared: Vec<PreparedAttr> = Vec::with_capacity(raw_attrs.len());
        for (attr_ns, attr_local, attr_val) in &raw_attrs {
            // Skip namespace declaration attributes — we re-emit them below.
            let is_xmlns_decl = (attr_local == "xmlns" && attr_ns.is_empty())
                || attr_ns == "http://www.w3.org/2000/xmlns/";
            if is_xmlns_decl {
                prepared.push(PreparedAttr {
                    prefix: String::new(),
                    local: attr_local.clone(),
                    value: attr_val.clone(),
                    skip: true,
                });
                continue;
            }
            let prefix = if attr_ns.is_empty() {
                String::new()
            } else {
                self.ensure_prefix(attr_ns, &mut new_decls)
            };
            prepared.push(PreparedAttr {
                prefix,
                local: attr_local.clone(),
                value: attr_val.clone(),
                skip: false,
            });
        }

        // Push new declarations into the scope so descendant elements can
        // reuse the same prefix without re-declaring.
        for decl in &new_decls {
            self.ns_scope.push(decl.clone());
        }

        // ── Emit opening tag ─────────────────────────────────────────────────
        if let Some(ref ind) = self.opts.indent {
            for _ in 0..depth {
                self.out.push_str(ind);
            }
        }
        self.out.push('<');
        if let Some(ref p) = elem_prefix {
            self.out.push_str(p);
            self.out.push(':');
        }
        self.out.push_str(&local_name);

        // Emit xmlns declarations for newly introduced namespaces.
        for (uri, prefix) in &new_decls {
            self.out.push_str(" xmlns:");
            self.out.push_str(prefix);
            self.out.push_str("=\"");
            push_attr_escaped(&mut self.out, uri);
            self.out.push('"');
        }

        // Emit regular attributes.
        for attr in &prepared {
            if attr.skip {
                continue;
            }
            self.out.push(' ');
            if !attr.prefix.is_empty() {
                self.out.push_str(&attr.prefix);
                self.out.push(':');
            }
            self.out.push_str(&attr.local);
            self.out.push_str("=\"");
            push_attr_escaped(&mut self.out, &attr.value);
            self.out.push('"');
        }

        // ── Children ─────────────────────────────────────────────────────────
        let children: Vec<NodeId> = self.doc.children(id).collect();
        if children.is_empty() {
            self.out.push_str("/>");
            if self.opts.indent.is_some() {
                self.out.push('\n');
            }
        } else {
            self.out.push('>');

            // In indent mode, add a newline before the first child only when
            // there are element children (avoid mangling text-only content).
            let has_elem_children = children
                .iter()
                .any(|&c| self.doc.kind(c) == NodeKind::Element);
            if self.opts.indent.is_some() && has_elem_children {
                self.out.push('\n');
            }

            for &child in &children {
                self.node(child, depth + 1);
            }

            // Closing tag indentation.
            if self.opts.indent.is_some() && has_elem_children {
                if let Some(ref ind) = self.opts.indent {
                    for _ in 0..depth {
                        self.out.push_str(ind);
                    }
                }
            }
            self.out.push_str("</");
            if let Some(ref p) = elem_prefix {
                self.out.push_str(p);
                self.out.push(':');
            }
            self.out.push_str(&local_name);
            self.out.push('>');
            if self.opts.indent.is_some() {
                self.out.push('\n');
            }
        }

        // Restore namespace scope.
        self.ns_scope.truncate(scope_start);
    }
}

/// Escape `s` for use in XML text content (`<`, `>`, `&`).
fn push_text_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// Escape `s` for use in an XML attribute value (double-quoted).
fn push_attr_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

/// Write `s` as CDATA content, splitting on any embedded `]]>` sequences.
///
/// The caller is responsible for wrapping with `<![CDATA[` and `]]>`.
fn push_cdata_content(out: &mut String, s: &str) {
    // The sequence `]]>` cannot appear inside a CDATA section.
    // Split it by closing/reopening: `]]>` → `]]` (in CDATA) + `]]>` (end)
    // + `<![CDATA[>` (new CDATA starting with `>`).
    let mut rest = s;
    while let Some(pos) = rest.find("]]>") {
        out.push_str(&rest[..pos]); // content before ]]>
        out.push_str("]]]]><![CDATA[>"); // ]] (in CDATA) + ]]> (end) + <![CDATA[> (start+>)
        rest = &rest[pos + 3..]; // skip past ]]>
    }
    out.push_str(rest);
}

// ---------------------------------------------------------------------------
// Builder — SAX-style tree construction
// ---------------------------------------------------------------------------

/// Errors that can occur while building a document.
#[derive(Debug, PartialEq)]
pub enum BuildError {
    /// The document contains no root element.
    NoRootElement,
    /// A closing tag was found with no matching opening tag.
    UnexpectedEndTag,
    /// A namespace prefix was used that has no binding in the current scope.
    UnboundNamespacePrefix(Box<str>),
    /// A numeric character reference (`&#NNN;` or `&#xNN;`) refers to a
    /// code point that is not a legal XML 1.0 character.
    InvalidCharacterReference(u32),
}

// ---------------------------------------------------------------------------
// Entity / character-reference decoder
// ---------------------------------------------------------------------------

/// Decode XML entity and character references in `input`, returning the
/// decoded text.
///
/// Returns `Cow::Borrowed(input)` with no allocation when `input` contains
/// no `&` characters (the common case for plain text).
///
/// Handles:
/// - Predefined entities: `&amp;` `&lt;` `&gt;` `&apos;` `&quot;`
/// - Decimal character references: `&#NNN;`
/// - Hex character references: `&#xNN;` or `&#XNN;`
/// - Unknown named entities (e.g. `&foo;`) are passed through unchanged
///   until DTD-based expansion is implemented in a later issue.
///
/// # Errors
///
/// Returns [`BuildError::InvalidCharacterReference`] if a numeric reference
/// names a code point that is not a legal XML 1.0 character (§2.2).
pub(crate) fn decode_entities(input: &str) -> Result<Cow<'_, str>, BuildError> {
    // Fast path: no `&` present — nothing to decode.
    if !input.contains('&') {
        return Ok(Cow::Borrowed(input));
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(amp) = rest.find('&') {
        // Push everything before the `&` verbatim.
        out.push_str(&rest[..amp]);
        rest = &rest[amp + 1..]; // skip `&`

        // Find the closing `;`.
        let semi = match rest.find(';') {
            Some(i) => i,
            None => {
                // Malformed — no closing `;`. Pass `&` through and continue.
                out.push('&');
                continue;
            }
        };

        let reference = &rest[..semi];
        rest = &rest[semi + 1..]; // skip past `;`

        if let Some(digits) = reference.strip_prefix('#') {
            // Numeric character reference: &#NNN; or &#xNN;
            if let Some(hex) = digits
                .strip_prefix('x')
                .or_else(|| digits.strip_prefix('X'))
            {
                // &#xNN; — hexadecimal character reference
                let code = u32::from_str_radix(hex, 16).unwrap_or(u32::MAX);
                let ch = xml_char_from_codepoint(code)?;
                out.push(ch);
            } else {
                // &#NNN; — decimal character reference
                let code = digits.parse::<u32>().unwrap_or(u32::MAX);
                let ch = xml_char_from_codepoint(code)?;
                out.push(ch);
            }
        } else if !reference.is_empty() {
            if reference == "amp" {
                out.push('&');
            } else if reference == "lt" {
                out.push('<');
            } else if reference == "gt" {
                out.push('>');
            } else if reference == "apos" {
                out.push('\'');
            } else if reference == "quot" {
                out.push('"');
            } else {
                // Unknown named entity — pass through unchanged until DTD
                // expansion is implemented in issue #12.
                out.push('&');
                out.push_str(reference);
                out.push(';');
            }
        } else {
            // Empty reference `&;` — pass through.
            out.push('&');
            out.push(';');
        }
    }

    // Push the remainder after the last `&` (or the whole string if no `&`
    // was processed in the loop above but the fast-path check missed it).
    out.push_str(rest);
    Ok(Cow::Owned(out))
}

/// Validate `code` against XML 1.0 §2.2 legal character ranges and return
/// the corresponding `char`, or [`BuildError::InvalidCharacterReference`].
///
/// Legal: `#x9 | #xA | #xD | [#x20–#xD7FF] | [#xE000–#xFFFD] | [#x10000–#x10FFFF]`
fn xml_char_from_codepoint(code: u32) -> Result<char, BuildError> {
    let ok = matches!(
        code,
        0x9 | 0xA | 0xD
        | 0x20..=0xD7FF
        | 0xE000..=0xFFFD
        | 0x10000..=0x10FFFF
    );
    if ok {
        // Safety: all accepted code points are valid Unicode scalar values
        // (we excluded surrogates 0xD800–0xDFFF and the non-chars 0xFFFE/0xFFFF).
        char::from_u32(code).ok_or(BuildError::InvalidCharacterReference(code))
    } else {
        Err(BuildError::InvalidCharacterReference(code))
    }
}

/// Builds a [`Document`] by processing a stream of [`xml_tokenizer::Token`]s.
///
/// Call [`process_token`] for each token from the tokenizer, then call
/// [`finish`] to get the completed document.
///
/// # Example
///
/// ```rust
/// use xml_tokenizer::Tokenizer;
/// use xml_tree::Builder;
///
/// let xml = b"<?xml version=\"1.0\"?><root><child/></root>";
/// let mut tok = Tokenizer::new(xml).unwrap();
/// let mut builder = Builder::new();
/// loop {
///     let token = tok.next_token().unwrap();
///     let done = token == xml_tokenizer::Token::Eof;
///     builder.process_token(token).unwrap();
///     if done { break; }
/// }
/// let doc = builder.finish().unwrap();
/// assert_eq!(doc.name(doc.first_child(doc.root()).unwrap()), "root");
/// ```
///
/// [`process_token`]: Builder::process_token
/// [`finish`]: Builder::finish
pub struct Builder {
    doc: Document,
    /// Stack of open elements.  `stack[0]` is always the document root.
    /// The "current node" is `stack.last()`.
    stack: Vec<NodeId>,
    /// Whether we have seen at least one root element.
    root_element_seen: bool,
    /// Namespace scope-stack, driven in lock-step with `stack`.
    ns: xml_ns::NsResolver,
}

impl Builder {
    /// Create a new builder with an empty document.
    pub fn new() -> Self {
        let doc = Document::new();
        let root = doc.root();
        Self {
            doc,
            stack: alloc::vec![root],
            root_element_seen: false,
            ns: xml_ns::NsResolver::new(),
        }
    }

    /// The node currently being built (top of the open-element stack).
    fn current(&self) -> NodeId {
        // Infallible because stack always has at least the document root.
        *self.stack.last().unwrap()
    }

    /// Feed one token into the builder.
    ///
    /// Tokens should be fed in the order emitted by [`xml_tokenizer::Tokenizer`].
    pub fn process_token(&mut self, token: xml_tokenizer::Token<'_>) -> Result<(), BuildError> {
        use xml_tokenizer::Token;
        match token {
            // XML declaration — recorded implicitly; no tree node needed.
            Token::XmlDecl { .. } => {}

            // DOCTYPE — we accept it but don't store it in the tree yet.
            Token::DoctypeDecl { .. } => {}

            Token::StartTag {
                name,
                raw_attrs,
                self_closing,
            } => {
                let parent = self.current();

                // Step 1 — register any xmlns declarations for this element so
                // that the resolver can resolve the element's own name.
                self.ns.push_element(parse_raw_attrs(raw_attrs));

                // Step 2 — resolve the element's qualified name.
                let eq = self.ns.resolve_element_name(name).map_err(
                    |xml_ns::NsError::UnboundPrefix(p)| BuildError::UnboundNamespacePrefix(p),
                )?;
                let elem = self.doc.append_element_ns(
                    parent,
                    eq.local.as_ref(),
                    eq.ns.as_ref().map(|u| u.as_str()).unwrap_or(""),
                );

                // Step 3 — attach attributes with namespace resolution.
                for (attr_name, attr_value) in parse_raw_attrs(raw_attrs) {
                    let aq = self.ns.resolve_attr_name(attr_name).map_err(
                        |xml_ns::NsError::UnboundPrefix(p)| BuildError::UnboundNamespacePrefix(p),
                    )?;
                    let decoded_value = decode_entities(attr_value)?;
                    self.doc.add_attr_ns(
                        elem,
                        aq.local.as_ref(),
                        aq.ns.as_ref().map(|u| u.as_str()).unwrap_or(""),
                        decoded_value.as_ref(),
                    );
                }

                if self_closing {
                    // Self-closing: push+pop the NS scope immediately.
                    self.ns.pop_element();
                } else {
                    self.stack.push(elem);
                }
                self.root_element_seen = true;
            }

            Token::EndTag { .. } => {
                // Pop the current element.  The stack always keeps the
                // document root (index 0), so len > 1 means we have an open
                // element to close.
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.ns.pop_element();
                } else {
                    return Err(BuildError::UnexpectedEndTag);
                }
            }

            Token::Text {
                raw,
                needs_unescape,
            } => {
                let parent = self.current();
                if needs_unescape {
                    let decoded = decode_entities(raw)?;
                    self.doc.append_text(parent, decoded.as_ref());
                } else {
                    self.doc.append_text(parent, raw);
                }
            }

            Token::Comment { content } => {
                let parent = self.current();
                self.doc.append_comment(parent, content);
            }

            Token::CData { content } => {
                let parent = self.current();
                self.doc.append_cdata(parent, content);
            }

            Token::ProcessingInstruction { target, data } => {
                let parent = self.current();
                self.doc.append_pi(parent, target, data.unwrap_or(""));
            }

            Token::Eof => {}

            // Token is #[non_exhaustive] — future variants are ignored.
            _ => {}
        }
        Ok(())
    }

    /// Finalise the builder and return the completed [`Document`].
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::NoRootElement`] if no element was ever seen.
    pub fn finish(self) -> Result<Document, BuildError> {
        if !self.root_element_seen {
            return Err(BuildError::NoRootElement);
        }
        Ok(self.doc)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Raw attribute parser
// ---------------------------------------------------------------------------

/// Parse `name="value"` / `name='value'` pairs from a raw attribute string.
///
/// This is a lazy iterator — pairs are parsed on demand.  Invalid or
/// malformed pairs are silently skipped (the tokenizer already validated
/// basic well-formedness at the byte level).
fn parse_raw_attrs(raw: &str) -> impl Iterator<Item = (&str, &str)> {
    // Rust note: `core::iter::from_fn` lets us build a stateful iterator
    // from a closure that captures mutable state — here `rest: &str`.
    // The closure returns `Some(item)` to yield, `None` to stop.
    let mut rest = raw.trim();
    core::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }

        // Scan the attribute name (everything up to `=` or whitespace).
        let name_end = rest.find(|c: char| c == '=' || c.is_ascii_whitespace())?;
        let name = &rest[..name_end];
        rest = rest[name_end..].trim_start();

        // Expect `=`.
        rest = rest.strip_prefix('=')?;
        rest = rest.trim_start();

        // Parse the quoted value.
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        rest = &rest[quote.len_utf8()..]; // consume opening quote

        let value_end = rest.find(quote)?;
        let value = &rest[..value_end];
        rest = &rest[value_end + quote.len_utf8()..]; // consume value + closing quote

        Some((name, value))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a document from an XML byte string.
    fn build(xml: &[u8]) -> Result<Document, alloc::string::String> {
        let mut tok = xml_tokenizer::Tokenizer::new(xml).map_err(|e| alloc::format!("{e:?}"))?;
        let mut builder = Builder::new();
        loop {
            let token = tok.next_token().map_err(|e| alloc::format!("{e:?}"))?;
            let done = token == xml_tokenizer::Token::Eof;
            builder
                .process_token(token)
                .map_err(|e| alloc::format!("{e:?}"))?;
            if done {
                break;
            }
        }
        builder.finish().map_err(|e| alloc::format!("{e:?}"))
    }

    // -----------------------------------------------------------------------
    // Document::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_document_has_root() {
        let doc = Document::new();
        let root = doc.root();
        assert_eq!(doc.kind(root), NodeKind::Document);
        assert_eq!(doc.name(root), "#document");
        assert!(doc.first_child(root).is_none());
    }

    // -----------------------------------------------------------------------
    // Name interning
    // -----------------------------------------------------------------------

    #[test]
    fn intern_name_same_string_same_id() {
        let mut doc = Document::new();
        let a = doc.intern_name("div");
        let b = doc.intern_name("div");
        assert_eq!(a, b);
    }

    #[test]
    fn intern_name_different_strings_different_ids() {
        let mut doc = Document::new();
        let a = doc.intern_name("div");
        let b = doc.intern_name("span");
        assert_ne!(a, b);
    }

    // -----------------------------------------------------------------------
    // Tree mutation
    // -----------------------------------------------------------------------

    #[test]
    fn append_element() {
        let mut doc = Document::new();
        let root = doc.root();
        let child = doc.append_element(root, "div");
        assert_eq!(doc.kind(child), NodeKind::Element);
        assert_eq!(doc.name(child), "div");
        assert_eq!(doc.parent(child), Some(root));
        assert_eq!(doc.first_child(root), Some(child));
        assert_eq!(doc.last_child(root), Some(child));
    }

    #[test]
    fn sibling_links() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");
        let c = doc.append_element(root, "c");

        // Forward chain
        assert_eq!(doc.next_sibling(a), Some(b));
        assert_eq!(doc.next_sibling(b), Some(c));
        assert_eq!(doc.next_sibling(c), None);

        // Backward chain
        assert_eq!(doc.prev_sibling(c), Some(b));
        assert_eq!(doc.prev_sibling(b), Some(a));
        assert_eq!(doc.prev_sibling(a), None);

        // Parent's first/last
        assert_eq!(doc.first_child(root), Some(a));
        assert_eq!(doc.last_child(root), Some(c));
    }

    #[test]
    fn append_text() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "p");
        let txt = doc.append_text(elem, "hello");
        assert_eq!(doc.kind(txt), NodeKind::Text);
        assert_eq!(doc.value(txt), "hello");
        assert_eq!(doc.name(txt), "#text");
    }

    #[test]
    fn append_comment() {
        let mut doc = Document::new();
        let root = doc.root();
        let c = doc.append_comment(root, " a comment ");
        assert_eq!(doc.kind(c), NodeKind::Comment);
        assert_eq!(doc.value(c), " a comment ");
        assert_eq!(doc.name(c), "#comment");
    }

    #[test]
    fn append_cdata() {
        let mut doc = Document::new();
        let root = doc.root();
        let e = doc.append_element(root, "r");
        let cd = doc.append_cdata(e, "<raw>");
        assert_eq!(doc.kind(cd), NodeKind::CData);
        assert_eq!(doc.value(cd), "<raw>");
        assert_eq!(doc.name(cd), "#cdata-section");
    }

    #[test]
    fn append_pi() {
        let mut doc = Document::new();
        let root = doc.root();
        let pi = doc.append_pi(root, "xml-stylesheet", "href=\"style.css\"");
        assert_eq!(doc.kind(pi), NodeKind::Pi);
        assert_eq!(doc.name(pi), "xml-stylesheet");
        assert_eq!(doc.value(pi), "href=\"style.css\"");
    }

    #[test]
    fn add_attr() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        doc.add_attr(elem, "href", "https://example.com");
        doc.add_attr(elem, "class", "link");

        let attrs = doc.attrs(elem);
        assert_eq!(attrs.len(), 2);
        assert_eq!(doc.attr_name(&attrs[0]), "href");
        assert_eq!(doc.attr_value(&attrs[0]), "https://example.com");
        assert_eq!(doc.attr_name(&attrs[1]), "class");
        assert_eq!(doc.attr_value(&attrs[1]), "link");
    }

    // -----------------------------------------------------------------------
    // ChildIter
    // -----------------------------------------------------------------------

    #[test]
    fn children_iterator() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");
        let c = doc.append_element(root, "c");

        let ids: Vec<NodeId> = doc.children(root).collect();
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn children_of_leaf_is_empty() {
        let mut doc = Document::new();
        let root = doc.root();
        let e = doc.append_element(root, "leaf");
        assert_eq!(doc.children(e).count(), 0);
    }

    // -----------------------------------------------------------------------
    // Builder
    // -----------------------------------------------------------------------

    #[test]
    fn builder_simple_element() {
        let doc = build(b"<root/>").unwrap();
        let root_elem = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.name(root_elem), "root");
        assert_eq!(doc.kind(root_elem), NodeKind::Element);
    }

    #[test]
    fn builder_nested_elements() {
        let doc = build(b"<a><b><c/></b></a>").unwrap();
        let a = doc.first_child(doc.root()).unwrap();
        let b = doc.first_child(a).unwrap();
        let c = doc.first_child(b).unwrap();
        assert_eq!(doc.name(a), "a");
        assert_eq!(doc.name(b), "b");
        assert_eq!(doc.name(c), "c");
        assert_eq!(doc.first_child(c), None);
    }

    #[test]
    fn builder_text_content() {
        let doc = build(b"<p>hello world</p>").unwrap();
        let p = doc.first_child(doc.root()).unwrap();
        let txt = doc.first_child(p).unwrap();
        assert_eq!(doc.kind(txt), NodeKind::Text);
        assert_eq!(doc.value(txt), "hello world");
    }

    #[test]
    fn builder_attributes() {
        let doc = build(b"<a href=\"x\" class=\"y\"/>").unwrap();
        let a = doc.first_child(doc.root()).unwrap();
        let attrs = doc.attrs(a);
        assert_eq!(attrs.len(), 2);
        assert_eq!(doc.attr_name(&attrs[0]), "href");
        assert_eq!(doc.attr_value(&attrs[0]), "x");
        assert_eq!(doc.attr_name(&attrs[1]), "class");
        assert_eq!(doc.attr_value(&attrs[1]), "y");
    }

    #[test]
    fn builder_comment_and_pi() {
        let doc = build(b"<r><!-- hi --><?foo bar?></r>").unwrap();
        let r = doc.first_child(doc.root()).unwrap();
        let children: Vec<_> = doc.children(r).collect();
        assert_eq!(children.len(), 2);
        assert_eq!(doc.kind(children[0]), NodeKind::Comment);
        assert_eq!(doc.value(children[0]), " hi ");
        assert_eq!(doc.kind(children[1]), NodeKind::Pi);
        assert_eq!(doc.name(children[1]), "foo");
        assert_eq!(doc.value(children[1]), "bar");
    }

    #[test]
    fn builder_cdata() {
        let doc = build(b"<r><![CDATA[<b>raw</b>]]></r>").unwrap();
        let r = doc.first_child(doc.root()).unwrap();
        let cd = doc.first_child(r).unwrap();
        assert_eq!(doc.kind(cd), NodeKind::CData);
        assert_eq!(doc.value(cd), "<b>raw</b>");
    }

    #[test]
    fn builder_xml_decl_accepted() {
        let doc = build(b"<?xml version=\"1.0\"?><root/>").unwrap();
        // XML decl generates no tree node — root element is the first child.
        let elem = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.name(elem), "root");
    }

    #[test]
    fn builder_no_root_element_is_error() {
        let mut builder = Builder::new();
        builder.process_token(xml_tokenizer::Token::Eof).unwrap();
        assert!(matches!(builder.finish(), Err(BuildError::NoRootElement)));
    }

    #[test]
    fn builder_siblings_preserved() {
        let doc = build(b"<r><a/><b/><c/></r>").unwrap();
        let r = doc.first_child(doc.root()).unwrap();
        let names: Vec<&str> = doc.children(r).map(|id| doc.name(id)).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    // -----------------------------------------------------------------------
    // Namespace integration
    // -----------------------------------------------------------------------

    #[test]
    fn builder_namespaced_element() {
        // <svg:circle> with a declared prefix resolves to local="circle",
        // ns="http://www.w3.org/2000/svg".
        let doc =
            build(b"<root xmlns:svg=\"http://www.w3.org/2000/svg\"><svg:circle/></root>").unwrap();
        let root = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.name(root), "root");
        assert_eq!(doc.ns_uri(root), ""); // no default namespace

        let circle = doc.first_child(root).unwrap();
        assert_eq!(doc.name(circle), "circle");
        assert_eq!(doc.ns_uri(circle), "http://www.w3.org/2000/svg");
    }

    #[test]
    fn builder_default_namespace_inherited_by_elements() {
        let doc = build(b"<root xmlns=\"http://example.com\"><child/></root>").unwrap();
        let root = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.name(root), "root");
        assert_eq!(doc.ns_uri(root), "http://example.com");

        let child = doc.first_child(root).unwrap();
        assert_eq!(doc.name(child), "child");
        assert_eq!(doc.ns_uri(child), "http://example.com");
    }

    #[test]
    fn builder_unprefixed_attr_has_no_namespace() {
        // Unprefixed attributes are NOT in the default namespace (NS spec §6.2).
        let doc = build(b"<root xmlns=\"http://example.com\" id=\"x\"/>").unwrap();
        let root = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.ns_uri(root), "http://example.com"); // element has default ns
        let attrs = doc.attrs(root);
        // xmlns attr + id attr
        let id_attr = attrs.iter().find(|a| doc.attr_name(a) == "id").unwrap();
        assert_eq!(doc.attr_ns_uri(id_attr), ""); // unprefixed attr has NO namespace
    }

    #[test]
    fn builder_prefixed_attr_has_namespace() {
        let doc = build(b"<root xmlns:xlink=\"http://www.w3.org/1999/xlink\" xlink:href=\"u\"/>")
            .unwrap();
        let root = doc.first_child(doc.root()).unwrap();
        let attrs = doc.attrs(root);
        let href = attrs.iter().find(|a| doc.attr_name(a) == "href").unwrap();
        assert_eq!(doc.attr_ns_uri(href), "http://www.w3.org/1999/xlink");
    }

    #[test]
    fn builder_namespace_undeclared_in_inner_scope() {
        // Inner xmlns="" undeclares the default namespace.
        let doc = build(b"<root xmlns=\"http://example.com\"><inner xmlns=\"\"/></root>").unwrap();
        let root = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.ns_uri(root), "http://example.com");
        let inner = doc.first_child(root).unwrap();
        assert_eq!(doc.ns_uri(inner), ""); // undeclared → no namespace
    }

    // -----------------------------------------------------------------------
    // parse_raw_attrs
    // -----------------------------------------------------------------------

    #[test]
    fn parse_attrs_double_quoted() {
        let pairs: Vec<_> = parse_raw_attrs(r#"id="main" class="x y""#).collect();
        assert_eq!(pairs, [("id", "main"), ("class", "x y")]);
    }

    #[test]
    fn parse_attrs_single_quoted() {
        let pairs: Vec<_> = parse_raw_attrs("href='x' title='y'").collect();
        assert_eq!(pairs, [("href", "x"), ("title", "y")]);
    }

    #[test]
    fn parse_attrs_empty() {
        let pairs: Vec<_> = parse_raw_attrs("").collect();
        assert!(pairs.is_empty());
    }

    // -----------------------------------------------------------------------
    // decode_entities
    // -----------------------------------------------------------------------

    #[test]
    fn decode_no_entities_borrows() {
        // Fast path: no `&` → Borrowed (no allocation).
        let result = decode_entities("hello world").unwrap();
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "hello world");
    }

    #[test]
    fn decode_predefined_amp() {
        assert_eq!(decode_entities("a &amp; b").unwrap().as_ref(), "a & b");
    }

    #[test]
    fn decode_predefined_lt_gt() {
        assert_eq!(decode_entities("&lt;tag&gt;").unwrap().as_ref(), "<tag>");
    }

    #[test]
    fn decode_predefined_apos_quot() {
        assert_eq!(decode_entities("&apos;&quot;").unwrap().as_ref(), "'\"");
    }

    #[test]
    fn decode_decimal_reference() {
        // &#65; = 'A'
        assert_eq!(decode_entities("&#65;").unwrap().as_ref(), "A");
    }

    #[test]
    fn decode_hex_reference_lower() {
        // &#x41; = 'A'
        assert_eq!(decode_entities("&#x41;").unwrap().as_ref(), "A");
    }

    #[test]
    fn decode_hex_reference_upper() {
        // &#X41; = 'A'
        assert_eq!(decode_entities("&#X41;").unwrap().as_ref(), "A");
    }

    #[test]
    fn decode_invalid_codepoint_is_error() {
        // U+0000 is not a legal XML 1.0 character.
        assert_eq!(
            decode_entities("&#0;"),
            Err(BuildError::InvalidCharacterReference(0))
        );
    }

    #[test]
    fn decode_surrogate_is_error() {
        // U+D800 is a surrogate — illegal in XML 1.0 §2.2.
        assert_eq!(
            decode_entities("&#xD800;"),
            Err(BuildError::InvalidCharacterReference(0xD800))
        );
    }

    #[test]
    fn decode_unknown_entity_passthrough() {
        // Unknown named entity — passed through unchanged until DTD (#12).
        assert_eq!(decode_entities("&unknown;").unwrap().as_ref(), "&unknown;");
    }

    #[test]
    fn decode_mixed_content() {
        assert_eq!(
            decode_entities("a &amp; b &#x3C; c").unwrap().as_ref(),
            "a & b < c"
        );
    }

    // -----------------------------------------------------------------------
    // Entity decoding integration (via Builder)
    // -----------------------------------------------------------------------

    #[test]
    fn builder_text_entity_decoded() {
        let doc = build(b"<p>a &amp; b</p>").unwrap();
        let p = doc.first_child(doc.root()).unwrap();
        let txt = doc.first_child(p).unwrap();
        assert_eq!(doc.value(txt), "a & b");
    }

    #[test]
    fn builder_text_numeric_entity_decoded() {
        let doc = build(b"<p>&#x41;</p>").unwrap();
        let p = doc.first_child(doc.root()).unwrap();
        let txt = doc.first_child(p).unwrap();
        assert_eq!(doc.value(txt), "A");
    }

    #[test]
    fn builder_attr_entity_decoded() {
        let doc = build(b"<a href=\"a&amp;b\"/>").unwrap();
        let a = doc.first_child(doc.root()).unwrap();
        let attrs = doc.attrs(a);
        let href = attrs.iter().find(|a| doc.attr_name(a) == "href").unwrap();
        assert_eq!(doc.attr_value(href), "a&b");
    }

    // -----------------------------------------------------------------------
    // Serialization
    // -----------------------------------------------------------------------

    /// Parse `xml`, serialize with default options, reparse, and return both
    /// documents.  Used for structural round-trip assertions.
    fn roundtrip(xml: &[u8]) -> (Document, Document) {
        let original = build(xml).unwrap();
        let serialized = original.to_xml_string(&SerializeOptions::default());
        let reparsed = build(serialized.as_bytes()).unwrap();
        (original, reparsed)
    }

    #[test]
    fn serialize_simple_element() {
        let doc = build(b"<root/>").unwrap();
        assert_eq!(doc.to_xml_string(&SerializeOptions::default()), "<root/>");
    }

    #[test]
    fn serialize_element_with_text() {
        let doc = build(b"<p>hello</p>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<p>hello</p>"
        );
    }

    #[test]
    fn serialize_text_escaping() {
        let doc = build(b"<p>a &amp; b &lt; c</p>").unwrap();
        // Text content is decoded on parse, re-encoded on serialize.
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<p>a &amp; b &lt; c</p>"
        );
    }

    #[test]
    fn serialize_attr_escaping() {
        let doc = build(b"<a href=\"a&amp;b\"/>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<a href=\"a&amp;b\"/>"
        );
    }

    #[test]
    fn serialize_nested_elements() {
        let doc = build(b"<a><b><c/></b></a>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<a><b><c/></b></a>"
        );
    }

    #[test]
    fn serialize_comment() {
        let doc = build(b"<r><!-- hi --></r>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<r><!-- hi --></r>"
        );
    }

    #[test]
    fn serialize_pi() {
        let doc = build(b"<r><?foo bar?></r>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<r><?foo bar?></r>"
        );
    }

    #[test]
    fn serialize_cdata() {
        let doc = build(b"<r><![CDATA[<b>raw</b>]]></r>").unwrap();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            "<r><![CDATA[<b>raw</b>]]></r>"
        );
    }

    #[test]
    fn serialize_cdata_split_on_cdata_end() {
        // Build a CDATA node whose content contains "]]>" directly (not via parsing,
        // since the XML parser would already split it).
        let mut doc = Document::new();
        let r = doc.append_element(doc.root(), "r");
        doc.append_cdata(r, "a]]>b");
        let xml = doc.to_xml_string(&SerializeOptions::default());
        // The serialized form must be re-parseable.
        let doc2 = build(xml.as_bytes()).unwrap();
        let r2 = doc2.first_child(doc2.root()).unwrap();
        // The split may produce multiple CDATA / text nodes; concatenate all values.
        let content: String = doc2.children(r2).map(|c| doc2.value(c)).collect();
        assert_eq!(content, "a]]>b");
    }

    #[test]
    fn serialize_xml_decl() {
        let opts = SerializeOptions {
            xml_decl: XmlDeclMode::Emit,
            declared_encoding: "UTF-8".into(),
            indent: None,
        };
        let doc = build(b"<root/>").unwrap();
        assert_eq!(
            doc.to_xml_string(&opts),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><root/>"
        );
    }

    #[test]
    fn serialize_indented() {
        let opts = SerializeOptions {
            indent: Some("  ".into()),
            ..SerializeOptions::default()
        };
        let doc = build(b"<a><b/></a>").unwrap();
        let xml = doc.to_xml_string(&opts);
        // Must contain a newline between elements.
        assert!(xml.contains('\n'), "expected indented output: {xml}");
        // Must be re-parseable.
        build(xml.as_bytes()).unwrap();
    }

    #[test]
    fn serialize_namespaced_element_roundtrip() {
        let (orig, reparsed) = roundtrip(b"<svg:circle xmlns:svg=\"http://www.w3.org/2000/svg\"/>");
        let orig_root = orig.first_child(orig.root()).unwrap();
        let rp_root = reparsed.first_child(reparsed.root()).unwrap();
        assert_eq!(orig.name(orig_root), reparsed.name(rp_root));
        assert_eq!(orig.ns_uri(orig_root), reparsed.ns_uri(rp_root));
    }

    #[test]
    fn serialize_default_namespace_roundtrip() {
        let (orig, reparsed) = roundtrip(b"<root xmlns=\"http://example.com\"><child/></root>");
        let orig_root = orig.first_child(orig.root()).unwrap();
        let rp_root = reparsed.first_child(reparsed.root()).unwrap();
        assert_eq!(orig.ns_uri(orig_root), reparsed.ns_uri(rp_root));
        let orig_child = orig.first_child(orig_root).unwrap();
        let rp_child = reparsed.first_child(rp_root).unwrap();
        assert_eq!(orig.ns_uri(orig_child), reparsed.ns_uri(rp_child));
    }

    #[test]
    fn serialize_node_subtree() {
        let doc = build(b"<a><b><c/></b></a>").unwrap();
        let a = doc.first_child(doc.root()).unwrap();
        let b = doc.first_child(a).unwrap();
        let xml = doc.serialize_node(b, &SerializeOptions::default());
        assert_eq!(xml, "<b><c/></b>");
    }

    // -----------------------------------------------------------------------
    // unlink_node / append_child
    // -----------------------------------------------------------------------

    #[test]
    fn unlink_middle_node() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");
        let c = doc.append_element(root, "c");

        doc.unlink_node(b);

        // b is fully detached.
        assert_eq!(doc.parent(b), None);
        assert_eq!(doc.prev_sibling(b), None);
        assert_eq!(doc.next_sibling(b), None);

        // a and c are still linked correctly.
        assert_eq!(doc.next_sibling(a), Some(c));
        assert_eq!(doc.prev_sibling(c), Some(a));
        assert_eq!(doc.first_child(root), Some(a));
        assert_eq!(doc.last_child(root), Some(c));
    }

    #[test]
    fn unlink_first_node() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");

        doc.unlink_node(a);

        assert_eq!(doc.first_child(root), Some(b));
        assert_eq!(doc.prev_sibling(b), None);
    }

    #[test]
    fn unlink_last_node() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");

        doc.unlink_node(b);

        assert_eq!(doc.last_child(root), Some(a));
        assert_eq!(doc.next_sibling(a), None);
    }

    #[test]
    fn unlink_and_reattach() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        let b = doc.append_element(root, "b");
        let container = doc.append_element(root, "container");

        doc.unlink_node(b);
        doc.append_child(container, b);

        assert_eq!(doc.parent(b), Some(container));
        assert_eq!(doc.first_child(container), Some(b));

        let root_children: Vec<NodeId> = doc.children(root).collect();
        assert_eq!(root_children, vec![a, container]);
    }

    // -----------------------------------------------------------------------
    // copy_subtree
    // -----------------------------------------------------------------------

    #[test]
    fn copy_subtree_basic() {
        let mut doc = Document::new();
        let root = doc.root();
        let src = doc.append_element(root, "source");
        doc.add_attr(src, "id", "1");
        doc.append_text(src, "hello");

        let dest = doc.append_element(root, "dest");
        let copy = doc.copy_subtree(src, dest);

        assert_ne!(copy, src);
        assert_eq!(doc.name(copy), "source");
        assert_eq!(doc.parent(copy), Some(dest));

        let copy_attrs = doc.attrs(copy);
        assert_eq!(copy_attrs.len(), 1);
        assert_eq!(doc.attr_name(&copy_attrs[0]), "id");
        assert_eq!(doc.attr_value(&copy_attrs[0]), "1");

        let copy_child = doc.first_child(copy).unwrap();
        assert_eq!(doc.kind(copy_child), NodeKind::Text);
        assert_eq!(doc.value(copy_child), "hello");
    }

    #[test]
    fn copy_subtree_preserves_deep_structure() {
        let mut doc = Document::new();
        let root = doc.root();
        let src = doc.append_element(root, "a");
        let b1 = doc.append_element(src, "b1");
        doc.append_element(b1, "c");
        doc.append_element(src, "b2");

        let dest = doc.append_element(root, "dest");
        let copy_a = doc.copy_subtree(src, dest);

        let child_names: Vec<&str> = doc.children(copy_a).map(|c| doc.name(c)).collect();
        assert_eq!(child_names, ["b1", "b2"]);

        let copy_b1 = doc.first_child(copy_a).unwrap();
        let grandchild = doc.first_child(copy_b1).unwrap();
        assert_eq!(doc.name(grandchild), "c");
    }

    #[test]
    fn copy_subtree_namespace_preserved() {
        let mut doc =
            build(b"<root xmlns:svg=\"http://www.w3.org/2000/svg\"><svg:circle r=\"5\"/></root>")
                .unwrap();
        let root_elem = doc.first_child(doc.root()).unwrap();
        let circle = doc.first_child(root_elem).unwrap();

        let copy = doc.copy_subtree(circle, root_elem);

        assert_eq!(doc.name(copy), "circle");
        assert_eq!(doc.ns_uri(copy), "http://www.w3.org/2000/svg");
        let copy_attrs = doc.attrs(copy);
        assert_eq!(doc.attr_value(&copy_attrs[0]), "5");
    }

    // -----------------------------------------------------------------------
    // deep_clone
    // -----------------------------------------------------------------------

    #[test]
    fn deep_clone_structural_equality() {
        let doc = build(b"<root id=\"x\"><child>text</child></root>").unwrap();
        let clone = doc.deep_clone();

        let orig_elem = doc.first_child(doc.root()).unwrap();
        let clone_elem = clone.first_child(clone.root()).unwrap();

        assert_eq!(doc.name(orig_elem), clone.name(clone_elem));
        assert_eq!(doc.ns_uri(orig_elem), clone.ns_uri(clone_elem));

        let orig_attrs = doc.attrs(orig_elem);
        let clone_attrs = clone.attrs(clone_elem);
        assert_eq!(orig_attrs.len(), clone_attrs.len());
        assert_eq!(
            doc.attr_value(&orig_attrs[0]),
            clone.attr_value(&clone_attrs[0])
        );

        let orig_child = doc.first_child(orig_elem).unwrap();
        let clone_child = clone.first_child(clone_elem).unwrap();
        let orig_text = doc.first_child(orig_child).unwrap();
        let clone_text = clone.first_child(clone_child).unwrap();
        assert_eq!(doc.value(orig_text), clone.value(clone_text));
    }

    #[test]
    fn deep_clone_is_independent() {
        let doc = build(b"<root/>").unwrap();
        let mut clone = doc.deep_clone();

        let clone_elem = clone.first_child(clone.root()).unwrap();
        clone.set_attr(clone_elem, "added", "yes");
        assert_eq!(clone.attrs(clone_elem).len(), 1);

        // Original is unaffected.
        let orig_elem = doc.first_child(doc.root()).unwrap();
        assert_eq!(doc.attrs(orig_elem).len(), 0);
    }

    #[test]
    fn deep_clone_roundtrip_xml() {
        let xml = b"<a id=\"1\"><b>text</b><!-- note --></a>";
        let doc = build(xml).unwrap();
        let clone = doc.deep_clone();
        assert_eq!(
            doc.to_xml_string(&SerializeOptions::default()),
            clone.to_xml_string(&SerializeOptions::default())
        );
    }

    // -----------------------------------------------------------------------
    // set_attr / remove_attr
    // -----------------------------------------------------------------------

    #[test]
    fn set_attr_appends_new() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        doc.set_attr(elem, "href", "https://example.com");

        let attrs = doc.attrs(elem);
        assert_eq!(attrs.len(), 1);
        assert_eq!(doc.attr_name(&attrs[0]), "href");
        assert_eq!(doc.attr_value(&attrs[0]), "https://example.com");
    }

    #[test]
    fn set_attr_updates_existing() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        doc.add_attr(elem, "href", "old");
        doc.set_attr(elem, "href", "new");

        let attrs = doc.attrs(elem);
        assert_eq!(attrs.len(), 1, "no duplicate attr should be created");
        assert_eq!(doc.attr_value(&attrs[0]), "new");
    }

    #[test]
    fn set_attr_does_not_corrupt_sibling_attrs() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        doc.add_attr(a, "x", "1");
        let b = doc.append_element(root, "b");
        doc.add_attr(b, "y", "2");

        // Add a second attr to 'a' now that 'b' already has attrs.
        doc.set_attr(a, "z", "3");

        let a_attrs = doc.attrs(a);
        assert_eq!(a_attrs.len(), 2);
        assert_eq!(doc.attr_name(&a_attrs[0]), "x");
        assert_eq!(doc.attr_value(&a_attrs[0]), "1");
        assert_eq!(doc.attr_name(&a_attrs[1]), "z");
        assert_eq!(doc.attr_value(&a_attrs[1]), "3");

        let b_attrs = doc.attrs(b);
        assert_eq!(b_attrs.len(), 1);
        assert_eq!(doc.attr_name(&b_attrs[0]), "y");
        assert_eq!(doc.attr_value(&b_attrs[0]), "2");
    }

    #[test]
    fn set_attr_ns_new() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        doc.set_attr_ns(elem, "href", "http://www.w3.org/1999/xlink", "url");

        let attrs = doc.attrs(elem);
        assert_eq!(attrs.len(), 1);
        assert_eq!(doc.attr_name(&attrs[0]), "href");
        assert_eq!(doc.attr_ns_uri(&attrs[0]), "http://www.w3.org/1999/xlink");
        assert_eq!(doc.attr_value(&attrs[0]), "url");
    }

    #[test]
    fn remove_attr_existing() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        doc.add_attr(elem, "href", "x");
        doc.add_attr(elem, "class", "y");

        let removed = doc.remove_attr(elem, "href");
        assert!(removed);

        let attrs = doc.attrs(elem);
        assert_eq!(attrs.len(), 1);
        assert_eq!(doc.attr_name(&attrs[0]), "class");
        assert_eq!(doc.attr_value(&attrs[0]), "y");
    }

    #[test]
    fn remove_attr_nonexistent() {
        let mut doc = Document::new();
        let root = doc.root();
        let elem = doc.append_element(root, "a");
        assert!(!doc.remove_attr(elem, "href"));
    }

    #[test]
    fn remove_attr_does_not_corrupt_sibling_attrs() {
        let mut doc = Document::new();
        let root = doc.root();
        let a = doc.append_element(root, "a");
        doc.add_attr(a, "x", "1");
        doc.add_attr(a, "y", "2");
        let b = doc.append_element(root, "b");
        doc.add_attr(b, "z", "3");

        doc.remove_attr(a, "x");

        let a_attrs = doc.attrs(a);
        assert_eq!(a_attrs.len(), 1);
        assert_eq!(doc.attr_name(&a_attrs[0]), "y");
        assert_eq!(doc.attr_value(&a_attrs[0]), "2");

        let b_attrs = doc.attrs(b);
        assert_eq!(b_attrs.len(), 1);
        assert_eq!(doc.attr_name(&b_attrs[0]), "z");
        assert_eq!(doc.attr_value(&b_attrs[0]), "3");
    }

    #[test]
    fn mutation_roundtrip_xml() {
        // Build a doc, mutate it via the new API, and verify the serialized XML.
        let mut doc = build(b"<root><child id=\"old\"/></root>").unwrap();
        let root_elem = doc.first_child(doc.root()).unwrap();
        let child = doc.first_child(root_elem).unwrap();

        doc.set_attr(child, "id", "new");
        doc.set_attr(child, "extra", "yes");

        let xml = doc.to_xml_string(&SerializeOptions::default());
        let reparsed = build(xml.as_bytes()).unwrap();
        let rp_root = reparsed.first_child(reparsed.root()).unwrap();
        let rp_child = reparsed.first_child(rp_root).unwrap();

        let rp_attrs = reparsed.attrs(rp_child);
        assert_eq!(rp_attrs.len(), 2);
        let id_val = rp_attrs
            .iter()
            .find(|a| reparsed.attr_name(a) == "id")
            .map(|a| reparsed.attr_value(a))
            .unwrap_or("");
        assert_eq!(id_val, "new");
    }
}
