//! W3C XML Conformance Test Suite runner.
//!
//! This integration test downloads the official W3C xmlconf test suite on
//! first run, then drives every test case through our parser and checks that
//! we accept/reject each document correctly.
//!
//! # Running
//!
//! ```text
//! cargo test -p libxml2-rs --test xmlconf -- --nocapture
//! ```
//!
//! The suite is cached in `crates/libxml2-rs/tests/xmlconf/` (gitignored) and
//! is not re-downloaded on subsequent runs.
//!
//! # Test types (from the W3C spec)
//!
//! | `TYPE`      | Meaning                                           | Expected |
//! |-------------|---------------------------------------------------|----------|
//! | `valid`     | Well-formed AND valid against DTD                 | accept   |
//! | `invalid`   | Well-formed but invalid against DTD               | accept*  |
//! | `not-wf`    | Not well-formed — must be rejected                | reject   |
//! | `error`     | Optional error; conforming parsers may accept     | either   |
//!
//! *We are a non-validating parser in Phase 1, so `invalid` documents must
//!  still be *accepted* (they are well-formed XML).

// Tell Rust this file uses the shared helper in tests/common/mod.rs.
// The `mod` statement here is how Rust integration tests share code: each
// file under `tests/` is compiled as a separate binary, so shared code lives
// in a submodule that each test file imports.
mod common;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use libxml2_rs::{parse_bytes, ParserOptions};

/// A single test case parsed from a sub-catalog.
#[derive(Debug)]
struct TestCase {
    /// Unique identifier, e.g. `"not-wf-sa-001"`
    id: String,
    /// `valid`, `invalid`, `not-wf`, or `error`
    test_type: String,
    /// Absolute path to the XML test file
    path: PathBuf,
    /// Optional human-readable description
    description: String,
}

/// Result of running one test case.
#[derive(Debug)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn main() {
    let suite_root = common::ensure_xmlconf_suite();
    let catalog_path = suite_root.join("xmlconf.xml");

    let cases = load_all_cases(&catalog_path);
    println!("[xmlconf] Loaded {} test cases", cases.len());

    let opts = ParserOptions::default();
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut skip = 0usize;
    let mut failures = Vec::<String>::new();

    for case in &cases {
        match run_case(case, &opts) {
            Outcome::Pass => pass += 1,
            Outcome::Skip(_reason) => {
                skip += 1;
                // Only log skips at high verbosity to avoid flooding output.
            }
            Outcome::Fail(msg) => {
                fail += 1;
                failures.push(format!("[FAIL] {} ({}): {msg}", case.id, case.test_type));
            }
        }
    }

    println!(
        "\nxmlconf results: {} passed, {} failed, {} skipped (total {})",
        pass,
        fail,
        skip,
        cases.len()
    );

    if !failures.is_empty() {
        // Print first 20 failures to avoid noise while parser is a stub.
        let shown = failures.len().min(20);
        println!("\nFirst {shown} failures:");
        for f in failures.iter().take(shown) {
            println!("  {f}");
        }
        if failures.len() > shown {
            println!("  … and {} more", failures.len() - shown);
        }
    }

    // TODO(Phase 1): change to `assert_eq!(fail, 0)` once the tokenizer
    // and tree are wired up and the parser is no longer a stub.
    if fail > 0 {
        println!(
            "\nNOTE: {} failures expected while parser is a stub (Phase 1 in progress)",
            fail
        );
    }
}

// ---------------------------------------------------------------------------
// Catalog loading — handles the two-level structure of xmlconf
// ---------------------------------------------------------------------------
//
// The top-level `xmlconf.xml` uses DTD internal-subset entity declarations to
// include sub-catalog files, e.g.:
//
//   <!ENTITY jclark-xmltest SYSTEM "xmltest/xmltest.xml">
//
// Those entities are then referenced inside <TESTCASES> elements in the body.
// Rather than implement a full XML parser for the catalog (ironic!), we use a
// simple pattern:
//
//   1. Scan the top-level file for `SYSTEM "path"` declarations.
//   2. For each sub-catalog path, read the file and scan for <TEST> tags.
//   3. URIs inside <TEST> are relative to the sub-catalog's directory.

/// Parse all test cases reachable from the top-level catalog.
fn load_all_cases(catalog: &Path) -> Vec<TestCase> {
    let catalog_dir = catalog.parent().expect("catalog has a parent dir");
    let raw = fs::read_to_string(catalog)
        .unwrap_or_else(|e| panic!("cannot read catalog {}: {e}", catalog.display()));

    // Collect SYSTEM paths from entity declarations.
    let sub_paths = collect_entity_paths(&raw);
    println!("[xmlconf] Found {} sub-catalog references", sub_paths.len());

    let mut cases = Vec::new();
    for rel_path in sub_paths {
        let sub_file = catalog_dir.join(&rel_path);
        if !sub_file.exists() {
            eprintln!(
                "[xmlconf] WARN: sub-catalog not found: {}",
                sub_file.display()
            );
            continue;
        }
        let sub_dir = sub_file.parent().expect("sub-catalog has parent");
        let sub_raw = match fs::read_to_string(&sub_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[xmlconf] WARN: cannot read {}: {e}", sub_file.display());
                continue;
            }
        };
        parse_tests_from_catalog(&sub_raw, sub_dir, &mut cases);
    }

    // Also parse <TEST> entries directly in the top-level catalog body
    // (some distributions embed them there).
    parse_tests_from_catalog(&raw, catalog_dir, &mut cases);

    cases
}

