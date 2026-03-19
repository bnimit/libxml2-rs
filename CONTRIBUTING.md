# Contributing to libxml2-rs

Thank you for your interest in contributing. This document covers everything you need to know to get started.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Developer Certificate of Origin (DCO)](#developer-certificate-of-origin-dco)
- [Ways to Contribute](#ways-to-contribute)
- [Development Setup](#development-setup)
- [Pull Request Process](#pull-request-process)
- [Coding Standards](#coding-standards)
- [Testing Requirements](#testing-requirements)
- [Documentation Requirements](#documentation-requirements)
- [Security Vulnerabilities](#security-vulnerabilities)

---

## Code of Conduct

This project follows the [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/). By participating, you agree to uphold this standard. Please report unacceptable behavior by opening a private issue or contacting the maintainers listed in [MAINTAINERS.md](MAINTAINERS.md).

---

## Developer Certificate of Origin (DCO)

This project uses the **Developer Certificate of Origin (DCO)** instead of a CLA. By contributing, you certify that you have the right to submit your contribution and agree to the terms at [developercertificate.org](https://developercertificate.org/).

**Every commit must be signed off:**

```bash
git commit -s -m "your commit message"
```

This adds a `Signed-off-by: Your Name <your@email.com>` trailer to your commit. The DCO bot on GitHub will block PRs that are missing sign-offs.

If you forget to sign off on past commits in a PR, you can fix them with:

```bash
git rebase --signoff HEAD~N   # where N is the number of commits to fix
git push --force-with-lease
```

---

## Ways to Contribute

### Right now (architecture phase)

- **Review [ARCHITECTURE_PLAN.md](ARCHITECTURE_PLAN.md)** — open an issue with your feedback. We especially want input from:
  - libxml2 users who can identify missing or incorrect API descriptions
  - XML/XSD/RelaxNG/XSLT spec experts
  - Rust library authors with experience in parser design or arena allocators
  - Security researchers familiar with libxml2's CVE history

- **Propose design changes** — open a GitHub Discussion or issue before writing large amounts of code; architecture decisions at this stage have outsized impact.

### Ongoing

- **Implement a crate** — pick an unstarted crate from the workspace (see [ARCHITECTURE_PLAN.md](ARCHITECTURE_PLAN.md) Section 7 for phasing) and open an issue to claim it before starting.
- **Write conformance test infrastructure** — W3C xmlconf, W3C XSTS, OASIS RelaxNG, html5lib-tests harnesses.
- **Add fuzz targets** — `fuzz/fuzz_targets/` for any parsing path.
- **Improve documentation** — rustdoc examples, mdBook guide pages, C API migration guide.
- **Bug fixes and CVE regression tests** — add a test in `tests/regression/` for any libxml2 CVE.

### Good first issues

Look for issues tagged `good-first-issue`. These are bounded, well-specified tasks that don't require deep familiarity with the full codebase.

---

## Development Setup

### Prerequisites

- Rust stable (MSRV: **1.81**) — install via [rustup](https://rustup.rs/)
- A C compiler (for the C ABI tests and differential testing against reference libxml2)
- libxml2 development headers (optional, only for differential testing):
  ```bash
  # macOS
  brew install libxml2

  # Debian/Ubuntu
  sudo apt-get install libxml2-dev

  # Fedora/RHEL
  sudo dnf install libxml2-devel
  ```

### Building

```bash
git clone https://github.com/<org>/libxml2-rs
cd libxml2-rs
cargo build --workspace
```

### Running tests

```bash
# All unit and integration tests
cargo test --workspace

# Include W3C conformance tests (downloads test suite on first run)
cargo test --workspace --features conformance-tests

# Specific crate
cargo test -p xml-tree
```

### Linting and formatting

```bash
cargo fmt --check          # Check formatting
cargo clippy --workspace --all-features -- -D warnings
```

Both checks run in CI and must pass before a PR can be merged.

### Running under Miri (undefined behavior detection)

```bash
cargo +nightly miri test -p xml-tree -p xml-tokenizer
```

Miri runs on the `unsafe`-containing crates in CI. If you add `unsafe` code, ensure it passes Miri.

### Fuzzing

```bash
cargo install cargo-fuzz
cargo fuzz run parse_xml -- -max_len=65536
```

See `fuzz/README.md` for details on the available fuzz targets and how to add new ones.

---

## Pull Request Process

1. **Open an issue first** for non-trivial changes to align on approach before investing time.
2. **Fork** the repository and create a branch: `git checkout -b feat/xml-tokenizer-entity-expansion`.
3. **Write tests** — new code must include unit tests and, where applicable, entries in the conformance test allowlist.
4. **Sign off all commits** (`git commit -s`).
5. **Pass CI** — tests, clippy, fmt, and miri must all pass.
6. **Update documentation** — public API changes require updated rustdoc; user-visible behavior changes require an entry in `CHANGELOG.md`.
7. **Open the PR** against `main`. Fill in the pull request template.
8. **Address review feedback** — at least one approval from a crate owner (see `CODEOWNERS`) is required before merge.

### Commit message style

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(xml-tree): add arena-based NodeId lookup

Replaces Vec<Box<NodeData>> with a flat Vec<NodeData> indexed by
NodeId(u32). This eliminates per-node heap allocations and makes
Document: Send + Sync trivially.

Closes #42

Signed-off-by: Your Name <you@example.com>
```

Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `chore`, `security`.

---

## Coding Standards

### Safety

- Every `unsafe` block **must** have a `// Safety:` comment explaining the invariant that makes it sound.
- Use `#![deny(unsafe_op_in_unsafe_fn)]` in all crates — explicit `unsafe {}` blocks inside `unsafe fn` are required.
- Minimize `unsafe` surface area. If a safe abstraction exists, use it.
- Run `cargo geiger` to audit unsafe usage in the dependency tree.

### Error handling

- Use `thiserror`-derived error types. No `unwrap()` or `expect()` in library code except where a panic is genuinely impossible (document with `// Infallible because ...`).
- All parsing functions return `Result<_, XmlError>` or a domain-specific error type.
- No `eprintln!` in library code. Use the `log` crate for diagnostics.

### Performance

- New code in hot paths (`xml-tokenizer`, `xml-tree` traversal) should be benchmarked with `criterion` before and after.
- Avoid unnecessary allocations in the parser. Prefer borrowing from the input slice; only allocate when entity expansion or normalization requires it.
- PRs that introduce >5% regression on the core parse benchmarks will be held for performance investigation.

### API design principles

- **Secure by default** — new parser options that could enable security risks must default to the safe/restrictive value.
- **No global mutable state** in the Rust API. Thread safety must be achievable without `unsafe` on the caller's side.
- **Explicit over implicit** — configuration via `ParserOptions` structs, not global flags.
- Follow the naming conventions established in the existing crates. When adding a function that corresponds to a libxml2 C function, document the equivalent with `/// libxml2 equivalent: xmlFooBar`.

### no_std compatibility

`xml-chars`, `xml-tokenizer`, and `xml-tree` must remain `no_std` + `alloc` compatible. Do not add `std`-only dependencies to these crates without a feature flag.

---

## Testing Requirements

| Change type | Required tests |
|---|---|
| New parsing feature | Unit tests + relevant W3C/OASIS conformance test cases pass |
| New public API | Rustdoc example that compiles and runs as a doctest |
| Bug fix | Regression test in `tests/regression/` (name it after the issue or CVE) |
| Security fix (CVE) | Regression test named `tests/regression/CVE-XXXX-XXXXX.xml` |
| Performance change | Before/after criterion benchmark numbers in PR description |
| `unsafe` code | Passes `cargo miri test` |

---

## Documentation Requirements

- All public items (`pub fn`, `pub struct`, `pub trait`, `pub enum`) must have doc comments.
- Non-trivial functions must include at least one `# Examples` section with a runnable doctest.
- Feature-gated items must use `#[doc(cfg(feature = "..."))]`.
- C API functions must document: the equivalent libxml2 symbol, ownership of pointer arguments, and null-handling behaviour.

---

## Security Vulnerabilities

**Do not open a public issue for security vulnerabilities.**

Follow the process in [SECURITY.md](SECURITY.md). We aim to acknowledge reports within 48 hours and release a patch within 7 days of a confirmed vulnerability.
