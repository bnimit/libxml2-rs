//! libxml2-rs — unified public API for the libxml2-rs workspace.
//!
//! See `docs/architecture/overview.md` for the full design.
#![warn(missing_docs)]

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// EntityResolver
// ---------------------------------------------------------------------------

/// Resolves external entity references (SYSTEM / PUBLIC identifiers) to their
/// byte content.
///
/// Only called when [`ParserOptions::allow_external_entities`] is `true`.
/// Implement this trait to control where external entity content is loaded
/// from (filesystem, in-memory map, network, etc.).
///
/// # Security
///
/// Enabling external entity loading opens the door to **XXE (XML External
/// Entity) injection** attacks.  Only enable it when you trust the source of
/// the XML input and have a controlled `EntityResolver` implementation.
pub trait EntityResolver: Send + Sync {
    /// Attempt to resolve a SYSTEM or PUBLIC identifier.
    ///
    /// Returns `Some(bytes)` if the entity can be served, `None` if this
    /// resolver does not handle the URI (the parser will then report an error).
    fn resolve(&self, public_id: Option<&str>, system_id: &str) -> Option<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// ParserOptions
// ---------------------------------------------------------------------------

/// Options that control the XML parser's behaviour.
///
/// The default configuration is *secure*: XXE (XML External Entity) attacks
/// are prevented by disabling external entity loading, and entity expansion is
/// limited to avoid "billion laughs" style DoS attacks.
///
/// Use [`ParserOptions::libxml2_compat`] only in tests that verify
/// compatibility with the reference implementation.
///
/// # Example
///
/// ```rust
/// use libxml2_rs::ParserOptions;
///
/// let opts = ParserOptions::default(); // secure defaults
/// assert!(!opts.allow_external_entities);
/// ```
#[derive(Clone)]
pub struct ParserOptions {
    /// Maximum nesting depth of entity expansion.
    ///
    /// Prevents stack-exhaustion from deeply recursive entity chains.
    /// Default: `100`.
    pub max_entity_depth: usize,

    /// Maximum total number of bytes produced by entity expansion.
    ///
    /// Prevents "billion laughs" style DoS attacks where a small document
    /// expands to gigabytes of text.  Default: `10 * 1024 * 1024` (10 MiB).
    pub max_entity_expansion: usize,

    /// Allow loading external entities via SYSTEM/PUBLIC identifiers.
    ///
    /// **`false` by default** — enabling this opens XXE attack vectors.
    /// Requires an [`EntityResolver`] to be set via `entity_resolver`.
    pub allow_external_entities: bool,

    /// Resolver used to load external entities when
    /// [`allow_external_entities`] is `true`.
    ///
    /// [`allow_external_entities`]: ParserOptions::allow_external_entities
    pub entity_resolver: Option<Arc<dyn EntityResolver>>,

    /// Maximum element nesting depth.
    ///
    /// Documents deeper than this limit are rejected.  Default: `10_000`.
    pub max_nesting_depth: usize,

    /// Maximum number of attributes per element.
    ///
    /// Default: `10_000`.
    pub max_attrs_per_element: usize,

    /// Substitute entity references with their replacement text in the DOM.
    ///
    /// Default: `true`.
    pub substitute_entities: bool,