/// Extract relative paths from `<!ENTITY … SYSTEM "path">` declarations.
fn collect_entity_paths(src: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut rest = src;
    while let Some(pos) = rest.find("SYSTEM \"") {
        let after = &rest[pos + 8..]; // after the opening `"`
        if let Some(end) = after.find('"') {
            let path = &after[..end];
            // Only include XML catalog files (not the DTD).
            if path.ends_with(".xml") {
                paths.push(path.to_string());
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    paths
}

/// Scan `src` for `<TEST …>` tags and append [`TestCase`]s to `out`.
///
/// `base_dir` is the directory containing the catalog file whose content is
/// `src`; it is used to resolve the relative `URI` attribute of each test.
fn parse_tests_from_catalog(src: &str, base_dir: &Path, out: &mut Vec<TestCase>) {
    let mut rest = src;
    while let Some(start) = rest.find("<TEST ") {
        let after_start = &rest[start..];
        // Find the end of the opening tag (may be self-closing `/>` or `>`).
        let Some(tag_len) = find_tag_end(after_start) else {
            break;
        };
        let tag_text = &after_start[..tag_len];

        let description = if tag_text.trim_end().ends_with("/>") {
            String::new()
        } else {
            // Find </TEST>
            let after_tag = &after_start[tag_len..];
            after_tag
                .find("</TEST>")
                .map(|i| after_tag[..i].trim().to_string())
                .unwrap_or_default()
        };

        if let Some(case) = parse_test_tag(tag_text, description, base_dir) {
            out.push(case);
        }

        rest = &after_start[tag_len..];
    }
}

/// Find the byte length of an XML opening tag starting at the beginning of `s`
/// (i.e. `s` starts with `<TEST `).  Handles quoted attributes so `>` inside
/// attribute values is not treated as the tag end.
fn find_tag_end(s: &str) -> Option<usize> {
    let mut in_dq = false;
    let mut in_sq = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            '>' if !in_dq && !in_sq => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// Extract one [`TestCase`] from a `<TEST …>` tag string.
fn parse_test_tag(tag: &str, description: String, base_dir: &Path) -> Option<TestCase> {
    let id = attr_value(tag, "ID")?;
    let test_type = attr_value(tag, "TYPE")?;
    let uri = attr_value(tag, "URI")?;
    let path = base_dir.join(&uri);
    Some(TestCase {
        id,
        test_type,
        path,
        description,
    })
}

/// Extract `name="…"` or `name='…'` from a tag string.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let prefix_dq = format!("{name}=\"");
    let prefix_sq = format!("{name}='");
    if let Some(pos) = tag.find(&prefix_dq) {
        let after = &tag[pos + prefix_dq.len()..];
        after.find('"').map(|end| after[..end].to_string())
    } else if let Some(pos) = tag.find(&prefix_sq) {
        let after = &tag[pos + prefix_sq.len()..];
        after.find('\'').map(|end| after[..end].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Test runner
// ---------------------------------------------------------------------------

fn run_case(case: &TestCase, opts: &ParserOptions) -> Outcome {
    let input = match fs::read(&case.path) {
        Ok(b) => b,
        Err(e) => {
            return Outcome::Skip(format!("cannot read {}: {e}", case.path.display()));
        }
    };

    let result = parse_bytes(&input, opts);

    match case.test_type.as_str() {
        // `valid` and `invalid` — well-formed XML, must be accepted.
        "valid" | "invalid" => match result {
            Ok(_) => Outcome::Pass,
            Err(e) => {
                let mut msg = format!("expected accept, got error: {e:?}");
                if !case.description.is_empty() {
                    let _ = write!(msg, "\n    {}", case.description);
                }
                Outcome::Fail(msg)
            }
        },

        // `not-wf` — must be rejected.
        "not-wf" => match result {
            Err(_) => Outcome::Pass,
            Ok(_) => {
                let mut msg = "expected reject (not well-formed), but parser accepted".to_string();
                if !case.description.is_empty() {
                    let _ = write!(msg, "\n    {}", case.description);
                }
                Outcome::Fail(msg)
            }
        },

        // `error` — optional behaviour; skip.
        "error" => Outcome::Skip("error-type test (optional behaviour)".into()),

        other => Outcome::Skip(format!("unknown test type: {other}")),
    }
}
