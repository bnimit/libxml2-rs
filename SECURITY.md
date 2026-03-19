# Security Policy

## Supported Versions

| Version | Supported |
|---|---|
| `main` branch | ✅ |
| Published releases | Latest minor release only |

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities by emailing the maintainers listed in [MAINTAINERS.md](MAINTAINERS.md) with the subject line: `[SECURITY] libxml2-rs — brief description`.

Include:
- A description of the vulnerability and its potential impact
- Steps to reproduce (a minimal XML document or code sample is ideal)
- The affected crate(s) and version(s)
- Whether you have a proposed fix

We will acknowledge your report within **48 hours** and aim to release a patch within **7 days** of confirming the vulnerability. We will credit reporters in the release notes unless you request otherwise.

## Security Model

libxml2-rs treats all parser inputs as untrusted by default:

- External entity loading (XXE) is **disabled** by default
- Entity expansion depth and total size are **hard-limited** by default
- Document nesting depth is **capped** by default
- Network access during parsing is **not performed** by default

These defaults can be relaxed via `ParserOptions`. See the documentation for `ParserOptions::libxml2_compat()` if you need to replicate libxml2's (permissive) default behavior.

## Scope

The security guarantee of this project is that **safe Rust code using the `libxml2-rs` public API cannot trigger memory-unsafe behavior regardless of input**. Vulnerabilities in scope include:

- Memory safety issues reachable via the safe public API
- Algorithmic DoS (unbounded memory or CPU growth) bypassing the built-in limits
- Incorrect validation results that could be exploited (e.g., accepting an invalid XML Signature)
- Security-relevant behavioral differences from libxml2 in the C ABI compatibility layer

Out of scope: issues that require a caller to explicitly opt in to unsafe behavior (e.g., calling `unsafe extern "C"` functions with invalid pointers) or issues in the caller's own code.