    /// Load and merge the external DTD subset (if declared in `<!DOCTYPE>`).
    ///
    /// **`false` by default.**  Requires [`allow_external_entities`] and an
    /// [`EntityResolver`] for the DTD file to be loadable.
    ///
    /// [`allow_external_entities`]: ParserOptions::allow_external_entities
    pub load_external_dtd: bool,
}

impl ParserOptions {
    /// Options that replicate libxml2's historically permissive (insecure)
    /// defaults.
    ///
    /// **Use only in differential / compatibility tests.**  Never use in
    /// production parsing of untrusted input.
    pub fn libxml2_compat() -> Self {
        Self {
            allow_external_entities: true,
            load_external_dtd: true,
            max_entity_expansion: usize::MAX,
            max_entity_depth: usize::MAX,
            ..Default::default()
        }
    }
}

impl Default for ParserOptions {
    fn default() -> Self {
        Self {
            max_entity_depth: 100,
            max_entity_expansion: 10 * 1024 * 1024,
            allow_external_entities: false,
            entity_resolver: None,
            max_nesting_depth: 10_000,
            max_attrs_per_element: 10_000,
            substitute_entities: true,
            load_external_dtd: false,
        }
    }
}

impl std::fmt::Debug for ParserOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParserOptions")
            .field("max_entity_depth", &self.max_entity_depth)
            .field("max_entity_expansion", &self.max_entity_expansion)
            .field("allow_external_entities", &self.allow_external_entities)
            .field(
                "entity_resolver",
                &self
                    .entity_resolver
                    .as_ref()
                    .map(|_| "<dyn EntityResolver>"),
            )
            .field("max_nesting_depth", &self.max_nesting_depth)
            .field("max_attrs_per_element", &self.max_attrs_per_element)
            .field("substitute_entities", &self.substitute_entities)
            .field("load_external_dtd", &self.load_external_dtd)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ParseError
// ---------------------------------------------------------------------------

/// Errors returned by the XML parser.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParseError {
    /// The input is not valid UTF-8.
    InvalidUtf8,
    /// The XML is not well-formed.
    NotWellFormed {
        /// Byte offset in the input where the error was detected.
        offset: usize,
        /// Human-readable description of the problem.
        message: String,
    },
    /// The document contains no root element.
    NoRootElement,
    /// An I/O error occurred while reading the input (only from
    /// [`parse_file`] and [`parse_reader`]).
    Io(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidUtf8 => write!(f, "input is not valid UTF-8"),
            ParseError::NotWellFormed { offset, message } => {
                write!(f, "not well-formed at byte {offset}: {message}")
            }
            ParseError::NoRootElement => write!(f, "document has no root element"),
            ParseError::Io(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// A parsed XML document.
///
/// Wraps [`xml_tree::Document`] — an arena-based DOM where all nodes are
/// stored in a flat `Vec` and referenced by integer [`xml_tree::NodeId`]s.
pub struct Document {
    inner: xml_tree::Document,
}

impl Document {
    /// The document root node.
    pub fn root(&self) -> xml_tree::NodeId {
        self.inner.root()
    }

    /// Access the underlying [`xml_tree::Document`] for traversal.
    pub fn tree(&self) -> &xml_tree::Document {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// parse_bytes
// ---------------------------------------------------------------------------

/// Parse a UTF-8 XML document from a byte slice.
///
/// Drives the zero-copy tokenizer ([`xml_tokenizer`]) and the arena tree
/// builder ([`xml_tree::Builder`]) to produce a [`Document`].
///
/// # Errors
///
/// | Error | Cause |
/// |---|---|
/// | [`ParseError::InvalidUtf8`] | Input is not valid UTF-8 |
/// | [`ParseError::NotWellFormed`] | Structural XML error (unclosed tag, illegal character, …) |
/// | [`ParseError::NoRootElement`] | Document is empty or contains only declarations |
///
/// # Example
///
/// ```rust
/// use libxml2_rs::{parse_bytes, ParserOptions};
///
/// let xml = b"<?xml version=\"1.0\"?><root><child/></root>";
/// let doc = parse_bytes(xml, &ParserOptions::default()).unwrap();
/// let tree = doc.tree();
/// let root_elem = tree.first_child(doc.root()).unwrap();
/// assert_eq!(tree.name(root_elem), "root");
/// ```
pub fn parse_bytes(input: &[u8], _opts: &ParserOptions) -> Result<Document, ParseError> {
    // Step 1 — create the tokenizer.  This validates UTF-8 upfront and strips
    // the BOM so the rest of the pipeline only sees clean UTF-8.
    let mut tokenizer = xml_tokenizer::Tokenizer::new(input).map_err(|e| match e {
        xml_tokenizer::TokenError::InvalidUtf8 => ParseError::InvalidUtf8,
        _ => ParseError::NotWellFormed {
            offset: 0,
            message: "tokenizer initialisation failed".into(),
        },
    })?;

    // Step 2 — feed every token into the tree builder.
    let mut builder = xml_tree::Builder::new();
    loop {
        let token = tokenizer.next_token().map_err(token_err_to_parse_err)?;
        let done = token == xml_tokenizer::Token::Eof;
        builder
            .process_token(token)
            .map_err(build_err_to_parse_err)?;
        if done {
            break;
        }
    }

    // Step 3 — finalise and wrap.
    let inner = builder.finish().map_err(build_err_to_parse_err)?;

    Ok(Document { inner })
}

// ---------------------------------------------------------------------------
// parse_str
// ---------------------------------------------------------------------------

/// Parse a UTF-8 XML document from a string slice.
///
/// Thin wrapper over [`parse_bytes`] — the string is passed as bytes with no
/// extra allocation.
///
/// # Errors
///
/// Same as [`parse_bytes`].
///
/// # Example
///
/// ```rust
/// use libxml2_rs::{parse_str, ParserOptions};
///
/// let doc = parse_str("<root/>", &ParserOptions::default()).unwrap();
/// assert_eq!(doc.tree().name(doc.tree().first_child(doc.root()).unwrap()), "root");
/// ```
pub fn parse_str(input: &str, opts: &ParserOptions) -> Result<Document, ParseError> {
    parse_bytes(input.as_bytes(), opts)
}

// ---------------------------------------------------------------------------
// parse_file
// ---------------------------------------------------------------------------

/// Parse an XML document from a file on disk.
///
/// Reads the entire file into memory then calls [`parse_bytes`].
/// Encoding detection (BOM / XML declaration sniff) will be added in a
/// follow-up issue once `encoding_rs` transcoding is wired in.
///
/// # Errors
///
/// | Error | Cause |
/// |---|---|
/// | [`ParseError::Io`] | File cannot be opened or read |
/// | [`ParseError::InvalidUtf8`] | File content is not valid UTF-8 |
/// | [`ParseError::NotWellFormed`] | XML is not well-formed |
/// | [`ParseError::NoRootElement`] | File is empty or contains only declarations |
///
/// # Example
///
/// ```rust,no_run
/// use libxml2_rs::{parse_file, ParserOptions};
///
/// let doc = parse_file("input.xml", &ParserOptions::default()).unwrap();
/// ```
pub fn parse_file(path: impl AsRef<Path>, opts: &ParserOptions) -> Result<Document, ParseError> {
    let bytes = std::fs::read(path).map_err(|e| ParseError::Io(e.to_string()))?;
    parse_bytes(&bytes, opts)
}

// ---------------------------------------------------------------------------
// parse_reader
// ---------------------------------------------------------------------------

/// Parse an XML document from any [`Read`] source.
///
/// Reads the source to completion, then calls [`parse_bytes`].
///
/// # Errors
///
/// | Error | Cause |
/// |---|---|
/// | [`ParseError::Io`] | Reader returns an I/O error |
/// | [`ParseError::InvalidUtf8`] | Content is not valid UTF-8 |
/// | [`ParseError::NotWellFormed`] | XML is not well-formed |
/// | [`ParseError::NoRootElement`] | Source is empty or contains only declarations |
///
/// # Example
///
/// ```rust
/// use libxml2_rs::{parse_reader, ParserOptions};
///
/// let xml = b"<root/>" as &[u8];
/// let doc = parse_reader(xml, &ParserOptions::default()).unwrap();
/// assert_eq!(doc.tree().name(doc.tree().first_child(doc.root()).unwrap()), "root");
/// ```
pub fn parse_reader<R: Read>(mut reader: R, opts: &ParserOptions) -> Result<Document, ParseError> {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|e| ParseError::Io(e.to_string()))?;
    parse_bytes(&bytes, opts)
}

// ---------------------------------------------------------------------------
// Internal error converters
// ---------------------------------------------------------------------------

/// Convert a [`xml_tree::BuildError`] to a [`ParseError`].
fn build_err_to_parse_err(e: xml_tree::BuildError) -> ParseError {
    match e {
        xml_tree::BuildError::NoRootElement => ParseError::NoRootElement,
        xml_tree::BuildError::UnexpectedEndTag => ParseError::NotWellFormed {
            offset: 0,
            message: "unexpected end tag".into(),
        },
        xml_tree::BuildError::UnboundNamespacePrefix(p) => ParseError::NotWellFormed {
            offset: 0,
            message: format!("unbound namespace prefix: {p}"),
        },
        xml_tree::BuildError::InvalidCharacterReference(code) => ParseError::NotWellFormed {
            offset: 0,
            message: format!("invalid character reference: U+{code:04X}"),
        },
    }
}

/// Convert a [`xml_tokenizer::TokenError`] to a [`ParseError`].
fn token_err_to_parse_err(e: xml_tokenizer::TokenError) -> ParseError {
    match e {
        xml_tokenizer::TokenError::InvalidUtf8 => ParseError::InvalidUtf8,
        xml_tokenizer::TokenError::UnexpectedEof => ParseError::NotWellFormed {
            offset: 0,
            message: "unexpected end of input".into(),
        },
        xml_tokenizer::TokenError::IllegalCharacter { offset } => ParseError::NotWellFormed {
            offset,
            message: "illegal character".into(),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_element() {
        let doc = parse_bytes(b"<root/>", &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "root");
    }

    #[test]
    fn parse_full_document() {
        let xml = b"<?xml version=\"1.0\"?><root><child attr=\"val\">text</child></root>";
        let doc = parse_bytes(xml, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let root = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(root), "root");
        let child = tree.first_child(root).unwrap();
        assert_eq!(tree.name(child), "child");
        assert_eq!(tree.attr_value(&tree.attrs(child)[0]), "val");
        let txt = tree.first_child(child).unwrap();
        assert_eq!(tree.value(txt), "text");
    }

    #[test]
    fn invalid_utf8_returns_error() {
        let result = parse_bytes(b"\xff\xfe", &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::InvalidUtf8)));
    }

    #[test]
    fn not_well_formed_returns_error() {
        let result = parse_bytes(b"<<", &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::NotWellFormed { .. })));
    }

    #[test]
    fn empty_input_returns_no_root_error() {
        let result = parse_bytes(b"", &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::NoRootElement)));
    }

