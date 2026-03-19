//! Unicode XML character class tables.
//!
//! Provides `const` lookup functions for the character productions defined in
//! the XML 1.0 and XML 1.1 specifications — `NameStartChar`, `NameChar`,
//! `Char`, `RestrictedChar`, and whitespace.
//!
//! This crate is `no_std` with no `alloc` dependency.
#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

/// Returns `true` if `c` is a valid XML 1.0 `Char`.
///
/// XML 1.0 §2.2: `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`
#[inline]
pub const fn is_char(c: char) -> bool {
    matches!(c,
        '\u{0009}'
        | '\u{000A}'
        | '\u{000D}'
        | '\u{0020}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Returns `true` if `c` is a valid XML 1.0 `NameStartChar`.
///
/// XML 1.0 §2.3 (5th edition).
#[inline]
pub const fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':'
        | 'A'..='Z'
        | '_'
        | 'a'..='z'
        | '\u{C0}'..='\u{D6}'
        | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}'
        | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}'
    )
}

/// Returns `true` if `c` is a valid XML 1.0 `NameChar`.
///
/// XML 1.0 §2.3 (5th edition): `NameStartChar | "-" | "." | [0-9] | #xB7 | …`
#[inline]
pub const fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c,
            '-' | '.' | '0'..='9' | '\u{B7}'
            | '\u{0300}'..='\u{036F}'
            | '\u{203F}'..='\u{2040}'
        )
}

/// Returns `true` if `c` is XML whitespace (`#x20`, `#x9`, `#xD`, `#xA`).
#[inline]
pub const fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}
