# Changelog

Changes are listed by release.

## 0.1.1 — 2026-11-05

- Switch `flate2` to the pure-Rust `zlib-rs` backend so docs.rs can build the Windows-targeted API without cross-compiling native zlib C code.
- Keep the public capture API and rectangle payload format unchanged; compressed data remains a standard zlib stream.

## 0.1.0 — 2026-11-04

- Initial public release of the Windows DXGI capture library.
- Expose High Color and True Color capture modes.
- Return compressed full-screen keyframes and changed rectangle updates.
- Include capture, byte-layout, architecture, and security documentation.
