//! libxml2-rs — unified public API for the libxml2-rs workspace.
//!
//! See `docs/architecture/overview.md` for the full design.
#![warn(missing_docs)]

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
    /// The document contains no root element.
    NoRootElement,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ParseError::InvalidUtf8 => write!(f, "input is not valid UTF-8"),
            ParseError::NotWellFormed { offset, message } => {
                write!(f, "not well-formed at byte {offset}: {message}")
            }
            ParseError::NoRootElement => write!(f, "document has no root element"),
        }
    }
}

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
            message: "tokenizer initialisation failed",
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

/// Convert a [`xml_tree::BuildError`] to a [`ParseError`].
fn build_err_to_parse_err(e: xml_tree::BuildError) -> ParseError {
    match e {
        xml_tree::BuildError::NoRootElement => ParseError::NoRootElement,
        xml_tree::BuildError::UnexpectedEndTag => ParseError::NotWellFormed {
            offset: 0,
            message: "unexpected end tag",
        },
        xml_tree::BuildError::UnboundNamespacePrefix(_) => ParseError::NotWellFormed {
            offset: 0,
            message: "unbound namespace prefix",
        },
    }
}

/// Convert a [`xml_tokenizer::TokenError`] to a [`ParseError`].
fn token_err_to_parse_err(e: xml_tokenizer::TokenError) -> ParseError {
    match e {
        xml_tokenizer::TokenError::InvalidUtf8 => ParseError::InvalidUtf8,
        xml_tokenizer::TokenError::UnexpectedEof => ParseError::NotWellFormed {
            offset: 0,
            message: "unexpected end of input",
        },
        xml_tokenizer::TokenError::IllegalCharacter { offset } => ParseError::NotWellFormed {
            offset,
            message: "illegal character",
        },
    }
}

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
        // `<<` is illegal — the tokenizer catches it immediately.
        let result = parse_bytes(b"<<", &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::NotWellFormed { .. })));
    }

    #[test]
    fn empty_input_returns_no_root_error() {
        let result = parse_bytes(b"", &ParserOptions::default());
        assert!(matches!(result, Err(ParseError::NoRootElement)));
    }
}
