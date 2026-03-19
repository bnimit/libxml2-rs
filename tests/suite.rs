//! Test suite downloader and cache manager.
//!
//! Downloads the W3C XML Conformance Test Suite on first use and caches it
//! in `tests/xmlconf/` so subsequent runs work offline.
//!
//! # How it works
//!
//! In Rust, `#[test]` functions are just regular functions that cargo runs.
//! We can do any setup we need before the actual assertions — including
//! checking whether a file exists and downloading it if not.
//!
//! The `ensure_xmlconf_suite()` function does exactly that:
//! 1. Check if `tests/xmlconf/xmlconf.xml` already exists → done, return path
//! 2. Otherwise, download the tarball from W3C
//! 3. Verify its SHA-256 checksum (so we always test against the exact same files)
//! 4. Extract it into `tests/xmlconf/`
//!
//! This means `git clone` just works with no flags, and CI works with no
//! submodule configuration.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// URL and checksum of the W3C XML Conformance Test Suite.
///
/// Pinning both the URL and the checksum means:
/// - We always test against the exact same ~2,000 test cases
/// - If the file is tampered with or corrupted, the test fails loudly
const XMLCONF_URL: &str =
    "https://www.w3.org/XML/Test/xmlts20130923.tar.gz";

/// SHA-256 of xmlts20130923.tar.gz — update this if the suite is re-released.
const XMLCONF_SHA256: &str =
    "adb4b9b3b4e2efa87e3a26ce1ae4843bbe0e5a503bef1b2e94e05b3f46be5b0d";

/// Returns the path to the xmlconf test suite root, downloading it first if needed.
///
/// Call this at the top of any test that needs the conformance suite:
///
/// ```rust,ignore
/// #[test]
/// fn w3c_conformance() {
///     let suite_dir = ensure_xmlconf_suite();
///     // suite_dir now contains xmlconf.xml and all test files
/// }
/// ```
///
/// # Panics
///
/// Panics with a descriptive message if the download or checksum verification fails.
/// This turns a network/IO problem into a clear test failure rather than a silent skip.
pub fn ensure_xmlconf_suite() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is set by cargo to the workspace root at test time.
    // This gives us a stable path regardless of where `cargo test` is run from.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suite_dir = workspace_root.join("tests").join("xmlconf");
    let catalog = suite_dir.join("xmlconf.xml");

    // Fast path: suite already downloaded
    if catalog.exists() {
        return suite_dir;
    }

    println!("W3C xmlconf suite not found — downloading from W3C...");
    println!("  URL: {}", XMLCONF_URL);

    // Download the tarball into a temp file
    let tarball_path = suite_dir
        .parent()
        .unwrap()
        .join("xmlts20130923.tar.gz");

    fs::create_dir_all(&suite_dir)
        .expect("failed to create tests/xmlconf/ directory");

    download_file(XMLCONF_URL, &tarball_path)
        .expect("failed to download W3C xmlconf test suite");

    // Verify checksum before extracting — never extract untrusted bytes
    verify_sha256(&tarball_path, XMLCONF_SHA256)
        .expect("checksum mismatch on downloaded xmlconf tarball");

    println!("  Checksum verified. Extracting...");

    extract_tarball(&tarball_path, &suite_dir)
        .expect("failed to extract xmlconf tarball");

    // Clean up the tarball — we only need the extracted files
    fs::remove_file(&tarball_path).ok();

    println!("  Done. Suite ready at: {}", suite_dir.display());

    assert!(
        catalog.exists(),
        "xmlconf.xml not found after extraction — tarball structure may have changed"
    );

    suite_dir
}

/// Download `url` to `dest` using only the standard library.
///
/// **Rust concept: `Result<T, E>`**
/// Almost every fallible operation in Rust returns `Result<T, E>`.
/// The `?` operator at the end of a line means:
/// "if this is an Err, return that Err immediately from this function".
/// It's equivalent to writing `match result { Ok(v) => v, Err(e) => return Err(e) }`.
fn download_file(url: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // We use std::process::Command to call `curl` rather than pulling in an
    // HTTP crate as a dev-dependency. Keeps the dependency tree lean.
    // curl is available on all CI runners (ubuntu, macos, windows via git-for-windows).
    let status = std::process::Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",   // follow redirects
            "--output", dest.to_str().unwrap(),
            url,
        ])
        .status()?;

    if !status.success() {
        return Err(format!("curl exited with status: {}", status).into());
    }

    Ok(())
}

/// Verify the SHA-256 checksum of `file` matches `expected_hex`.
///
/// **Rust concept: closures and iterators**
/// The `sha256_hex()` helper below uses iterator chaining — a core Rust idiom.
/// Instead of writing a loop with an accumulator, we chain `.fold()` over bytes.
fn verify_sha256(file: &Path, expected_hex: &str) -> Result<(), String> {
    let actual = sha256_hex(file)
        .map_err(|e| format!("failed to read file for checksum: {}", e))?;

    if actual != expected_hex {
        return Err(format!(
            "checksum mismatch!\n  expected: {}\n  actual:   {}",
            expected_hex, actual
        ));
    }

    Ok(())
}

/// Compute the SHA-256 of a file and return it as a lowercase hex string.
///
/// We implement this without a dependency by shelling out to `shasum`/`certutil`.
fn sha256_hex(file: &Path) -> io::Result<String> {
    // Platform-specific: shasum on unix, certutil on windows
    #[cfg(not(target_os = "windows"))]
    let output = std::process::Command::new("shasum")
        .args(["-a", "256", file.to_str().unwrap()])
        .output()?;

    #[cfg(target_os = "windows")]
    let output = std::process::Command::new("certutil")
        .args(["-hashfile", file.to_str().unwrap(), "SHA256"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // shasum output: "<hash>  <filename>"
    // certutil output: contains the hash on the second line
    let hash = stdout
        .lines()
        .find(|line| line.len() == 64 && line.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "could not parse hash output"))?
        .to_lowercase();

    Ok(hash)
}

/// Extract a `.tar.gz` file into `dest_dir` using the `tar` command.
fn extract_tarball(tarball: &Path, dest_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new("tar")
        .args([
            "xzf",
            tarball.to_str().unwrap(),
            "-C", dest_dir.to_str().unwrap(),
            "--strip-components=1",  // removes the top-level directory from the tarball
        ])
        .status()?;

    if !status.success() {
        return Err(format!("tar exited with status: {}", status).into());
    }

    Ok(())
}
