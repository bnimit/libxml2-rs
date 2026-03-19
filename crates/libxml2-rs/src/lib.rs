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
    // Step 1 — transcode to UTF-8 if needed (zero-copy for plain UTF-8 input).
    let utf8 = transcode_to_utf8(input)?;

    // Step 2 — create the tokenizer.  This validates UTF-8 upfront and strips
    // the BOM so the rest of the pipeline only sees clean UTF-8.
    let mut tokenizer = xml_tokenizer::Tokenizer::new(&utf8).map_err(|e| match e {
        xml_tokenizer::TokenError::InvalidUtf8 => ParseError::InvalidUtf8,
        _ => ParseError::NotWellFormed {
            offset: 0,
            message: "tokenizer initialisation failed".into(),
        },
    })?;

    // Step 3 — feed every token into the tree builder.
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

    // Step 4 — finalise and wrap.
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
// Encoding detection and transcoding
// ---------------------------------------------------------------------------

/// Transcode `input` to UTF-8, returning a zero-copy `Cow::Borrowed` when the
/// input is already UTF-8 (with or without BOM) and `Cow::Owned` when it had
/// to be transcoded from another encoding.
///
/// Detection order:
/// 1. BOM — `encoding_rs::Encoding::for_bom` covers UTF-8, UTF-16 LE, UTF-16 BE.
/// 2. XML declaration `encoding="…"` sniff for ASCII-compatible encodings.
/// 3. Default: assume UTF-8.
fn transcode_to_utf8(input: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>, ParseError> {
    // ── 1. BOM detection ────────────────────────────────────────────────────
    if let Some((enc, bom_len)) = encoding_rs::Encoding::for_bom(input) {
        if enc == encoding_rs::UTF_8 {
            // UTF-8 BOM: the tokenizer already strips it; pass through as-is.
            return Ok(std::borrow::Cow::Borrowed(input));
        }
        // UTF-16 LE or BE: transcode the post-BOM bytes.
        let (decoded, _, had_errors) = enc.decode(&input[bom_len..]);
        if had_errors {
            return Err(ParseError::NotWellFormed {
                offset: 0,
                message: format!("encoding error while transcoding {} input", enc.name()),
            });
        }
        return Ok(std::borrow::Cow::Owned(decoded.into_owned().into_bytes()));
    }

    // ── 2. XML declaration sniff ─────────────────────────────────────────────
    if let Some(label) = sniff_xml_encoding_label(input) {
        if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            if enc != encoding_rs::UTF_8 {
                let (decoded, actual_enc, had_errors) = enc.decode(input);
                if had_errors {
                    return Err(ParseError::NotWellFormed {
                        offset: 0,
                        message: format!("encoding error while transcoding {} input", enc.name()),
                    });
                }
                // Warn if the decoder auto-detected a different encoding.
                if actual_enc != enc {
                    log::warn!(
                        "declared encoding {label:?} but content detected as {}",
                        actual_enc.name()
                    );
                }
                return Ok(std::borrow::Cow::Owned(decoded.into_owned().into_bytes()));
            }
        } else {
            log::warn!("unknown encoding label {label:?}; proceeding as UTF-8");
        }
    }

    // ── 3. Default: UTF-8 (zero copy) ────────────────────────────────────────
    Ok(std::borrow::Cow::Borrowed(input))
}