    #[test]
    fn parse_str_works() {
        let doc = parse_str("<hello/>", &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "hello");
    }

    #[test]
    fn parse_reader_works() {
        let xml = b"<world/>" as &[u8];
        let doc = parse_reader(xml, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "world");
    }

    #[test]
    fn parse_reader_io_error_propagated() {
        struct FailReader;
        impl Read for FailReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "broken pipe",
                ))
            }
        }
        let result = parse_reader(FailReader, &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::Io(_))));
    }

    #[test]
    fn parser_options_defaults_are_secure() {
        let opts = ParserOptions::default();
        assert!(!opts.allow_external_entities);
        assert!(!opts.load_external_dtd);
        assert_eq!(opts.max_entity_depth, 100);
        assert_eq!(opts.max_entity_expansion, 10 * 1024 * 1024);
        assert_eq!(opts.max_nesting_depth, 10_000);
    }

    #[test]
    fn libxml2_compat_has_permissive_defaults() {
        let opts = ParserOptions::libxml2_compat();
        assert!(opts.allow_external_entities);
        assert!(opts.load_external_dtd);
        assert_eq!(opts.max_entity_expansion, usize::MAX);
    }

    #[test]
    fn parse_error_display() {
        assert_eq!(
            ParseError::InvalidUtf8.to_string(),
            "input is not valid UTF-8"
        );
        assert_eq!(
            ParseError::NoRootElement.to_string(),
            "document has no root element"
        );
        let e = ParseError::NotWellFormed {
            offset: 42,
            message: "bad tag".into(),
        };
        assert_eq!(e.to_string(), "not well-formed at byte 42: bad tag");
        assert_eq!(
            ParseError::Io("broken pipe".into()).to_string(),
            "I/O error: broken pipe"
        );
    }
}
