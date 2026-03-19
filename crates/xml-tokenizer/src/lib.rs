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

use memchr::memchr;

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
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    /// Create a new tokenizer over `input`.
    ///
    /// # Errors
    ///
    /// Returns [`TokenError::InvalidUtf8`] if `input` is not valid UTF-8.
    pub fn new(input: &'a [u8]) -> Result<Self, TokenError> {
        // Validate UTF-8 upfront; this lets every inner method use
        // `from_utf8_unchecked` without re-checking — zero cost on hot path.
        core::str::from_utf8(input).map_err(|_| TokenError::InvalidUtf8)?;

        // Skip UTF-8 BOM (0xEF 0xBB 0xBF) if present at the start.
        // The XML spec permits (but does not require) a BOM before the
        // XML declaration.
        let pos = if input.starts_with(b"\xef\xbb\xbf") {
            3
        } else {
            0
        };

        Ok(Self { input, pos })
    }

    /// Advance to the next token.
    ///
    /// Returns [`Token::Eof`] when the input is exhausted.
    pub fn next_token(&mut self) -> Result<Token<'a>, TokenError> {
        if self.pos >= self.input.len() {
            return Ok(Token::Eof);
        }
        // Dispatch: markup starts with `<`, everything else is text.
        if self.input[self.pos] == b'<' {
            self.pos += 1; // consume `<`
            self.scan_after_lt()
        } else {
            self.scan_text()
        }
    }

    // -----------------------------------------------------------------------
    // Low-level helpers
    // -----------------------------------------------------------------------

    /// Return a `&'a str` slice of `input[start..end]`.
    ///
    /// # Safety
    ///
    /// Callers must ensure `start` and `end` are on UTF-8 character
    /// boundaries.  This is guaranteed because:
    /// 1. The whole input was validated as UTF-8 in `new()`.
    /// 2. Every scan method only advances `pos` by scanning ASCII bytes (which
    ///    are always single-byte codepoints, hence valid boundaries) or by
    ///    reading char boundaries via `char_indices()`.
    #[inline]
    fn slice_str(&self, start: usize, end: usize) -> &'a str {
        // Safety: see doc comment above.
        unsafe { core::str::from_utf8_unchecked(&self.input[start..end]) }
    }

    /// `input[pos..]` as `&str`.
    #[inline]
    fn tail_str(&self) -> &'a str {
        self.slice_str(self.pos, self.input.len())
    }

    /// Return the byte at `pos` without consuming it, or `None` at end.
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    /// Return the byte at `pos + 1` without consuming it.
    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.input.get(self.pos + offset).copied()
    }

    /// Advance past any XML whitespace (space, tab, CR, LF).
    #[inline]
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.pos += 1;
        }
    }

    /// Consume `byte` or return an error.
    fn expect(&mut self, byte: u8) -> Result<(), TokenError> {
        match self.peek() {
            Some(b) if b == byte => {
                self.pos += 1;
                Ok(())
            }
            Some(_) => Err(TokenError::IllegalCharacter { offset: self.pos }),
            None => Err(TokenError::UnexpectedEof),
        }
    }

    // -----------------------------------------------------------------------
    // Name scanning
    // -----------------------------------------------------------------------

    /// Scan an XML `Name` starting at `self.pos`.
    ///
    /// A Name begins with a `NameStartChar` (letter, `_`, `:`, …) followed by
    /// zero or more `NameChar`s.  We delegate to `xml_chars` for the exact
    /// Unicode ranges so we match the spec precisely.
    fn scan_name(&mut self) -> Result<&'a str, TokenError> {
        // `tail_str()` is safe here because we just validated UTF-8.
        let s = self.tail_str();
        let mut chars = s.char_indices();

        // First character: must be NameStartChar.
        //
        // Rust note: `char_indices()` yields (byte_offset, char) pairs where
        // `byte_offset` is relative to the start of the string slice.  The
        // offset of the *next* character is `byte_offset + char.len_utf8()`.
        let (_, first) = chars.next().ok_or(TokenError::UnexpectedEof)?;
        if !xml_chars::is_name_start_char(first) {
            return Err(TokenError::IllegalCharacter { offset: self.pos });
        }

        // Remaining characters: must be NameChar.
        // We scan until we hit a non-NameChar or reach end-of-input.
        let end = loop {
            match chars.next() {
                None => break s.len(),
                Some((i, c)) if !xml_chars::is_name_char(c) => break i,
                _ => {}
            }
        };

        let name = &s[..end];
        self.pos += end;
        Ok(name)
    }

    // -----------------------------------------------------------------------
    // Token scanners
    // -----------------------------------------------------------------------

    /// Scan a text node: raw bytes up to the next `<` (or end of input).
    fn scan_text(&mut self) -> Result<Token<'a>, TokenError> {
        let start = self.pos;

        // `memchr` uses SIMD on supported platforms — much faster than a
        // byte-by-byte loop.  It returns the *offset* of the match relative
        // to the slice we pass in, or `None` if not found.
        let len = memchr(b'<', &self.input[self.pos..]).unwrap_or(self.input.len() - self.pos);
        self.pos += len;

        let raw = self.slice_str(start, self.pos);
        // Only set needs_unescape when `&` is actually present, so callers
        // can skip unescaping on the common case (no entities).
        let needs_unescape = raw.contains('&');
        Ok(Token::Text {
            raw,
            needs_unescape,
        })
    }

    /// Dispatch after `<` has been consumed.
    fn scan_after_lt(&mut self) -> Result<Token<'a>, TokenError> {
        match self.peek() {
            None => Err(TokenError::UnexpectedEof),
            Some(b'/') => {
                self.pos += 1;
                self.scan_end_tag()
            }
            Some(b'?') => {
                self.pos += 1;
                self.scan_pi()
            }
            Some(b'!') => {
                self.pos += 1;
                self.scan_bang()
            }
            // Any other byte: element start tag.
            Some(_) => self.scan_start_tag(),
        }
    }

    /// Dispatch after `<!` has been consumed.
    fn scan_bang(&mut self) -> Result<Token<'a>, TokenError> {
        // Three possibilities: `<!--` (comment), `<![CDATA[`, `<!DOCTYPE`.
        match (self.peek(), self.peek_at(1)) {
            (Some(b'-'), Some(b'-')) => {
                self.pos += 2; // consume `--`
                self.scan_comment()
            }
            (Some(b'['), _) => {
                if self.input[self.pos..].starts_with(b"[CDATA[") {
                    self.pos += 7; // consume `[CDATA[`
                    self.scan_cdata()
                } else {
                    Err(TokenError::IllegalCharacter { offset: self.pos })
                }
            }
            _ => {
                if self.input[self.pos..].starts_with(b"DOCTYPE") {
                    self.pos += 7; // consume `DOCTYPE`
                    self.scan_doctype()
                } else {
                    Err(TokenError::IllegalCharacter { offset: self.pos })
                }
            }
        }
    }

    /// Scan `<!-- content -->`.
    ///
    /// The XML 1.0 spec forbids `--` anywhere inside a comment unless it is
    /// the closing `-->`.  We enforce that here.
    fn scan_comment(&mut self) -> Result<Token<'a>, TokenError> {
        let start = self.pos;
        loop {
            // Find the next `-` — comments end with `-->` and `--` anywhere
            // else is illegal.
            let Some(rel) = memchr(b'-', &self.input[self.pos..]) else {
                return Err(TokenError::UnexpectedEof);
            };
            let abs = self.pos + rel;

            if self.input.get(abs + 1) == Some(&b'-') {
                // Found `--`.  Per spec, must be followed by `>`.
                if self.input.get(abs + 2) == Some(&b'>') {
                    let content = self.slice_str(start, abs);
                    self.pos = abs + 3; // skip past `-->`
                    return Ok(Token::Comment { content });
                } else {
                    // `--` inside comment not immediately closed is illegal.
                    return Err(TokenError::IllegalCharacter { offset: abs });
                }
            }
            // Single `-`, keep scanning from just after it.
            self.pos = abs + 1;
        }
    }

    /// Scan `<![CDATA[ content ]]>`.
    fn scan_cdata(&mut self) -> Result<Token<'a>, TokenError> {
        let start = self.pos;
        // Use memmem to search for the two-byte-plus sequence `]]>` in one
        // pass rather than manually tracking state.
        match memchr::memmem::find(&self.input[self.pos..], b"]]>") {
            None => Err(TokenError::UnexpectedEof),
            Some(rel) => {
                let content = self.slice_str(start, self.pos + rel);
                self.pos += rel + 3; // skip past `]]>`
                Ok(Token::CData { content })
            }
        }
    }

    /// Scan `<!DOCTYPE name ...>`.
    fn scan_doctype(&mut self) -> Result<Token<'a>, TokenError> {
        // Must be followed by whitespace before the document type name.
        if !matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            return Err(TokenError::IllegalCharacter { offset: self.pos });
        }
        self.skip_ws();

        let name = self.scan_name()?;
        self.skip_ws();

        let mut public_id = None;
        let mut system_id = None;

        if self.input[self.pos..].starts_with(b"PUBLIC") {
            self.pos += 6;
            self.skip_ws();
            public_id = Some(self.scan_quoted()?);
            self.skip_ws();
            // System literal after PUBLIC is optional.
            if matches!(self.peek(), Some(b'"') | Some(b'\'')) {
                system_id = Some(self.scan_quoted()?);
                self.skip_ws();
            }
        } else if self.input[self.pos..].starts_with(b"SYSTEM") {
            self.pos += 6;
            self.skip_ws();
            system_id = Some(self.scan_quoted()?);
            self.skip_ws();
        }

        // Skip optional internal subset `[...]`, then consume closing `>`.
        self.skip_doctype_tail()?;

        Ok(Token::DoctypeDecl {
            name,
            public_id,
            system_id,
        })
    }

    /// Skip the optional `[internal subset]` and the closing `>` of DOCTYPE.
    fn skip_doctype_tail(&mut self) -> Result<(), TokenError> {
        if self.peek() == Some(b'[') {
            self.pos += 1; // consume `[`
            let mut depth: usize = 1;
            let mut in_quote: Option<u8> = None;
            loop {
                if self.pos >= self.input.len() {
                    return Err(TokenError::UnexpectedEof);
                }
                let b = self.input[self.pos];
                match in_quote {
                    // Inside a quoted string: only watch for the closing quote.
                    Some(q) if b == q => {
                        in_quote = None;
                    }
                    Some(_) => {}
                    // Outside quotes: track brackets and quoted string openers.
                    None => match b {
                        b'"' | b'\'' => in_quote = Some(b),
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                self.pos += 1; // consume the closing `]`
                                break;
                            }
                        }
                        _ => {}
                    },
                }
                self.pos += 1;
            }
            self.skip_ws();
        }
        self.expect(b'>')
    }

    /// Scan a quoted literal: `"content"` or `'content'`, returning the
    /// content without the surrounding quotes.
    fn scan_quoted(&mut self) -> Result<&'a str, TokenError> {
        let quote = match self.peek() {
            Some(q @ b'"') | Some(q @ b'\'') => {
                self.pos += 1; // consume opening quote
                q
            }
            Some(_) => return Err(TokenError::IllegalCharacter { offset: self.pos }),
            None => return Err(TokenError::UnexpectedEof),
        };
        let start = self.pos;
        let Some(rel) = memchr(quote, &self.input[self.pos..]) else {
            return Err(TokenError::UnexpectedEof);
        };
        let content = self.slice_str(start, self.pos + rel);
        self.pos += rel + 1; // skip past closing quote
        Ok(content)
    }

    /// Scan `<?target data?>` or `<?xml ...?>` (XML declaration).
    fn scan_pi(&mut self) -> Result<Token<'a>, TokenError> {
        let target = self.scan_name()?;

        // `<?xml` is the XML declaration — only `xml` (exact lowercase) is
        // valid; any other case combination (e.g. `<?XML`) is reserved and
        // therefore illegal per the XML spec.
        if target == "xml" {
            return self.scan_xml_decl();
        }
        if target.eq_ignore_ascii_case("xml") {
            return Err(TokenError::IllegalCharacter {
                offset: self.pos - target.len(),
            });
        }

        // PI data: optional, preceded by mandatory whitespace.
        let data = if matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.skip_ws();
            let start = self.pos;
            let end = self.find_pi_end()?;
            let s = self.slice_str(start, end);
            // Treat empty data (just whitespace before `?>`) as no data.
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            // No data: must be immediately followed by `?>`.
            if self.peek() == Some(b'?') && self.peek_at(1) == Some(b'>') {
                self.pos += 2;
            } else {
                return Err(TokenError::IllegalCharacter { offset: self.pos });
            }
            None
        };

        Ok(Token::ProcessingInstruction { target, data })
    }

    /// Scan forward to find `?>`, consume it, and return the offset of the
    /// `?` (i.e. the exclusive end of any PI data before `?>`).
    fn find_pi_end(&mut self) -> Result<usize, TokenError> {
        match memchr::memmem::find(&self.input[self.pos..], b"?>") {
            None => Err(TokenError::UnexpectedEof),
            Some(rel) => {
                let data_end = self.pos + rel;
                self.pos = data_end + 2; // skip `?>`
                Ok(data_end)
            }
        }
    }

    /// Parse `<?xml version="1.0" encoding="…" standalone="yes|no"?>`.
    ///
    /// Called after the target name `xml` has been consumed.
    fn scan_xml_decl(&mut self) -> Result<Token<'a>, TokenError> {
        // At minimum one whitespace char before `version` is required.
        if !matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            return Err(TokenError::IllegalCharacter { offset: self.pos });
        }
        self.skip_ws();

        // version="…"  (required)
        let version = self.parse_pseudo_attr("version")?;
        self.skip_ws();

        // encoding="…"  (optional, must come before standalone)
        let encoding = if self.input[self.pos..].starts_with(b"encoding") {
            let enc = self.parse_pseudo_attr("encoding")?;
            self.skip_ws();
            Some(enc)
        } else {
            None
        };

        // standalone="yes|no"  (optional)
        let standalone = if self.input[self.pos..].starts_with(b"standalone") {
            let s = self.parse_pseudo_attr("standalone")?;
            self.skip_ws();
            Some(s == "yes")
        } else {
            None
        };

        self.expect(b'?')?;
        self.expect(b'>')?;

        Ok(Token::XmlDecl {
            version,
            encoding,
            standalone,
        })
    }

    /// Parse one pseudo-attribute of the form `name="value"` or `name='value'`.
    ///
    /// Pseudo-attributes appear only inside the XML declaration.
    fn parse_pseudo_attr(&mut self, expected_name: &str) -> Result<&'a str, TokenError> {
        if !self.input[self.pos..].starts_with(expected_name.as_bytes()) {
            return Err(TokenError::IllegalCharacter { offset: self.pos });
        }
        self.pos += expected_name.len();
        self.skip_ws();
        self.expect(b'=')?;
        self.skip_ws();
        self.scan_quoted()
    }

    /// Scan `<name attrs>` or `<name attrs/>`.
    fn scan_start_tag(&mut self) -> Result<Token<'a>, TokenError> {
        let name = self.scan_name()?;

        // Record where the raw attribute text starts (right after the name).
        let attrs_start = self.pos;

        // `scan_to_tag_end` advances `pos` past the closing `>` or `/>` and
        // returns whether the tag is self-closing.
        let self_closing = self.scan_to_tag_end()?;

        // Compute the exclusive end of the attributes, excluding the `>` or
        // `/>` that `scan_to_tag_end` just consumed.
        let attrs_end = if self_closing {
            self.pos - 2 // strip `/>`
        } else {
            self.pos - 1 // strip `>`
        };

        // Trim leading/trailing whitespace from raw_attrs so callers see
        // `attr="val"` rather than ` attr="val" `.
        let raw_attrs = self.slice_str(attrs_start, attrs_end).trim();
        Ok(Token::StartTag {
            name,
            raw_attrs,
            self_closing,
        })
    }

    /// Scan `</name>`.
    fn scan_end_tag(&mut self) -> Result<Token<'a>, TokenError> {
        let name = self.scan_name()?;
        self.skip_ws();
        self.expect(b'>')?;
        Ok(Token::EndTag { name })
    }

    /// Scan forward past the end of the current tag (`>` or `/>`), tracking
    /// quoted attribute values so a `>` inside a value is not mistaken for
    /// the tag end.
    ///
    /// Returns `true` for self-closing (`/>`), `false` for `>`.
    fn scan_to_tag_end(&mut self) -> Result<bool, TokenError> {
        let mut in_quote: Option<u8> = None;
        loop {
            if self.pos >= self.input.len() {
                return Err(TokenError::UnexpectedEof);
            }
            let b = self.input[self.pos];
            match in_quote {
                // Inside a quoted value: only the matching quote ends it.
                Some(q) if b == q => {
                    in_quote = None;
                    self.pos += 1;
                }
                Some(_) => {
                    self.pos += 1;
                }
                None => match b {
                    b'"' | b'\'' => {
                        in_quote = Some(b);
                        self.pos += 1;
                    }
                    b'>' => {
                        self.pos += 1;
                        return Ok(false);
                    }
                    b'/' if self.peek_at(1) == Some(b'>') => {
                        self.pos += 2;
                        return Ok(true);
                    }
                    _ => {
                        self.pos += 1;
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: collect all tokens from a byte slice.
    fn tokens(input: &[u8]) -> Result<Vec<Token<'_>>, TokenError> {
        let mut tok = Tokenizer::new(input)?;
        let mut out = Vec::new();
        loop {
            let t = tok.next_token()?;
            let done = t == Token::Eof;
            out.push(t);
            if done {
                break;
            }
        }
        Ok(out)
    }

    // --- Eof / empty input ---

    #[test]
    fn empty_input_gives_eof() {
        assert_eq!(tokens(b"").unwrap(), vec![Token::Eof]);
    }

    #[test]
    fn bom_only_gives_eof() {
        assert_eq!(tokens(b"\xef\xbb\xbf").unwrap(), vec![Token::Eof]);
    }

    // --- Text nodes ---

    #[test]
    fn plain_text() {
        let toks = tokens(b"hello world").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Text {
                    raw: "hello world",
                    needs_unescape: false
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn text_with_entity_sets_needs_unescape() {
        let toks = tokens(b"a &amp; b").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::Text {
                    raw: "a &amp; b",
                    needs_unescape: true
                },
                Token::Eof,
            ]
        );
    }

    // --- Start / end tags ---

    #[test]
    fn self_closing_tag() {
        let toks = tokens(b"<br/>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StartTag {
                    name: "br",
                    raw_attrs: "",
                    self_closing: true
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn open_tag() {
        let toks = tokens(b"<root>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StartTag {
                    name: "root",
                    raw_attrs: "",
                    self_closing: false
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tag_with_attribute() {
        let toks = tokens(b"<a href=\"x\">").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StartTag {
                    name: "a",
                    raw_attrs: "href=\"x\"",
                    self_closing: false
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn gt_inside_attribute_value_is_not_tag_end() {
        // `>` inside a quoted attribute value must not be treated as the tag
        // end — this is one of the trickier cases in tag scanning.
        let toks = tokens(b"<a b=\"x>y\">").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StartTag {
                    name: "a",
                    raw_attrs: "b=\"x>y\"",
                    self_closing: false
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn end_tag() {
        let toks = tokens(b"</root>").unwrap();
        assert_eq!(toks, vec![Token::EndTag { name: "root" }, Token::Eof]);
    }

    #[test]
    fn element_round_trip() {
        let toks = tokens(b"<a>text</a>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::StartTag {
                    name: "a",
                    raw_attrs: "",
                    self_closing: false
                },
                Token::Text {
                    raw: "text",
                    needs_unescape: false
                },
                Token::EndTag { name: "a" },
                Token::Eof,
            ]
        );
    }

    // --- Comments ---

    #[test]
    fn comment() {
        let toks = tokens(b"<!-- hello -->").unwrap();
        assert_eq!(
            toks,
            vec![Token::Comment { content: " hello " }, Token::Eof]
        );
    }

    #[test]
    fn comment_with_single_dash_inside() {
        let toks = tokens(b"<!-- a-b -->").unwrap();
        assert_eq!(toks, vec![Token::Comment { content: " a-b " }, Token::Eof]);
    }

    #[test]
    fn double_dash_in_comment_is_error() {
        // XML 1.0 §2.5: `--` is forbidden inside comments.
        assert_eq!(
            tokens(b"<!-- a -- b -->"),
            Err(TokenError::IllegalCharacter { offset: 7 })
        );
    }

    #[test]
    fn unclosed_comment_is_eof_error() {
        assert_eq!(tokens(b"<!-- unclosed"), Err(TokenError::UnexpectedEof));
    }

    // --- CDATA ---

    #[test]
    fn cdata_section() {
        let toks = tokens(b"<![CDATA[some <markup> here]]>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::CData {
                    content: "some <markup> here"
                },
                Token::Eof
            ]
        );
    }

    #[test]
    fn empty_cdata() {
        let toks = tokens(b"<![CDATA[]]>").unwrap();
        assert_eq!(toks, vec![Token::CData { content: "" }, Token::Eof]);
    }

    // --- Processing instructions ---

    #[test]
    fn pi_with_data() {
        let toks = tokens(b"<?foo bar baz?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::ProcessingInstruction {
                    target: "foo",
                    data: Some("bar baz")
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn pi_without_data() {
        let toks = tokens(b"<?foo?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::ProcessingInstruction {
                    target: "foo",
                    data: None
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn uppercase_xml_pi_target_is_error() {
        // `<?XML` is reserved and must not appear.
        assert!(matches!(tokens(b"<?XML version='1.0'?>"), Err(_)));
    }

    // --- XML declaration ---

    #[test]
    fn xml_decl_version_only() {
        let toks = tokens(b"<?xml version=\"1.0\"?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::XmlDecl {
                    version: "1.0",
                    encoding: None,
                    standalone: None
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn xml_decl_with_encoding() {
        let toks = tokens(b"<?xml version='1.0' encoding='UTF-8'?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::XmlDecl {
                    version: "1.0",
                    encoding: Some("UTF-8"),
                    standalone: None,
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn xml_decl_all_attrs() {
        let toks =
            tokens(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::XmlDecl {
                    version: "1.0",
                    encoding: Some("UTF-8"),
                    standalone: Some(true),
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn xml_decl_standalone_no() {
        let toks = tokens(b"<?xml version=\"1.0\" standalone=\"no\"?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::XmlDecl {
                    version: "1.0",
                    encoding: None,
                    standalone: Some(false)
                },
                Token::Eof,
            ]
        );
    }

    // --- DOCTYPE ---

    #[test]
    fn doctype_simple() {
        let toks = tokens(b"<!DOCTYPE root>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::DoctypeDecl {
                    name: "root",
                    public_id: None,
                    system_id: None
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn doctype_system() {
        let toks = tokens(b"<!DOCTYPE html SYSTEM \"about:legacy-compat\">").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::DoctypeDecl {
                    name: "html",
                    public_id: None,
                    system_id: Some("about:legacy-compat"),
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn doctype_public() {
        let toks = tokens(
            b"<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.01//EN\" \
              \"http://www.w3.org/TR/html4/strict.dtd\">",
        )
        .unwrap();
        assert_eq!(
            toks,
            vec![
                Token::DoctypeDecl {
                    name: "html",
                    public_id: Some("-//W3C//DTD HTML 4.01//EN"),
                    system_id: Some("http://www.w3.org/TR/html4/strict.dtd"),
                },
                Token::Eof,
            ]
        );
    }

    #[test]
    fn doctype_with_internal_subset() {
        let toks = tokens(b"<!DOCTYPE foo [<!ENTITY bar \"baz\">]><foo/>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::DoctypeDecl {
                    name: "foo",
                    public_id: None,
                    system_id: None
                },
                Token::StartTag {
                    name: "foo",
                    raw_attrs: "",
                    self_closing: true
                },
                Token::Eof,
            ]
        );
    }

    // --- Error cases ---

    #[test]
    fn invalid_utf8_is_error() {
        assert!(matches!(
            Tokenizer::new(b"\xff\xfe"),
            Err(TokenError::InvalidUtf8)
        ));
    }

    #[test]
    fn truncated_open_bracket_is_eof_error() {
        assert_eq!(tokens(b"<"), Err(TokenError::UnexpectedEof));
    }

    #[test]
    fn unclosed_start_tag_is_eof_error() {
        assert_eq!(tokens(b"<root"), Err(TokenError::UnexpectedEof));
    }

    // --- BOM handling ---

    #[test]
    fn bom_before_xml_decl() {
        let toks = tokens(b"\xef\xbb\xbf<?xml version=\"1.0\"?>").unwrap();
        assert_eq!(
            toks,
            vec![
                Token::XmlDecl {
                    version: "1.0",
                    encoding: None,
                    standalone: None
                },
                Token::Eof,
            ]
        );
    }
}
