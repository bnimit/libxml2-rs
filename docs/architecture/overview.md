# libxml2-rs: A Memory-Safe Rust Replacement for libxml2

**Version:** 1.0
**Status:** Draft for Review
**License:** Apache License 2.0

---

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Strategic Motivation](#2-strategic-motivation)
3. [Scope and Feature Parity Matrix](#3-scope-and-feature-parity-matrix)
4. [Crate Workspace Architecture](#4-crate-workspace-architecture)
5. [Core Technical Design Decisions](#5-core-technical-design-decisions)
6. [C ABI Compatibility Layer](#6-c-abi-compatibility-layer)
7. [Phased Delivery Roadmap](#7-phased-delivery-roadmap)
8. [Testing Strategy](#8-testing-strategy)
9. [Documentation Strategy](#9-documentation-strategy)
10. [Open Source Governance](#10-open-source-governance)
11. [Risk Register](#11-risk-register)
12. [Appendix: Key Reference Files and Locations](#12-appendix)

---

## 1. Executive Summary

**libxml2** is the de-facto XML processing library of the internet. It is a system library on Linux, macOS, and every Android device; it powers Python's `lxml`, PHP's XML stack, GNOME, LibreOffice, countless language runtimes, and an estimated 50,000+ software packages. It is also one of the most CVE-laden libraries in existence, with hundreds of memory-safety vulnerabilities across its 25-year history — including heap overflows, use-after-free, XXE injection, and billion-laughs DoS — nearly all of which are structural consequences of being written in C.

**libxml2-rs** is a ground-up, pure-Rust reimplementation of libxml2's full public API surface, designed to:

- **Eliminate the entire class of memory-safety vulnerabilities** that make libxml2 a perennial security liability
- **Serve as a drop-in binary replacement** via an exact C ABI compatibility layer
- **Exceed libxml2 performance** on modern hardware via SIMD scanning, arena allocation, and zero-copy parsing
- **Achieve W3C/OASIS conformance parity** with libxml2 2.x on all normative test suites
- **Be adopted as an OpenSSF-aligned open source project** under Apache 2.0 with DCO governance

This document describes the full technical architecture, phased delivery roadmap, testing strategy, documentation plan, and governance model for this initiative.

---

## 2. Strategic Motivation

### 2.1 The Security Case

libxml2's CVE history is not an indictment of its maintainers — it is structural. The library manages complex, parser-driven heap allocation of tree structures in C, which is inherently vulnerable to:

| Attack Class | Root Cause | Rust Eliminates? |
|---|---|---|
| Buffer overflows | Manual bounds checking | ✅ Yes (bounds-checked slices) |
| Heap use-after-free | Manual lifetime management | ✅ Yes (ownership) |
| Double-free | Manual memory deallocation | ✅ Yes (single owner) |
| XXE / entity injection | Design issue (configurable) | ⚠️ Mitigated by design |
| Billion-laughs DoS | Entity expansion limits | ⚠️ Mitigated by design |
| Integer overflow | C integer semantics | ✅ Yes (checked arithmetic) |
| Null pointer dereference | No null safety | ✅ Yes (Option<T>) |
| Format string bugs | printf-family | ✅ Yes (Rust formatting) |

Representative CVEs motivating this work:

| CVE | Class | Component |
|---|---|---|
| CVE-2024-25062 | Use-after-free | Streaming validation |
| CVE-2023-29469 | Use-after-free | `xmlHaltParser` |
| CVE-2023-28484 | NULL deref | XSD complex type fixup |
| CVE-2022-40304 | Use-after-free | Dict string deduplication |
| CVE-2022-40303 | Integer overflow | `xmlParseNameComplex` |
| CVE-2022-29824 | Integer overflow → buffer over-read | Multiple functions |
| CVE-2021-3541 | Algorithmic DoS | Entity expansion (billion-laughs) |
| CVE-2021-3518 | Use-after-free | XInclude copy range |
| CVE-2021-3517 | Heap buffer overflow | `xmlEncodeEntitiesInternal` |
| CVE-2017-9047/9048 | Buffer overflow | `xmlSnprintfElementContent` |
| CVE-2016-4448 | Format string | Serializer |
| CVE-2016-1762 | Heap overflow | `xmlNextChar` |
| CVE-2015-7942 | Heap buffer overflow | Conditional sections parser |

**Highest-risk subsystems** (where most CVEs originate — implementation must be especially rigorous):
1. **Push/chunked parser** — complex incremental state machine
2. **Entity expansion** — recursive, requires hard algorithmic limits
3. **XInclude** — cross-node references, tricky node lifecycle
4. **XSD schema validation** — complex fixup passes with back-references
5. **HTML lenient parser** — highly stateful error recovery

Rust's borrow checker and type system eliminate the **entire bug class** responsible for >90% of these CVEs. Algorithmic DoS (billion-laughs) requires application-level limits regardless of language — these are built into `ParserOptions::default()`.

### 2.2 The Performance Opportunity

Modern Rust XML parsers (quick-xml) already **outperform libxml2** in event-streaming mode by 2-4x on x86-64 with SIMD. A carefully engineered Rust DOM implementation using arena allocation will match or exceed libxml2's DOM performance while using significantly less memory (no per-node malloc overhead).

### 2.3 The Ecosystem Gap

As of 2026, the Rust XML ecosystem is fragmented and incomplete:

| Feature | Pure Rust Available | Quality |
|---|---|---|
| XML streaming (SAX-like) | quick-xml, xml-rs | Production-grade |
| Read-only DOM | roxmltree | Production-grade |
| Mutable DOM | minidom, xmltree | Limited |
| HTML5 parsing | html5ever | Production-grade |
| XPath 1.0 | sxd-xpath | **Stalled, incomplete** |
| XSD validation | — | **Does not exist** |
| DTD validation | — | **Does not exist** |
| RelaxNG | — | **Does not exist** |
| XSLT 1.0 | — | **Does not exist** |
| XInclude | — | **Does not exist** |
| C14N | — | **Does not exist** |
| XML Catalogs | — | **Does not exist** |
| C ABI compatibility | libxml (bindings only) | **Bindings, not replacement** |

libxml2-rs fills every gap in this table.

---

## 3. Scope and Feature Parity Matrix

The following table defines the complete feature scope. Each feature is assigned a Phase (1–4) corresponding to the delivery roadmap in Section 7.

### 3.1 Parsing Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| XML 1.0 well-formedness parsing | `xmlParseDoc`, `xmlParseFile`, `xmlParseMemory` | 1 | Core milestone |
| XML 1.1 parsing | `xmlParseDoc` with version detection | 2 | Less common; after 1.0 conformance |
| SAX2 event interface | `xmlSAXHandler` (30 callbacks) | 1 | Streaming interface |
| DOM tree (xmlDoc/xmlNode) | `xmlParseDoc` → tree | 1 | Mutable document tree |
| Push parser (chunked input) | `xmlParseChunk`, `xmlCreatePushParserCtxt` | 1 | Network streaming use case |
| Pull/Reader interface | `xmlTextReader`, `xmlNewTextReader` | 2 | XmlReader/.NET-style cursor |
| Writer interface | `xmlTextWriter`, `xmlNewTextWriter` | 2 | Streaming XML generation |
| HTML lenient parser | `htmlParseDoc`, `htmlReadMemory` | 2 | Pre-HTML5 error recovery |
| HTML5-compliant parser | (not in libxml2) | 3 | Use html5ever as foundation |
| Namespace 1.0 support | Automatic in parser | 1 | Required for SAX2/DOM |
| Namespace 1.1 support | Automatic | 2 | |
| Entity expansion (internal) | Controlled in `xmlParserCtxt` | 1 | With safe limits by default |
| Entity expansion (external) | `xmlSetExternalEntityLoader` | 2 | Disabled by default (XXE safety) |
| Encoding detection/transcoding | `xmlCharEncodingHandler` | 1 | Use `encoding_rs` crate |
| gzip transparent decompression | `xmlParseFile("foo.xml.gz")` | 3 | Via `flate2` crate |
| Custom I/O handlers | `xmlRegisterInputCallbacks` | 2 | Trait-based in Rust |

### 3.2 Tree / Serialization Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| Tree traversal (parent/child/sibling) | `xmlNode` linked list navigation | 1 | Core tree API |
| Tree mutation (add/remove/move nodes) | `xmlAddChild`, `xmlUnlinkNode` | 1 | |
| Attribute manipulation | `xmlGetProp`, `xmlSetProp` | 1 | |
| Serialization (tree → string) | `xmlDocDumpMemory`, `xmlNodeDump` | 1 | |
| Serialization (tree → file) | `xmlSaveFile`, `xmlSaveDoc` | 1 | |
| Serialization formatting/indent | `xmlKeepBlanksDefault`, `xmlSaveFormatFile` | 1 | |
| Namespace reconciliation | `xmlReconciliateNs` | 2 | |
| Document copy/clone | `xmlCopyDoc`, `xmlCopyNode` | 2 | |

### 3.3 Validation Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| DTD validation (internal subset) | `xmlValidCtxt`, `xmlValidateDtd` | 2 | Attribute defaults, content models |
| DTD validation (external subset) | `xmlParseDTD` | 2 | |
| XML Schema (XSD 1.0) validation | `xmlSchemaValidCtxt`, `xmlSchemaNewDocParserCtxt` | 3 | Largest validation effort |
| RelaxNG validation | `xmlRelaxNGValidCtxt` | 3 | Both XML and compact syntax |
| Schematron validation | `xmlSchematronValidCtxt` | 4 | ISO Schematron subset |

### 3.4 XPath / Query Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| XPath 1.0 evaluation | `xmlXPathEval`, `xmlXPathCompile` | 2 | Full spec compliance required |
| XPath extension functions | `xmlXPathRegisterFunc` | 2 | Pluggable function registry |
| XPath variables | `xmlXPathRegisterVariable` | 2 | |
| XPointer framework | `xmlXPointerEval` | 3 | Used in XLink |
| Pattern matching | `xmlPattern`, `xmlPatternMatch` | 2 | Simplified XPath patterns |

### 3.5 Transformation Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| XSLT 1.0 (libxslt) | `xsltApplyStylesheet` | 4 | Separate libxslt companion; massive scope |
| XInclude 1.0 | `xmlXIncludeProcess` | 3 | xi:include substitution |
| C14N (original + exclusive) | `xmlC14NDocSave` | 3 | Needed for XML-DSIG |

### 3.6 Infrastructure Layer

| Feature | libxml2 API | Phase | Notes |
|---|---|---|---|
| XML Catalogs | `xmlLoadCatalog`, `xmlCatalogResolve` | 3 | OASIS XML Catalogs |
| URI handling | `xmlParseURI`, `xmlBuildURI` | 1 | Needed by all other layers |
| Error reporting system | `xmlError`, `xmlSetGenericErrorFunc` | 1 | Structured errors with location |
| Custom memory allocators | `xmlMemSetup` | 2 | For embedding in constrained envs |
| Thread safety | Global init/cleanup | 1 | `Send + Sync` by design |
| xmllint equivalent (CLI) | `xmllint` binary | 2 | Validation, formatting, XPath CLI |

---

## 4. Crate Workspace Architecture

### 4.1 Workspace Layout

```
libxml2-rs/                              # Git repository root
├── Cargo.toml                           # Workspace manifest
├── Cargo.lock                           # Committed for reproducibility
├── LICENSE                              # Apache 2.0
├── DCO                                  # Developer Certificate of Origin
├── CONTRIBUTING.md
├── CODE_OF_CONDUCT.md                   # Contributor Covenant 2.1
├── SECURITY.md                          # Responsible disclosure process
├── MAINTAINERS.md
│
├── crates/
│   ├── xml-chars/                       # Unicode XML character class tables (no_std)
│   ├── xml-tokenizer/                   # Low-level byte scanner (no_std + alloc)
│   ├── xml-ns/                          # Namespace URI/prefix types (no_std)
│   ├── xml-tree/                        # Arena-based mutable DOM (no_std + alloc)
│   ├── xml-xpath/                       # XPath 1.0 evaluator
│   ├── xml-schema/                      # W3C XML Schema (XSD) validation
│   ├── xml-relaxng/                     # RelaxNG (XML + compact syntax)
│   ├── xml-xinclude/                    # XInclude processing
│   ├── xml-c14n/                        # Canonical XML (C14N 1.0 + Exclusive)
│   ├── xml-catalog/                     # OASIS XML Catalogs
│   ├── html-parser/                     # HTML5 parser (wraps html5ever)
│   ├── libxml2-rs/                      # Unified facade — the main user-facing crate
│   └── libxml2-rs-c/                    # C ABI layer — cdylib/staticlib target
│
├── tools/
│   └── xmllint-rs/                      # xmllint replacement CLI (binary crate)
│
├── fuzz/
│   ├── Cargo.toml
│   └── fuzz_targets/
│       ├── parse_xml.rs
│       ├── parse_html.rs
│       ├── xpath_eval.rs
│       ├── schema_validate.rs
│       ├── roundtrip.rs
│       └── c_api_fuzz.rs                # Fuzz the C ABI layer
│
├── benches/
│   ├── parse_throughput.rs
│   ├── dom_operations.rs
│   └── xpath_eval.rs
│
├── tests/
│   ├── xmlconf/                         # W3C XML 1.0 conformance test suite
│   ├── xmlconf_1_1/                     # W3C XML 1.1 tests
│   ├── oasis/                           # OASIS XML test suite
│   ├── w3c-xsd/                         # W3C XML Schema test suite (XSTS)
│   ├── relaxng/                         # OASIS RelaxNG test suite
│   ├── xslt/                            # OASIS XSLT 1.0 + XPath tests
│   ├── html5lib-tests/                  # HTML5 parsing conformance
│   ├── c14n/                            # W3C C14N test vectors
│   ├── regression/                      # CVE regression tests
│   └── differential/                    # Compare output vs libxml2 reference
│
└── docs/
    ├── guide/                           # mdBook user guide
    │   ├── book.toml
    │   └── src/
    ├── api-migration/                   # C API → Rust API mapping
    └── security/                        # Security model documentation
```

### 4.2 Workspace Cargo.toml

```toml
[workspace]
resolver = "2"
members = [
    "crates/xml-chars",
    "crates/xml-tokenizer",
    "crates/xml-ns",
    "crates/xml-tree",
    "crates/xml-xpath",
    "crates/xml-schema",
    "crates/xml-relaxng",
    "crates/xml-xinclude",
    "crates/xml-c14n",
    "crates/xml-catalog",
    "crates/html-parser",
    "crates/libxml2-rs",
    "crates/libxml2-rs-c",
    "tools/xmllint-rs",
    "fuzz",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
rust-version = "1.81"         # MSRV: core::error::Error stable
license = "Apache-2.0"
repository = "https://github.com/ibm/libxml2-rs"
homepage = "https://ibm.github.io/libxml2-rs"

[workspace.dependencies]
# Parsing / scanning
memchr = "2.7"
encoding_rs = "0.8"

# Data structures
bumpalo = { version = "3.16", features = ["collections"] }
indexmap = "2"
smallvec = { version = "1", features = ["union", "const_generics"] }

# Error handling
thiserror = "1"

# Async support
tokio = { version = "1", default-features = false, features = ["io-util"] }

# Testing
proptest = "1"
criterion = { version = "0.5", features = ["html_reports"] }
libfuzzer-sys = "0.4"

# C API
cbindgen = "0.26"

[profile.release]
lto = "fat"
codegen-units = 1
panic = "abort"               # Smaller binary, required for C-compatible library

[profile.bench]
inherits = "release"
debug = true                  # Needed for flamegraph profiling
```

### 4.3 Crate Dependency Graph

```
xml-chars          (no_std, no_alloc) — Unicode tables only
    ↑
xml-tokenizer      (no_std + alloc) — byte scanner, event emitter
    ↑         ↑
xml-ns          xml-tree   (no_std + alloc) — arena DOM
    ↑              ↑
    └──────────────┤
                   ↓
               xml-xpath  (std) — XPath 1.0
                   ↑
               xml-schema   xml-relaxng   xml-xinclude   xml-c14n
                   ↑              ↑              ↑            ↑
                   └──────────────┴──────────────┴────────────┘
                                  ↓
                           libxml2-rs (facade)
                                  ↑
                      libxml2-rs-c (C ABI layer)
```

---

## 5. Core Technical Design Decisions

### 5.1 Memory Model: Arena + Index Trees

The central challenge of a libxml2 replacement is representing a **mutable, bidirectional tree** (parent ↔ child ↔ sibling pointers) in safe Rust. We reject `Rc<RefCell<Node>>` (used by sxd-document) due to cache-unfriendliness and reference-counting overhead. We choose the **arena + integer index** pattern:

```rust
// crates/xml-tree/src/lib.rs

/// A node identifier — a 32-bit index into the Document's node arena.
/// Cheap to copy (4 bytes). Valid only within the Document that created it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(u32);

/// A complete XML document. Owns all node and string data.
/// `Send + Sync` — safe to share across threads (read-only after parse).
pub struct Document {
    // Node storage — flat Vec, cache-friendly sequential allocation
    nodes: Vec<NodeData>,

    // String storage — bump allocator for attribute values, text content
    // NOTE: bumpalo does NOT call Drop; strings are just bytes, this is fine
    string_arena: bumpalo::Bump,

    // Name interning — element names, attribute names deduplicated
    // Maps &str → NameId (u32 index into name_table)
    name_table: IndexSet<Box<str>>,
    ns_table:   IndexSet<Box<str>>,   // Namespace URIs

    root: NodeId,
}

/// All data for a single node. 64 bytes — fits in a cache line.
#[repr(C)]
struct NodeData {
    kind:         NodeKind,           // u8
    _pad:         [u8; 3],
    name:         NameId,             // u32 — index into name_table
    ns:           Option<NsId>,       // u32 — index into ns_table
    parent:       Option<NodeId>,     // u32
    first_child:  Option<NodeId>,     // u32
    last_child:   Option<NodeId>,     // u32
    next_sibling: Option<NodeId>,     // u32
    prev_sibling: Option<NodeId>,     // u32
    // Text/value: range into string_arena
    value_offset: u32,
    value_len:    u32,
    // Attributes: range into a separate flat attrs Vec
    attrs_start:  u32,
    attrs_end:    u32,
}
// static_assert!(size_of::<NodeData>() <= 64);

#[repr(u8)]
enum NodeKind {
    Element = 1,
    Attribute = 2,
    Text = 3,
    CData = 4,
    EntityRef = 5,
    Entity = 6,
    Pi = 7,
    Comment = 8,
    Document = 9,
    DocumentType = 10,
    DocumentFrag = 11,
    Notation = 12,
    HtmlDocument = 13,
    Dtd = 14,
    Namespace = 18,
}
```

**Why this design wins:**
- `NodeId` is `Copy` — no borrow checker issues passing node references around
- All nodes in a flat `Vec` — sequential memory, prefetcher-friendly
- No `unsafe` needed for tree operations — just array indexing
- `Document: Send + Sync` trivially (no raw pointers in public API)
- C API maps cleanly: `*mut xmlDoc` ↔ `Box<Document>`; `*mut xmlNode` ↔ `(*mut Document, NodeId)` packed into a struct

### 5.2 Zero-Copy Tokenizer

The tokenizer borrows from input, yielding events as slices:

```rust
// crates/xml-tokenizer/src/lib.rs

pub enum Token<'input> {
    Decl { version: &'input str, encoding: Option<&'input str>, standalone: Option<bool> },
    DoctypeDecl { name: &'input str, public_id: Option<&'input str>, system_id: Option<&'input str> },
    StartTag { name: QName<'input>, attrs: AttrIter<'input>, self_closing: bool },
    EndTag   { name: QName<'input> },
    Text     { raw: &'input str, needs_entity_decode: bool },
    CData    { content: &'input str },
    Comment  { content: &'input str },
    Pi       { target: &'input str, data: Option<&'input str> },
    Eof,
}

pub struct Tokenizer<'input> {
    input: &'input [u8],
    pos:   usize,
    // SIMD-accelerated byte search via memchr
}
```

Entity decoding is deferred: the `needs_entity_decode` flag avoids allocating strings for plain text nodes. Only nodes containing `&` or `<` entities allocate.

### 5.3 Encoding Layer

All XML-required encodings are handled via the `encoding_rs` crate (written by Henri Sivonen, used in Firefox). This provides:
- UTF-8, UTF-16 LE/BE, ISO-8859-{1..16}, Windows-125x, and all WHATWG-required encodings
- Battle-tested, SIMD-accelerated, extensively fuzzed
- BOM detection

Input is always transcoded to UTF-8 at the I/O boundary before tokenization. Internally, all strings are UTF-8.

### 5.4 XPath 1.0 Engine Architecture

XPath requires particular care for performance. We compile XPath expressions to a **bytecode representation** rather than `Box<dyn Expression>` (which adds vtable overhead):

```rust
// crates/xml-xpath/src/engine.rs

/// Compiled XPath expression — bytecode for fast repeated evaluation
pub struct CompiledXPath {
    ops: Vec<Op>,          // Instruction sequence
    consts: Vec<XValue>,   // Constant pool (strings, numbers)
}

#[repr(u8)]
enum Op {
    // Navigation
    AxisChild,
    AxisDescendant,
    AxisParent,
    AxisAttribute,
    AxisSelf,
    AxisFollowingSibling,
    // Predicates
    Predicate,
    // Tests
    NameTest(u32),         // Index into const pool (interned name)
    NodeTypeTest(u8),
    // Functions
    CallBuiltin(u8),       // Index into builtin function table
    CallExtension(u32),    // Index into registered extension functions
    // Arithmetic
    Add, Sub, Mul, Div, Mod, Neg,
    // Comparison
    Eq, Ne, Lt, Le, Gt, Ge,
    // Logic
    And, Or, Not,
    // Value
    PushConst(u32),        // Push constant at index
    PushContextNode,
    PushContextPosition,
    PushContextSize,
    // Result
    ReturnNodeSet,
    ReturnString,
    ReturnNumber,
    ReturnBoolean,
}

pub struct XPathContext<'doc> {
    doc:       &'doc Document,
    node:      NodeId,
    position:  usize,
    size:      usize,
    variables: HashMap<QName<'static>, XValue>,
    functions: FunctionRegistry,
}
```

### 5.5 Error Handling

Structured errors matching libxml2's error system:

```rust
// crates/libxml2-rs/src/error.rs

#[derive(Debug, Clone, thiserror::Error)]
#[error("{domain}:{code} at {file}:{line}:{col} — {message}")]
pub struct XmlError {
    pub domain:  ErrorDomain,
    pub code:    u32,           // Compatible with libxml2 xmlParserErrors values
    pub level:   ErrorLevel,    // Warning, Error, Fatal
    pub message: String,
    pub file:    Option<String>,
    pub line:    u32,
    pub col:     u32,
    pub node:    Option<NodeId>,
}

#[repr(u32)]
pub enum ErrorDomain {
    Parser      = 1,
    Tree        = 2,
    Namespace   = 3,
    Dtd         = 4,
    XPath       = 5,
    XPointer    = 6,
    XInclude    = 7,
    Io          = 8,
    Catalog     = 9,
    C14n        = 10,
    Xslt        = 11,
    Valid       = 12,
    Check       = 13,
    Writer      = 14,
    Module      = 15,
    I18n        = 16,
    Schematron  = 17,
    Relaxng     = 18,
    Schemas     = 19,
    Html        = 20,
    // ... matching libxml2 values
}
```

Error reporting uses a **structured callback system** matching libxml2's `xmlSetGenericErrorFunc` pattern, but with a Rust-idiomatic trait:

```rust
pub trait ErrorHandler: Send + Sync {
    fn handle(&self, error: &XmlError);
}

// Default: log via the `log` crate (not eprintln)
pub struct LogErrorHandler;
impl ErrorHandler for LogErrorHandler {
    fn handle(&self, e: &XmlError) {
        match e.level {
            ErrorLevel::Warning => log::warn!("{}", e),
            ErrorLevel::Error | ErrorLevel::Fatal => log::error!("{}", e),
        }
    }
}
```

### 5.6 Security-by-Default Configuration

libxml2's default configuration is historically permissive (XXE enabled, no entity expansion limits). We invert this:

```rust
/// Parser options — secure defaults, opt-in for dangerous features.
#[derive(Debug, Clone)]
pub struct ParserOptions {
    /// Maximum entity expansion depth. Default: 100.
    pub max_entity_depth: usize,

    /// Maximum total entity expansion size (bytes). Default: 10 MB.
    /// Prevents billion-laughs attacks.
    pub max_entity_expansion: usize,

    /// Allow loading external entities (XXE). Default: FALSE.
    /// Must be explicitly enabled. When enabled, use `entity_resolver`.
    pub allow_external_entities: bool,

    /// Custom external entity resolver. Only called if `allow_external_entities`.
    pub entity_resolver: Option<Arc<dyn EntityResolver>>,

    /// Maximum document nesting depth. Default: 10,000.
    pub max_nesting_depth: usize,

    /// Maximum attribute count per element. Default: 10,000.
    pub max_attrs_per_element: usize,

    /// Substitute entities in returned text. Default: true.
    pub substitute_entities: bool,

    /// Load external DTD subset. Default: false.
    pub load_external_dtd: bool,
}

impl Default for ParserOptions {
    fn default() -> Self {
        Self {
            max_entity_depth: 100,
            max_entity_expansion: 10 * 1024 * 1024,
            allow_external_entities: false,   // XXE-safe by default
            entity_resolver: None,
            max_nesting_depth: 10_000,
            max_attrs_per_element: 10_000,
            substitute_entities: true,
            load_external_dtd: false,
        }
    }
}

/// Compatibility mode: replicate libxml2's (insecure) defaults
pub fn libxml2_compat_options() -> ParserOptions {
    ParserOptions {
        allow_external_entities: true,
        load_external_dtd: true,
        max_entity_expansion: usize::MAX,
        max_entity_depth: usize::MAX,
        ..Default::default()
    }
}
```

---

## 6. C ABI Compatibility Layer

### 6.1 Strategy

The `libxml2-rs-c` crate builds as a `cdylib` (shared library) and `staticlib` with:
- **Identical symbol names** to libxml2 2.x (`xmlParseDoc`, `xmlFreeDoc`, etc.)
- **Identical `#[repr(C)]` struct layouts** for all public data structures
- **Version script** exporting exactly the libxml2 public symbol set
- **cbindgen-generated header** at `include/libxml/parser.h`, `include/libxml/tree.h`, etc.

This enables **binary drop-in replacement**:
```bash
# Test compatibility via LD_PRELOAD
LD_PRELOAD=/usr/local/lib/libxml2_rs.so ./application

# Or replace system library (careful!)
sudo cp target/release/libxml2.so /usr/lib/x86_64-linux-gnu/libxml2.so.2
```

### 6.2 Key C API Mapping Patterns

```rust
// crates/libxml2-rs-c/src/parser.rs

/// xmlDocPtr xmlParseDoc(const xmlChar *cur)
///
/// # Safety
/// `cur` must be a valid null-terminated UTF-8 byte string
/// with lifetime at least until this function returns.
/// Caller is responsible for freeing the returned xmlDocPtr with xmlFreeDoc().
#[no_mangle]
pub unsafe extern "C" fn xmlParseDoc(cur: *const XmlChar) -> XmlDocPtr {
    if cur.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = CStr::from_ptr(cur as *const c_char).to_bytes();
    match libxml2_rs::parse_bytes(bytes, &ParserOptions::libxml2_compat()) {
        Ok(doc) => Box::into_raw(Box::new(doc)) as XmlDocPtr,
        Err(e) => {
            report_error(e);
            std::ptr::null_mut()
        }
    }
}

/// void xmlFreeDoc(xmlDocPtr cur)
#[no_mangle]
pub unsafe extern "C" fn xmlFreeDoc(cur: XmlDocPtr) {
    if !cur.is_null() {
        drop(Box::from_raw(cur as *mut libxml2_rs::Document));
    }
}

/// xmlNodePtr xmlDocGetRootElement(const xmlDoc *doc)
#[no_mangle]
pub unsafe extern "C" fn xmlDocGetRootElement(doc: *const XmlDoc) -> XmlNodePtr {
    if doc.is_null() { return std::ptr::null_mut(); }
    let doc = &*(doc as *const libxml2_rs::Document);
    match doc.root_element() {
        Some(node_id) => node_id_to_ptr(doc, node_id),
        None => std::ptr::null_mut(),
    }
}
```

### 6.3 cbindgen Configuration

```toml
# crates/libxml2-rs-c/cbindgen.toml
language = "C"
include_guard = "LIBXML2_COMPAT_H"
autogen_warning = "/* Auto-generated by cbindgen — matches libxml2 2.x API */"

[export]
prefix = "xml"
include = []

[fn]
must_use = ""
prefix = ""

[struct]
rename_fields = "None"
```

### 6.4 Symbol Versioning

```
# crates/libxml2-rs-c/libxml2.map
LIBXML2_2.6.0 {
    global:
        xmlParseDoc;
        xmlParseFile;
        xmlParseMemory;
        xmlFreeDoc;
        xmlDocGetRootElement;
        xmlNodeGetContent;
        xmlGetProp;
        xmlSetProp;
        xmlNewDoc;
        xmlNewNode;
        xmlNewChild;
        xmlAddChild;
        xmlUnlinkNode;
        xmlFreeNode;
        xmlSaveDoc;
        xmlDocDumpMemory;
        xmlXPathNewContext;
        xmlXPathFreeContext;
        xmlXPathEval;
        xmlXPathFreeObject;
        xmlSchemaNewParserCtxt;
        xmlSchemaFree;
        xmlSchemaNewValidCtxt;
        xmlSchemaValidateDoc;
        xmlRelaxNGNewParserCtxt;
        xmlRelaxNGFree;
        # ... complete symbol list from libxml2 exports
    local: *;
};
```

---

## 7. Phased Delivery Roadmap

### Phase 1 — Core Parser (Months 1–6)
**Goal:** A production-ready XML 1.0 parser that passes ≥95% of W3C xmlconf tests.

**Deliverables:**
- `xml-chars`: Unicode character class tables for XML 1.0 Name productions
- `xml-tokenizer`: Zero-copy tokenizer, SAX2 event emission, entity expansion (with limits), push parser API
- `xml-tree`: Arena-based mutable DOM (xmlDoc/xmlNode parity), serialization
- `xml-ns`: Namespace 1.0 support
- `libxml2-rs` facade: `parse_str`, `parse_bytes`, `parse_file`, `parse_reader`
- Error system with structured `XmlError`
- URI handling (`libxml/uri.h` equivalent)
- W3C xmlconf test runner (≥95% pass rate target)
- OSS-Fuzz integration (submitted to Google's OSS-Fuzz program)
- GitHub Actions CI: test, clippy, miri, cargo-deny, MSRV check
- Initial API documentation (rustdoc) + mdBook getting-started guide

**Success Criteria:**
- `cargo test` passes all unit and xmlconf tests
- `cargo fuzz` runs 24h without crashes on xmlconf seed corpus
- Throughput benchmark: ≥ libxml2 SAX mode performance on xmlmark benchmark suite

### Phase 2 — Extended Parsing + XPath (Months 7–12)
**Goal:** Feature parity for the most commonly used libxml2 APIs.

**Deliverables:**
- `xmlTextReader` pull parser interface (XmlReader-style)
- `xmlTextWriter` streaming writer interface
- HTML lenient parser (pre-HTML5 mode)
- DTD validation (internal + external subsets)
- XPath 1.0 engine with bytecode compilation
- `xmlPattern` simplified pattern matching
- Custom I/O handler callbacks
- Encoding transcoding via `encoding_rs` (all XML-required encodings)
- External entity support (opt-in, behind `allow_external_entities` flag)
- Custom allocator hooks (`xmlMemSetup` equivalent)
- `xmllint-rs` CLI tool with: `--valid`, `--schema`, `--xpath`, `--format`, `--c14n`
- C ABI layer (Phase 1 symbol set): `xmlParseDoc`, `xmlFreeDoc`, `xmlDocGetRootElement`, tree navigation, `xmlGetProp`, `xmlSetProp`, `xmlSaveDoc`
- Differential testing harness vs libxml2 reference
- XML Namespace conformance test suite

**Success Criteria:**
- XPath passes OASIS XPath 1.0 test suite at ≥98%
- DTD validation tested against OASIS XML test suite
- C API symbols pass libxml2-compat test suite (build existing libxml2 test programs against Rust .so)

### Phase 3 — Schema Validation + Transformations (Months 13–24)
**Goal:** Enterprise-grade validation support. This is the hardest phase.

**Deliverables:**
- **XML Schema (XSD 1.0)** — the single largest engineering effort:
  - Schema parser (XSD is itself an XML document)
  - Type system (built-in types, simple types, complex types)
  - Content model validation (sequences, choices, all, any)
  - Identity constraints (unique, key, keyref)
  - Inheritance (extension, restriction)
  - W3C XSTS conformance test suite runner
- **RelaxNG validation** — XML syntax and compact syntax (using peg/nom parser for compact syntax)
- **XInclude 1.0** processing
- **C14N** (original and exclusive) — needed for XML-DSIG
- **XML Catalogs** (OASIS spec)
- HTML5-compliant parser (`html5ever` integration)
- **XPointer** framework (for XLink consumers)
- C ABI layer (Phase 2+3 symbols): xmlSchemaXxx, xmlRelaxNGXxx, xmlXIncludeXxx, xmlC14NXxx

**Success Criteria:**
- XSD validation passes W3C XSTS at ≥90% (libxml2 scores ~85-90%)
- RelaxNG passes OASIS RelaxNG test suite at ≥95%
- C14N output byte-identical to libxml2 on W3C C14N test vectors
- Real-world smoke tests: parse/validate ODF, OOXML, Maven POM, SOAP, DITA

### Phase 4 — XSLT + Schematron + Maturity (Months 25–36)
**Goal:** Full ecosystem parity. Production-ready for broad adoption.

**Deliverables:**
- **XSLT 1.0** processor (enormous scope — consider as a companion crate, not in core):
  - Template matching, modes, priority
  - All XSLT 1.0 elements and functions
  - Extension element/function framework
  - xsl:message, xsl:fallback
  - Output method: XML, HTML, text
  - OASIS XSLT 1.0 conformance test suite
- **Schematron** (ISO Schematron subset) — implemented as XSLT transformation
- **Full C ABI parity**: all 500+ public functions with complete symbol compatibility
- Performance optimization sprint:
  - Profile-guided optimization (PGO)
  - XPath JIT (compile frequently-evaluated expressions to native code via cranelift)
  - Parallel validation (rayon for schema validation of large document sets)
- Security audit (trail of bits or similar)
- SBOM (Software Bill of Materials) generation
- CVE response process established with project security contacts
- Submission to major Linux distributions (Fedora, Debian, Alpine)

---

## 8. Testing Strategy

### 8.1 Test Pyramid

```
                        ┌─────────────────────────────┐
                        │  E2E / Integration Tests     │  (slowest)
                        │  - Real-world XML corpus     │
                        │  - Differential vs libxml2   │
                        │  - C ABI compatibility       │
                        ├─────────────────────────────┤
                        │  W3C/OASIS Conformance Tests │
                        │  - xmlconf (~2000 tests)     │
                        │  - XSTS (XSD)                │
                        │  - RelaxNG suite             │
                        │  - XPath/XSLT tests          │
                        ├─────────────────────────────┤
                        │  Property-Based Tests        │
                        │  - proptest: round-trip      │
                        │  - proptest: no-panic        │
                        ├─────────────────────────────┤
                        │  Unit Tests                  │  (fastest)
                        │  - Per-crate unit tests      │
                        │  - Doc tests                 │
                        └─────────────────────────────┘
```

### 8.2 W3C Conformance Test Integration

Each conformance suite is checked in as a git submodule and exercised via `cargo test --features conformance`:

```rust
// tests/xmlconf.rs
#[test]
#[cfg(feature = "conformance-tests")]
fn w3c_xml_1_0_conformance() {
    let suite = xmlconf::load_suite("tests/xmlconf/xmlconf.xml");
    let results = suite.run(|test| libxml2_rs::parse_bytes(test.content, &Default::default()));

    let pass_rate = results.pass_rate();
    println!("W3C XML Conformance: {:.1}% ({}/{} passed)",
             pass_rate * 100.0, results.passed, results.total);

    // Publish conformance report
    results.write_html("target/conformance/xmlconf.html");

    assert!(pass_rate >= 0.95, "W3C XML conformance below 95% target");
}
```

### 8.3 Fuzzing Architecture

```
fuzz/fuzz_targets/
├── parse_xml.rs            # Fuzz core XML parser
├── parse_html.rs           # Fuzz HTML parser
├── xpath_eval.rs           # Fuzz XPath (xml + expression)
├── schema_validate.rs      # Fuzz schema + document pair
├── roundtrip.rs            # parse→serialize→parse must be identical
├── c_api_fuzz.rs           # Fuzz C ABI boundary (e.g., random pointer patterns)
└── differential_fuzz.rs    # Both libxml2 and libxml2-rs, compare results
```

**OSS-Fuzz submission** (`oss-fuzz/projects/libxml2-rs/`):
- Continuous fuzzing at Google scale (billions of executions/day)
- Automatic CVE filing for crashes
- Required for security credibility as a system-library replacement

**Seed corpus strategy:**
- All files from W3C xmlconf test suite
- Wikipedia XML dumps (sampled)
- OpenOffice.org / OOXML documents
- SVG files from W3C test suite
- All CVE reproducer files (checked in as regression tests)

### 8.4 Property-Based Testing

```rust
// proptest: XML documents always round-trip losslessly
proptest! {
    #[test]
    fn roundtrip_serialization(doc in arb_well_formed_xml()) {
        let serialized = doc.to_xml_string();
        let reparsed = libxml2_rs::parse_str(&serialized)
            .expect("serialized output must be parseable");
        prop_assert!(trees_are_equivalent(&doc, &reparsed));
    }

    #[test]
    fn parser_never_panics(data in proptest::collection::vec(any::<u8>(), 0..65536)) {
        // Must not panic on ANY byte sequence
        let _ = libxml2_rs::parse_bytes(&data, &Default::default());
    }

    #[test]
    fn xpath_never_panics(xml in ".*", xpath_expr in ".*") {
        if let Ok(doc) = libxml2_rs::parse_str(&xml) {
            let ctx = libxml2_rs::xpath::Context::new(&doc);
            let _ = ctx.evaluate(&xpath_expr);
        }
    }
}
```

### 8.5 Differential Testing vs libxml2

```rust
// tests/differential/mod.rs
// Requires `libxml2` system library installed
#[test]
#[cfg(feature = "differential-testing")]
fn compare_parse_output_with_libxml2() {
    for path in glob("tests/corpus/**/*.xml").unwrap().flatten() {
        let bytes = fs::read(&path).unwrap();

        let reference = libxml2_reference::parse(&bytes);  // via libxml2-sys
        let candidate = libxml2_rs::parse_bytes(&bytes, &ParserOptions::libxml2_compat());

        match (reference, candidate) {
            (Ok(ref_doc), Ok(cand_doc)) => {
                let ref_xml  = libxml2_reference::serialize_canonical(&ref_doc);
                let cand_xml = libxml2_rs::c14n::serialize(&cand_doc);
                assert_eq!(ref_xml, cand_xml, "Output mismatch: {:?}", path);
            }
            (Err(_), Err(_)) => {}   // Both reject: acceptable
            (Ok(_), Err(e)) => panic!("libxml2-rs rejected: {:?} — {}", path, e),
            (Err(_), Ok(_)) => {}    // libxml2-rs is stricter: acceptable, log it
        }
    }
}
```

### 8.6 Performance Regression Testing

```rust
// benches/parse_throughput.rs — criterion
criterion_group! {
    name = parse_benches;
    config = Criterion::default()
        .sample_size(100)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(3));
    targets = bench_small, bench_medium, bench_large, bench_xpath
}
```

CI enforces: any PR that causes >5% throughput regression on the medium benchmark is blocked pending review.

### 8.7 Security Regression Tests

Every libxml2 CVE that is in scope becomes a regression test:

```
tests/regression/
├── CVE-2022-40304-use-after-free.xml
├── CVE-2022-40303-integer-overflow.xml
├── CVE-2021-3541-billion-laughs.xml
├── CVE-2019-19956-memory-leak.xml
└── ...
```

```rust
#[test]
fn cve_2021_3541_billion_laughs() {
    let xml = include_bytes!("regression/CVE-2021-3541-billion-laughs.xml");
    let result = libxml2_rs::parse_bytes(xml, &Default::default());
    // Must either reject (Err) or complete within 100ms (not DoS)
    match result {
        Err(_) => {}   // Rejected: perfect
        Ok(_) => {
            // If accepted, the timer guarantees no DoS
            // (test itself has a 100ms timeout via test harness)
        }
    }
}
```

---

## 9. Documentation Strategy

### 9.1 Documentation Layers

| Layer | Tool | Audience | Location |
|---|---|---|---|
| API reference | rustdoc | Rust developers | docs.rs/libxml2-rs |
| User guide | mdBook | All developers | ibm.github.io/libxml2-rs |
| C migration guide | mdBook | C/C++ developers | ibm.github.io/libxml2-rs/migration |
| C API reference | cbindgen + Doxygen | C developers | ibm.github.io/libxml2-rs/c-api |
| Security model | mdBook | Security teams | ibm.github.io/libxml2-rs/security |
| Architecture | This document | Architects | ARCHITECTURE_PLAN.md |
| Conformance reports | HTML (generated) | Standards teams | ibm.github.io/libxml2-rs/conformance |
| Changelog | Keep a Changelog format | All | CHANGELOG.md |

### 9.2 API Documentation Standards

Every public item in `libxml2-rs` must have:
1. A one-line summary (used in module index)
2. Extended description with examples
3. `# Errors` section (for `Result`-returning functions)
4. `# Panics` section (if the function can panic — minimize panics)
5. `# Safety` section (for `unsafe` functions)
6. `# Performance` note (for functions with non-obvious complexity)
7. Cross-reference to equivalent libxml2 C function (`# libxml2 equivalent: xmlParseDoc`)

### 9.3 C API Migration Guide

A critical document: a side-by-side comparison of every libxml2 C API call and its libxml2-rs Rust equivalent:

```markdown
## Parsing a Document

### C (libxml2)
```c
#include <libxml/parser.h>
#include <libxml/tree.h>

xmlDocPtr doc = xmlParseFile("input.xml");
if (doc == NULL) { /* error */ }
xmlNodePtr root = xmlDocGetRootElement(doc);
printf("Root element: %s\n", root->name);
xmlFreeDoc(doc);
xmlCleanupParser();
```

### Rust (libxml2-rs)
```rust
use libxml2_rs::Document;

let doc = Document::parse_file("input.xml")?;
let root = doc.root_element().ok_or("no root")?;
println!("Root element: {}", root.name());
// doc drops automatically — no manual free
```
```

### 9.4 mdBook Structure

```
docs/guide/src/
├── SUMMARY.md
├── introduction.md
│   └── why-rust.md
├── quickstart.md
├── parsing/
│   ├── from-string.md
│   ├── from-file.md
│   ├── streaming-sax.md
│   ├── push-parser.md
│   └── html.md
├── tree/
│   ├── navigation.md
│   ├── mutation.md
│   ├── serialization.md
│   └── namespaces.md
├── xpath/
│   ├── basics.md
│   ├── functions.md
│   └── extension-functions.md
├── validation/
│   ├── dtd.md
│   ├── xml-schema.md
│   ├── relaxng.md
│   └── schematron.md
├── security/
│   ├── default-safe.md
│   ├── xxe.md
│   └── dos-protection.md
├── migration/
│   ├── from-c.md
│   ├── api-mapping.md           # Exhaustive function-by-function table
│   └── behavioral-differences.md
├── c-api/
│   ├── overview.md
│   ├── linking.md
│   └── drop-in-replacement.md
└── internals/
    ├── architecture.md
    ├── arena-trees.md
    └── contributing.md
```

---

## 10. Open Source Governance

### 10.1 Repository Setup

**GitHub Organization:** `github.com/<org>/libxml2-rs`

**Required files:**
```
LICENSE                  # Apache License 2.0
DCO                      # Developer Certificate of Origin 1.1
CONTRIBUTING.md          # Contribution guidelines, DCO sign-off process
CODE_OF_CONDUCT.md       # Contributor Covenant 2.1
SECURITY.md              # Security contact, responsible disclosure process
MAINTAINERS.md           # Named maintainers with @username and area of responsibility
GOVERNANCE.md            # Decision-making process, steering committee
.github/
├── CODEOWNERS           # Per-directory review requirements
├── ISSUE_TEMPLATE/
│   ├── bug_report.yml
│   ├── feature_request.yml
│   └── security_vulnerability.yml  # Points to SECURITY.md
├── pull_request_template.md
└── workflows/
    ├── ci.yml           # Tests, clippy, miri
    ├── conformance.yml  # W3C/OASIS conformance suites (scheduled nightly)
    ├── fuzz.yml         # cargo-fuzz (scheduled)
    ├── release.yml      # crates.io publish on tag
    └── security.yml     # cargo-deny, cargo-audit
```

### 10.2 Licensing

- **Primary license:** Apache License 2.0
  - Rationale: Compatible with libxml2's MIT license; GPLv2/v3 compatible; standard in CNCF/OpenSSF ecosystem; widely accepted by enterprise consumers
- **No CLA required:** Use DCO (Developer Certificate of Origin)
  - Every commit must have `Signed-off-by: Name <email>` via `git commit -s`
  - DCO bot enforces this on all PRs
  - Rationale: Lower contributor friction than CLA; standard in Linux Foundation projects

### 10.3 Governance Model

**Initial phase (v0.x):** Benevolent dictator — founding maintainers hold final authority.

**Production phase (v1.0+):** Technical Steering Committee (TSC):
- 5 members: 2 founding org, 2 community, 1 at-large (elected annually)
- Decisions by lazy consensus; major decisions (license change, API breakage) require TSC vote
- Monthly public TSC meeting notes published in `governance/meetings/`

### 10.4 CI/CD Requirements

```yaml
# .github/workflows/ci.yml (abbreviated)
on: [push, pull_request]
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        rust: [stable, beta, "1.81"]  # stable + MSRV
    steps:
      - cargo test --workspace --all-features
      - cargo clippy --workspace --all-features -- -D warnings
      - cargo fmt --check

  miri:
    steps:
      - cargo +nightly miri test -p xml-tree -p xml-tokenizer

  deny:
    steps:
      - cargo deny check  # License compliance, vulnerability DB

  audit:
    steps:
      - cargo audit  # RustSec advisory database

  msrv:
    steps:
      - cargo check --rust-version  # Verify MSRV compiles
```

### 10.5 Release Process

- **Versioning:** SemVer 2.0 for the Rust API; separate versioning scheme for C ABI compatibility (`libxml2-rs-c` tracks libxml2 major.minor it's compatible with)
- **Release cadence:** Minor releases every 3 months; patch releases as needed for security
- **Security releases:** Within 7 days of confirmed CVE; coordinated with project security contacts
- **crates.io publish:** Automated from GitHub tag via `release.yml` workflow

### 10.6 Foundation Considerations

Submit the project to the **OpenSSF** (Open Source Security Foundation) under its **Memory Safety** initiative. This provides:
- Visibility to the security community
- Access to OpenSSF's best practices badge program
- Potential OSS-Fuzz integration support
- Collaboration with Google (Chrome team), Mozilla, etc. who are also motivated to replace libxml2

---

## 11. Risk Register

| Risk | Probability | Impact | Mitigation |
|---|---|---|---|
| XSD 1.0 is extremely complex; full implementation may take 18+ months | High | High | Scope to 90% conformance; known gaps documented; ship partial impl with clear version markers |
| XSLT 1.0 is a standalone domain (effectively a programming language); may be indefinitely delayed | High | Medium | Phase 4; consider WASM-bridged libxslt as interim; expose XSLT via `libxslt` crate |
| C ABI binary compatibility is hard to test exhaustively | Medium | High | Automated C ABI testing; run libxml2's own test suite against Rust .so; differential fuzzing |
| Performance regression vs libxml2 in DOM mode | Medium | Medium | Benchmark-driven development; criterion regressions block PRs; PGO in release builds |
| Rust's learning curve slows contributor onboarding | Medium | Medium | Invest heavily in architecture documentation, mentorship, and good-first-issue labeling |
| A major libxml2 CVE drops during our development, making the partial Rust impl look bad | Low | High | Quick patch track: any CVE in libxml2's parsing core is highest priority; OSS-Fuzz catches novel issues |
| Entity expansion / XInclude behavior differences break real applications | Medium | High | `libxml2_compat_options()` mode; differential testing on real-world XML corpora |
| Thread safety bugs in the C ABI layer | Low | High | `cargo test` under TSAN; Miri for unsafe code; keep C ABI layer thin |
| MSRV creep breaks embedded/distro consumers | Low | Medium | Formal MSRV policy; CI tests MSRV; use `cargo-msrv` to detect accidental breakage |

---

## 12. Appendix

### 12.1 Key External Resources

| Resource | Location | Purpose |
|---|---|---|
| W3C XML Conformance Test Suite | `https://www.w3.org/XML/Test/` | xmlconf ~2000 tests |
| W3C XML Schema Test Suite (XSTS) | `https://www.w3.org/XML/2004/xml-schema-test-suite/` | XSD validation tests |
| OASIS RelaxNG Test Suite | `https://relaxng.org/` | RelaxNG conformance |
| W3C XSLT/XPath Test Suite | `https://www.w3.org/XML/Group/xsl-spec.html` | XSLT 1.0 / XPath 1.0 |
| html5lib-tests | `https://github.com/html5lib/html5lib-tests` | HTML5 parsing tests |
| W3C C14N Test Vectors | `https://www.w3.org/TR/xml-c14n/` | C14N correctness |
| libxml2 source (reference) | `https://gitlab.gnome.org/GNOME/libxml2` | Behavior reference |
| libxml2-api.xml | In libxml2 source: `doc/libxml2-api.xml` | Complete API symbol list |
| OSS-Fuzz | `https://github.com/google/oss-fuzz` | Continuous fuzzing |
| OpenSSF Best Practices | `https://bestpractices.coreinfrastructure.org/` | Security badge |

### 12.2 Key Rust Crate Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `memchr` | ~2.7 | SIMD byte scanning in tokenizer |
| `encoding_rs` | ~0.8 | Character encoding transcoding |
| `bumpalo` | ~3.16 | Arena allocator for string/node storage |
| `indexmap` | ~2 | Ordered hash map for name interning |
| `smallvec` | ~1 | Small-vector optimization for attributes |
| `thiserror` | ~1 | Error type derivation |
| `log` | ~0.4 | Structured logging (no hard eprintln) |
| `html5ever` | ~0.28 | HTML5 parsing (Phase 2+) |
| `criterion` | ~0.5 | Benchmarking |
| `proptest` | ~1 | Property-based testing |
| `cargo-fuzz` / `libfuzzer-sys` | ~0.4 | Fuzzing infrastructure |
| `cbindgen` | ~0.26 | C header generation (build dep) |
| `cargo-deny` | ~0.14 | License + vulnerability scanning (dev) |

### 12.3 Major libxml2 Consumers (Adoption Targets)

Understanding who uses libxml2 drives prioritization — these are the ecosystems most likely to adopt or benefit from libxml2-rs:

| Consumer | How They Use libxml2 | Priority Signal |
|---|---|---|
| **PHP** (`ext/libxml`, `DOMDocument`, `XMLReader`, `XMLWriter`, `XSL`) | Hundreds of millions of web applications | 🔴 Highest — PHP ships libxml2 |
| **Python / lxml** | `pip install lxml` has billions of downloads | 🔴 Highest |
| **Ruby / Nokogiri** | Dominant Ruby XML/HTML library | 🔴 High |
| **PostgreSQL** | `XPATH()`, `XMLTABLE()`, `xml` data type | 🔴 High — DB query path |
| **LibreOffice** | ODF (`.odt`, `.ods`, `.odp`) parsing | 🟡 High |
| **xmlsec1** | XML Signature, XML Encryption (SAML, eIDAS) | 🔴 High — security-critical |
| **Android NDK** | System library on all Android devices | 🟡 High |
| **Inkscape / resvg** | SVG parsing | 🟡 Medium |
| **DocBook/DITA toolchains** | XSLT-based documentation | 🟡 Medium (XSLT needed) |
| **XML-RPC / SOAP / REST-XML** | Web service clients/servers | 🟡 Medium |

**Key insight for roadmap prioritization:** PHP, Python/lxml, Ruby/Nokogiri, and PostgreSQL share a common pattern — they need DOM parsing + XPath + XSD validation. Phases 1–3 of the roadmap directly serve all four. XSLT (Phase 4) is most critical for DocBook toolchains.

### 12.4 Estimated Team and Timeline

| Phase | Duration | Core Team Size | Key Specializations Needed |
|---|---|---|---|
| Phase 1 | 6 months | 3–4 engineers | Rust, XML spec expertise, parser engineering |
| Phase 2 | 6 months | 4–5 engineers | + XPath, C FFI, DTD internals |
| Phase 3 | 12 months | 5–6 engineers | + XSD (XML Schema expert critical), RelaxNG |
| Phase 4 | 12 months | 4–5 engineers | + XSLT (XSLT implementor), security auditor |

**Total estimated effort:** ~36 months for full parity; ~18 months for the most impactful 80% (Phases 1–2 + XSD validation from Phase 3).

---

*Document prepared: 2026-03-19*
*Next review: 2026-06-19*
*See MAINTAINERS.md for project contacts.*
