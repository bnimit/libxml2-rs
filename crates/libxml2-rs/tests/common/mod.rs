//! Shared test utilities for libxml2-rs integration tests.
//!
//! This module handles downloading and caching the W3C XML Conformance Test
//! Suite (xmlconf) so integration tests can run against it without committing
//! 2 MB of test data to the repo.
//!
//! # How it works
//!
//! The first time `ensure_xmlconf_suite()` is called, it:
//!   1. Downloads the tarball from the W3C website using `curl`.
//!   2. Verifies the SHA-256 digest (pinned in this file).
//!   3. Extracts the archive into `tests/xmlconf/`.
//!
//! On subsequent runs the extracted directory is detected and the download is
//! skipped.  The `.tar.gz` file is deleted after successful extraction to save
//! disk space.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// URL of the W3C XML Conformance Test Suite tarball.
///
/// This is the 2013-09-23 edition — the most recent published release.
const SUITE_URL: &str = "https://www.w3.org/XML/Test/xmlts20130923.tar.gz";

/// Expected SHA-256 hex digest of the tarball.
///
/// Re-verify with: `sha256sum xmlts20130923.tar.gz`
const SUITE_SHA256: &str = "9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f";

/// Returns the path to the extracted `xmlconf/` directory, downloading and
/// extracting it first if necessary.
///
/// # Panics
///
/// Panics (i.e. fails the test) if the download, checksum, or extraction
/// fails.  Error messages are printed to stdout so they appear in `--nocapture`
/// CI output.
pub fn ensure_xmlconf_suite() -> PathBuf {
    // Resolve relative to the *manifest directory* of this crate so the path
    // is stable regardless of where `cargo test` is invoked from.
    //
    // CARGO_MANIFEST_DIR is set by Cargo for every integration test binary.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suite_dir = manifest_dir.join("tests/xmlconf");
    let tarball = manifest_dir.join("tests/xmlts20130923.tar.gz");

    // Check whether the suite has already been extracted.
    let marker = suite_dir.join("xmlconf/xmlconf.xml");
    if marker.exists() {
        println!("[xmlconf] Suite already present at {}", suite_dir.display());
        return suite_dir.join("xmlconf");
    }

    println!("[xmlconf] Downloading W3C XML Conformance Test Suite…");
    fs::create_dir_all(&suite_dir).expect("create tests/xmlconf dir");

    download_file(SUITE_URL, &tarball);
    verify_sha256(&tarball, SUITE_SHA256);
    extract_tarball(&tarball, &suite_dir);

    // Clean up the raw tarball — the extracted tree is what we need.
    let _ = fs::remove_file(&tarball);

    let xmlconf_root = suite_dir.join("xmlconf");
    assert!(
        xmlconf_root.join("xmlconf.xml").exists(),
        "extraction succeeded but xmlconf/xmlconf.xml not found"
    );

    println!("[xmlconf] Suite ready at {}", xmlconf_root.display());
    xmlconf_root
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn download_file(url: &str, dest: &Path) {
    println!("[xmlconf]   curl {} -> {}", url, dest.display());
    let status = Command::new("curl")
        .args([
            "--fail",       // non-zero exit on HTTP error
            "--location",   // follow redirects
            "--silent",     // no progress bar
            "--show-error", // but do print errors
            "--output",
            dest.to_str().expect("dest path is valid UTF-8"),
            url,
        ])
        .status()
        .expect("failed to spawn curl — is it installed?");

    assert!(status.success(), "curl failed with status {status}");
}

fn verify_sha256(file: &Path, expected_hex: &str) {
    println!("[xmlconf]   verifying SHA-256…");

    // Read the file and compute the digest using the `sha2` crate — but since
    // we don't want to add it as a dev-dep just for this helper, we use the
    // platform `shasum`/`sha256sum` command instead.
    //
    // On macOS the command is `shasum -a 256`; on Linux it's `sha256sum`.
    let (cmd, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("shasum", &["-a", "256"])
    } else {
        ("sha256sum", &[])
    };

    let output = Command::new(cmd)
        .args(args)
        .arg(file)
        .output()
        .unwrap_or_else(|_| panic!("failed to run {cmd}"));

    assert!(output.status.success(), "{cmd} failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Output format: "<hex>  <path>"
    // On Windows, sha256sum (GNU coreutils via Git Bash) prefixes the hash
    // with `\` when the file path contains backslashes, to signal escaping.
    // Strip that leading backslash before comparing.
    let actual_hex = stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('\\');

    assert_eq!(
        actual_hex,
        expected_hex,
        "SHA-256 mismatch for {}!\n  expected: {}\n  actual:   {}",
        file.display(),
        expected_hex,
        actual_hex
    );
    println!("[xmlconf]   checksum OK");
}

fn extract_tarball(tarball: &Path, dest_dir: &Path) {
    println!(
        "[xmlconf]   extracting {} -> {}",
        tarball.display(),
        dest_dir.display()
    );
    let status = Command::new("tar")
        .args([
            "xzf",
            tarball.to_str().expect("tarball path is valid UTF-8"),
            "-C",
            dest_dir.to_str().expect("dest path is valid UTF-8"),
        ])
        .status()
        .expect("failed to spawn tar");

    assert!(status.success(), "tar failed with status {status}");
}
