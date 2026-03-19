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
    fn append_child(&mut self, parent: NodeId, child: NodeId) {
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
}
