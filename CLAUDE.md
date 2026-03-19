# CLAUDE.md — libxml2-rs

A memory-safe Rust replacement for libxml2. Full design rationale is in `docs/architecture/overview.md`.

## Repository Layout Convention

This project follows standard OSS file placement rules — **do not reorganise these without good reason:**

**Root-level files (GitHub magic files — must stay at root):**
- `README.md`, `LICENSE`, `CLAUDE.md` — project identity
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `MAINTAINERS.md` — community health files; GitHub surfaces these automatically in PRs, issues, and the community tab
- `SECURITY.md` — GitHub wires this to the "Report a vulnerability" button; must be at root
- `CHANGELOG.md` — convention for release consumers and tooling

**`docs/` — reference and user-facing documentation:**
- `docs/architecture/` — technical design documents (start here for context)
  - `docs/architecture/overview.md` — the primary architecture plan
- `docs/guide/` — mdBook user guide (Phase 1 deliverable)

**`crates/` — library crates (one directory per crate)**

**`tools/` — binary crates (CLI tools)**

**`fuzz/`, `benches/`, `tests/` — quality infrastructure at workspace root**

**`.github/`** — GitHub Actions workflows, issue templates, `CODEOWNERS`

## Essential Commands

```bash
cargo build --workspace                          # build all crates
cargo test --workspace                           # all tests
cargo test --workspace --features conformance-tests  # + W3C/OASIS suites
cargo clippy --workspace --all-features -- -D warnings
cargo fmt --check
cargo +nightly miri test -p xml-tree -p xml-tokenizer  # UB detection
cargo fuzz run parse_xml -- -max_len=65536       # fuzzing
cargo bench -p xml-tree                          # benchmarks
```

## Workspace — Crate Responsibilities

| Crate | Purpose | no_std? |
|---|---|---|
| `xml-chars` | Unicode XML character class tables | ✅ no_std, no alloc |
| `xml-tokenizer` | Zero-copy byte scanner, SAX2 event emitter | ✅ no_std + alloc |
| `xml-ns` | Namespace URI/prefix types | ✅ no_std + alloc |
| `xml-tree` | Arena-based mutable DOM (`NodeId` index tree) | ✅ no_std + alloc |
| `xml-xpath` | XPath 1.0 bytecode VM | std |
| `xml-schema` | W3C XML Schema (XSD) validation | std |
| `xml-relaxng` | RelaxNG validation | std |
| `xml-xinclude` | XInclude processing | std |
| `xml-c14n` | Canonical XML | std |
| `xml-catalog` | OASIS XML Catalogs | std |
| `html-parser` | HTML5 (wraps html5ever) | std |
| `libxml2-rs` | Public API facade — re-exports all the above | std |
| `libxml2-rs-c` | C ABI layer (`cdylib`) — cbindgen generated header | std |

**Never add `std`-only dependencies to `xml-chars`, `xml-tokenizer`, `xml-ns`, or `xml-tree` without a `std` feature flag.**

## Core Design Decisions — Do Not Deviate Without Discussion

**Tree representation:** `NodeId(u32)` index into `Document.nodes: Vec<NodeData>` — not `Rc<RefCell<Node>>`, not raw pointers, not `Box<Node>`. Every node field that references another node uses `Option<NodeId>`. This is what makes `Document: Send + Sync`.

**String storage:** `bumpalo::Bump` for text/attribute values. `IndexSet<Box<str>>` for interned element/attribute names (`NameId(u32)` index). Never store `String` per-node.

**XPath engine:** Compiles to `Vec<Op>` bytecode — not `Box<dyn Expression>`. Vtable dispatch per-AST-node is too slow for repeated evaluation.

**Encoding:** All input is transcoded to UTF-8 at the I/O boundary using `encoding_rs`. Internally everything is UTF-8. Never use `iconv` directly.

**Security defaults:** `ParserOptions::default()` must always be safe (XXE off, entity expansion limited, nesting depth capped). Use `ParserOptions::libxml2_compat()` only in tests that verify compatibility with the reference implementation.

## Unsafe Code Rules

- Every `unsafe` block requires a `// Safety:` comment explaining the invariant.
- `#![deny(unsafe_op_in_unsafe_fn)]` is set in all crates — explicit `unsafe {}` inside `unsafe fn` is required.
- New `unsafe` in hot paths (tokenizer, tree) must pass `cargo miri test`.
- Use `memchr` for SIMD byte scanning — do not write raw SIMD intrinsics directly.

## What Not To Do

- No `unwrap()` or `expect()` in library code unless the panic is genuinely impossible — document it with `// Infallible because ...`
- No `eprintln!` or `println!` in library code — use the `log` crate (`log::warn!`, `log::error!`)
- No global mutable state in the Rust API
- No per-node `Box<T>` or `Vec<T>` allocations in the tree — use the arena and flat attribute storage
- Do not implement `Clone` for `Document` naively — deep clone is expensive and must be explicit
- Do not add network I/O to any crate except behind an explicit opt-in feature

## C ABI Layer (`libxml2-rs-c`)

- Symbol names must exactly match libxml2 2.x (`xmlParseDoc`, `xmlFreeDoc`, etc.)
- Every `pub extern "C"` function needs a `/// # Safety` doc comment describing pointer validity requirements
- `Box::into_raw` / `Box::from_raw` is the ownership transfer pattern for opaque handles
- Run `cbindgen` via `build.rs` — never hand-edit the generated header

## Testing Philosophy

- **Conformance first:** the W3C xmlconf suite is the ground truth for correctness, not unit tests
- **Every CVE gets a regression test** in `tests/regression/` named after the CVE ID
- **Differential testing** against reference libxml2 (via `libxml2-sys`) is the integration test for the C ABI layer
- **Fuzz targets** live in `fuzz/fuzz_targets/` — add one for any new parsing path
- A PR that regresses core parse throughput by >5% is blocked pending investigation

## Conformance Targets

| Suite | Target pass rate |
|---|---|
| W3C XML 1.0 xmlconf | ≥ 95% |
| W3C XSD (XSTS) | ≥ 90% |
| OASIS RelaxNG | ≥ 95% |
| html5lib-tests | ≥ 98% |

## MSRV

**Rust 1.85** — minimum version that supports Rust edition 2024 (required by transitive dependencies via `clap`/`criterion`). Do not use features from later versions without updating the MSRV in `Cargo.toml` and the CI matrix.
