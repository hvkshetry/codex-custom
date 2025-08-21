Contributing Guide
==================

Thanks for your interest in contributing!

- Development workflow, testing, and release guidance are in `README.md`.
- Pull requests are welcome. Keep changes focused and include tests where possible.
- By default, this fork uses a CLA workflow referencing `docs/CLA.md`. You may remove `.github/workflows/cla.yml` if you do not wish to require a CLA.
- Before large changes, consider filing an issue to discuss direction.

Getting Started
---------------

- Node.js 22+ and pnpm are recommended for JS tooling in `codex-cli/`.
- Rust toolchain is required for `codex-rs/` components; see `codex-rs/README.md`.

Code Style
----------

- Use Prettier for JS/TS and `rustfmt`/`clippy` for Rust.
- Keep commits atomic; ensure tests pass locally.