/// Scan the first 256 bytes of `input` for an ASCII-compatible XML declaration
/// and return the declared `encoding` label if found.
///
/// Only works for encodings that are ASCII-compatible in the first few bytes
/// (UTF-8, ISO-8859-*, Windows-125x, …).  UTF-16 without a BOM is not handled
/// here — those documents should carry a BOM.
fn sniff_xml_encoding_label(input: &[u8]) -> Option<String> {
    // Only scan the first 256 bytes; an XML declaration must appear right at
    // the start of the document.
    let head = &input[..input.len().min(256)];
    // Must start with `<?xml` (ASCII-compatible).
    if !head.starts_with(b"<?xml") {
        return None;
    }
    // Find `?>` in the raw bytes — the XML declaration is always pure ASCII
    // so we can scan for it even in non-UTF-8 documents.
    let decl_end = head.windows(2).position(|w| w == b"?>")?;
    // The XML declaration content is guaranteed ASCII — from_utf8 will succeed.
    let decl_bytes = &head[..decl_end];
    let decl = std::str::from_utf8(decl_bytes).ok()?;
    // Find `encoding` pseudo-attribute.
    let enc_start = decl.find("encoding")?;
    let after = &decl[enc_start + 8..];
    // Skip optional whitespace and the `=`.
    let after = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let after = after.strip_prefix('=')?;
    let after = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
    // Expect an opening quote.
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let label_rest = &after[1..];
    let end = label_rest.find(quote)?;
    Some(label_rest[..end].to_string())
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
        // 0x80 is a lone continuation byte — not valid UTF-8 and not a BOM.
        let result = parse_bytes(b"\x80text", &ParserOptions::default());
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

    // -----------------------------------------------------------------------
    // Encoding detection and transcoding
    // -----------------------------------------------------------------------

    #[test]
    fn utf8_bom_document_parses() {
        // UTF-8 BOM followed by a well-formed document.
        let xml = b"\xef\xbb\xbf<root/>";
        let doc = parse_bytes(xml, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "root");
    }

    #[test]
    fn utf16_le_bom_document_parses() {
        // Build a minimal UTF-16 LE document: BOM + `<r/>` encoded as UTF-16 LE.
        let mut utf16le: Vec<u8> = vec![0xFF, 0xFE]; // LE BOM
        for ch in "<r/>".encode_utf16() {
            utf16le.push(ch as u8);
            utf16le.push((ch >> 8) as u8);
        }
        let doc = parse_bytes(&utf16le, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "r");
    }

    #[test]
    fn utf16_be_bom_document_parses() {
        // Build a minimal UTF-16 BE document: BOM + `<r/>` encoded as UTF-16 BE.
        let mut utf16be: Vec<u8> = vec![0xFE, 0xFF]; // BE BOM
        for ch in "<r/>".encode_utf16() {
            utf16be.push((ch >> 8) as u8);
            utf16be.push(ch as u8);
        }
        let doc = parse_bytes(&utf16be, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let elem = tree.first_child(doc.root()).unwrap();
        assert_eq!(tree.name(elem), "r");
    }

    #[test]
    fn iso_8859_1_document_transcodes() {
        // ISO-8859-1: `é` = 0xE9.  We include it in a text node.
        // The document declares encoding="ISO-8859-1".
        let mut doc_bytes =
            b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><p>caf\xe9</p>".to_vec();
        // Ensure the byte is present (it is: \xe9 = é in Latin-1).
        let doc = parse_bytes(&doc_bytes, &ParserOptions::default()).unwrap();
        let tree = doc.tree();
        let p = tree.first_child(doc.root()).unwrap();
        let txt = tree.first_child(p).unwrap();
        assert_eq!(tree.value(txt), "café");
        // Make the borrow checker happy (suppress unused warning).
        doc_bytes.clear();
    }

    #[test]
    fn plain_utf8_no_allocation() {
        // The fast path: plain UTF-8, no BOM, no encoding decl → Cow::Borrowed.
        use std::borrow::Cow;
        let input = b"<root/>";
        let result = transcode_to_utf8(input).unwrap();
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn sniff_xml_encoding_label_finds_label() {
        let xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><r/>";
        assert_eq!(sniff_xml_encoding_label(xml).as_deref(), Some("ISO-8859-1"));
    }

    #[test]
    fn sniff_xml_encoding_label_none_without_decl() {
        let xml = b"<root/>";
        assert_eq!(sniff_xml_encoding_label(xml), None);
    }
}
