# libxml2-rs

A ground-up, memory-safe Rust replacement for [libxml2](https://gitlab.gnome.org/GNOME/libxml2) — the XML library that underpins PHP, Python's lxml, Ruby's Nokogiri, PostgreSQL's XML functions, LibreOffice, and tens of thousands of other projects.

## Why?

libxml2 is one of the most widely deployed C libraries in existence. It is also one of the most CVE-laden — hundreds of memory-safety vulnerabilities over 25 years, almost all structurally caused by manual memory management in C: use-after-free, heap overflows, integer overflows, null pointer dereferences. Rust's ownership model eliminates this entire bug class at compile time.

This project is a **complete, drop-in replacement**:
- Full feature parity with libxml2 2.x (SAX, DOM, XPath, XSD, DTD, RelaxNG, XInclude, C14N, HTML parser, and more)
- A C ABI compatibility layer — binary drop-in replacement via `LD_PRELOAD` or static linking
- Secure-by-default configuration (XXE disabled, entity expansion limited out of the box)
- W3C/OASIS conformance suite compliance at or above libxml2's level

## Status

> **This project is in the architecture and early implementation phase.**
> We are actively seeking contributors, reviewers, and domain experts.

See [ARCHITECTURE_PLAN.md](ARCHITECTURE_PLAN.md) for the full technical design.

### Roadmap

| Phase | Scope | Status |
|---|---|---|
| 1 — Core Parser | XML 1.0, SAX2, mutable DOM, serialization | 🔲 Planning |
| 2 — Extended | XPath 1.0, DTD validation, Reader/Writer, HTML, C ABI | 🔲 Planning |
| 3 — Validation | XSD, RelaxNG, XInclude, C14N, Catalogs | 🔲 Planning |
| 4 — XSLT + Maturity | XSLT 1.0, Schematron, full C ABI parity, security audit | 🔲 Planning |

## Design Highlights

- **Arena + index tree** — `NodeId(u32)` indices into a flat `Vec<NodeData>`. No `Rc<RefCell<>>`, no pointer chasing. `Document: Send + Sync`.
- **Zero-copy tokenizer** — borrows from input, SIMD byte scanning via `memchr`. Only allocates on entity expansion.
- **XPath bytecode VM** — compiles XPath expressions to an instruction sequence rather than `Box<dyn Expression>` vtable dispatch.
- **`encoding_rs`** for all character encoding transcoding (the same library used in Firefox).
- **Secure defaults** — `ParserOptions::default()` disables external entities, limits entity expansion depth/size, and limits nesting depth. Opt-in to libxml2-compatible permissive mode for compatibility testing.

## Workspace Structure

```
crates/
├── xml-chars/        # Unicode XML character class tables (no_std)
├── xml-tokenizer/    # Zero-copy byte scanner (no_std + alloc)
├── xml-ns/           # Namespace URI/prefix types
├── xml-tree/         # Arena-based mutable DOM
├── xml-xpath/        # XPath 1.0 evaluator
├── xml-schema/       # W3C XML Schema (XSD) validation
├── xml-relaxng/      # RelaxNG validation
├── xml-xinclude/     # XInclude processing
├── xml-c14n/         # Canonical XML
├── xml-catalog/      # OASIS XML Catalogs
├── html-parser/      # HTML5 parser (html5ever integration)
├── libxml2-rs/       # Unified public API facade
└── libxml2-rs-c/     # C ABI compatibility layer (cdylib)
tools/
└── xmllint-rs/       # xmllint-compatible CLI tool
```

## Getting Started

> The library is not yet published to crates.io. Once Phase 1 is complete, it will be available as `libxml2-rs`.

```toml
# Cargo.toml (future)
[dependencies]
libxml2-rs = "0.1"
```

```rust
use libxml2_rs::Document;

let doc = Document::parse_str("<root><child id=\"1\">hello</child></root>")?;
let root = doc.root_element().unwrap();
println!("Root: {}", root.name());    // "root"

for child in root.children() {
    println!("  {}: {}", child.name(), child.text().unwrap_or(""));
}
```

## Contributing

We welcome contributions at every level — from reviewing the architecture to implementing individual crates, writing tests, or improving documentation.

**Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.**

Areas where help is especially valuable right now:
- Review of [ARCHITECTURE_PLAN.md](ARCHITECTURE_PLAN.md) — feedback from XML spec experts, parser engineers, and libxml2 users is invaluable
- XSD 1.0 (XML Schema) — the largest and most complex module; domain expertise welcome
- W3C/OASIS conformance test harness infrastructure
- XSLT 1.0 implementation design

## Security

Security is the primary motivation for this project. If you discover a vulnerability, please follow the responsible disclosure process in [SECURITY.md](SECURITY.md).

All parser inputs are treated as untrusted by default. We participate in OSS-Fuzz for continuous fuzzing at scale.

## License

Apache License 2.0 — see [LICENSE](LICENSE).

Compatible with libxml2's MIT license. See [CONTRIBUTING.md](CONTRIBUTING.md) for the DCO sign-off requirement.
