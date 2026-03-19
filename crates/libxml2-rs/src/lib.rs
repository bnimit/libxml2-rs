//! libxml2-rs — unified public API for the libxml2-rs workspace.
//!
//! See `docs/architecture/overview.md` for the full design.
//!
//! **Status (Phase 1):** Stub types and API only — no parsing logic yet.
//! Tracking issue: <https://github.com/your-org/libxml2-rs/issues/5>
#![warn(missing_docs)]
// TODO(Phase 1): remove once all stubs are wired up
#![allow(dead_code)]

/// Options that control the XML parser's behaviour.
///
/// The default configuration is *secure*: XXE (XML External Entity) attacks
/// are prevented by disabling external entity loading, and entity expansion is
/// limited to avoid "billion laughs" style DoS attacks.
///
/// # Example
///
/// ```rust
/// use libxml2_rs::ParserOptions;
///
/// let opts = ParserOptions::default(); // secure defaults
/// ```
#[derive(Debug, Clone)]
pub struct ParserOptions {
    /// Maximum depth of entity expansion (0 = use library default).
    pub max_entity_depth: u32,
    /// Allow loading external entities via system/public identifiers.
    /// **Disabled by default** — enabling this opens XXE attack vectors.
    pub load_external_entities: bool,
}

impl Default for ParserOptions {
    fn default() -> Self {
        Self {
            max_entity_depth: 10,
            load_external_entities: false,
        }
    }
}

/// Errors returned by the XML parser.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum ParseError {
    /// The input is not valid UTF-8.
    InvalidUtf8,
    /// The XML is not well-formed.
    NotWellFormed {
        /// Byte offset in the input where the error was detected.
        offset: usize,
        /// Human-readable description of the problem.
        message: &'static str,
    },
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::InvalidUtf8 => write!(f, "input is not valid UTF-8"),
            ParseError::NotWellFormed { offset, message } => {
                write!(f, "not well-formed at byte {offset}: {message}")
            }
        }
    }
}

/// A parsed XML document.
///
/// This is currently a stub — it will wrap `xml_tree::Document` once the
/// tree crate is wired in (Phase 1, Issue #3).
#[derive(Debug)]
pub struct Document {
    // TODO(Phase 1): replace with xml_tree::Document
    _private: (),
}

/// Parse a UTF-8 XML document from a byte slice.
///
/// # Errors
///
/// Returns [`ParseError::InvalidUtf8`] if `input` is not valid UTF-8, or
/// [`ParseError::NotWellFormed`] if the XML is malformed.
///
/// # Example
///
/// ```rust
/// use libxml2_rs::{parse_bytes, ParserOptions};
///
/// let xml = b"<?xml version=\"1.0\"?><root/>";
/// // This currently returns Err because the tokenizer is not yet implemented.
/// let _result = parse_bytes(xml, &ParserOptions::default());
/// ```
pub fn parse_bytes(_input: &[u8], _opts: &ParserOptions) -> Result<Document, ParseError> {
    // TODO(Phase 1): drive xml_tokenizer + xml_tree here
    Err(ParseError::NotWellFormed {
        offset: 0,
        message: "parser not yet implemented",
    })
}
