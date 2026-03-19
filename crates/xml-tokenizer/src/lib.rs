//! Zero-copy XML 1.0 tokenizer.
//!
//! Scans a byte slice and emits [`Token`]s as slices into the original input.
//! No allocations are performed unless entity expansion requires it.
//!
//! This crate is `no_std` + `alloc`.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

extern crate alloc;

/// A single lexical token, borrowing from the input slice.
///
/// All `&str` fields are slices into the original input — no copying.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Token<'a> {
    /// `<?xml version="..." encoding="..." standalone="..."?>`
    XmlDecl {
        /// XML version string, e.g. `"1.0"`
        version: &'a str,
        /// Declared encoding, if present
        encoding: Option<&'a str>,
        /// Standalone declaration, if present
        standalone: Option<bool>,
    },
    /// `<!DOCTYPE ...>`
    DoctypeDecl {
        /// Document type name
        name: &'a str,
        /// Public identifier, if present
        public_id: Option<&'a str>,
        /// System identifier, if present
        system_id: Option<&'a str>,
    },
    /// An opening tag: `<name attr="val">`
    StartTag {
        /// Qualified element name
        name: &'a str,
        /// Raw attribute bytes (parsed lazily)
        raw_attrs: &'a str,
        /// `true` for self-closing `<name/>`
        self_closing: bool,
    },
    /// A closing tag: `</name>`
    EndTag {
        /// Qualified element name
        name: &'a str,
    },
    /// Character data between tags
    Text {
        /// Raw text slice; may contain `&amp;` etc. if `needs_unescape` is true
        raw: &'a str,
        /// `true` if the text contains entity references or `&#...;` sequences
        needs_unescape: bool,
    },
    /// `<![CDATA[...]]>`
    CData {
        /// Content, verbatim (no entity processing)
        content: &'a str,
    },
    /// `<!-- ... -->`
    Comment {
        /// Comment content
        content: &'a str,
    },
    /// `<?target data?>`
    ProcessingInstruction {
        /// PI target name
        target: &'a str,
        /// PI data, if any
        data: Option<&'a str>,
    },
    /// End of input
    Eof,
}

/// Tokenizer errors.
#[derive(Debug, PartialEq)]
pub enum TokenError {
    /// Input is not valid UTF-8
    InvalidUtf8,
    /// Unexpected end of input inside a token
    UnexpectedEof,
    /// A character illegal in this context was encountered
    IllegalCharacter {
        /// Byte offset in the input
        offset: usize,
    },
}

/// Zero-copy tokenizer over a UTF-8 byte slice.
pub struct Tokenizer<'a> {
    input: &'a [u8],
    pos:   usize,
}

impl<'a> Tokenizer<'a> {
    /// Create a new tokenizer over `input`.
    ///
    /// # Errors
    ///
    /// Returns [`TokenError::InvalidUtf8`] if `input` is not valid UTF-8.
    pub fn new(input: &'a [u8]) -> Result<Self, TokenError> {
        // Validate UTF-8 upfront; allows unchecked conversion inside the hot path.
        core::str::from_utf8(input).map_err(|_| TokenError::InvalidUtf8)?;
        Ok(Self { input, pos: 0 })
    }

    /// Advance to the next token.
    ///
    /// Returns [`Token::Eof`] when the input is exhausted.
    pub fn next_token(&mut self) -> Result<Token<'a>, TokenError> {
        // TODO(Phase 1): implement full tokenizer state machine
        if self.pos >= self.input.len() {
            return Ok(Token::Eof);
        }
        Err(TokenError::UnexpectedEof)
    }
}
