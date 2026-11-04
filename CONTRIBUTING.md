# Contributing to LX Capture

Bug reports, focused fixes, and documentation improvements are welcome.

## Before opening a change

- Check existing issues and pull requests for duplicate reports.
- Describe the Windows version, display configuration, color depth, and smallest reproduction that shows the problem.
- Keep public API changes documented, including their byte-layout and compatibility effects.
- Never attach private screen recordings, credentials, or personal data.

## Local checks

Use the Windows MSVC Rust toolchain for DXGI code. From the repository root:

```powershell
cargo fmt --all --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

For non-Windows targets, only the platform-independent rectangle helpers are available. State which checks you ran and which target you used in a pull request.

## Pull requests

Include the motivation, the changed public behavior, any relevant output example, and check results. Update the guides and Mermaid source when behavior or data flow changes.
