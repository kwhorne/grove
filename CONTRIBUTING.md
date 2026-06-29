# Contributing to Grove

Thanks for your interest in improving Grove! This document covers how to get a
dev build running and the conventions the project follows.

## Getting started

```bash
# Requirements: Rust 1.80+, and (for the GUI) Node 20+ with pnpm.
git clone https://github.com/kwhorne/grove
cd grove
cargo build

# Frontend (only needed when working on the GUI)
cd crates/grove-gui/ui && pnpm install && pnpm build && cd -
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the crate layout and
[docs/TESTING.md](docs/TESTING.md) for how to run a build end to end.

## Before you open a pull request

Run the same checks CI does:

```bash
cargo fmt --all
cargo clippy --workspace --exclude grove-gui --all-targets -- -D warnings
cargo test --workspace --exclude grove-gui
# GUI:
cd crates/grove-gui/ui && pnpm run build   # svelte-check + vite build
```

- **Format** with `cargo fmt` and keep `clippy` clean (no warnings).
- **Add tests** for new behaviour where it makes sense.
- Keep commits focused; conventional-commit-style messages
  (`feat:`, `fix:`, `docs:`, `ci:` …) are appreciated.
- Update `CHANGELOG.md` under an *Unreleased* heading for user-facing changes.

## Working on the GUI

The GUI binary **embeds** the frontend at compile time. After changing the UI,
rebuild the frontend **and** the binary:

```bash
pnpm --dir crates/grove-gui/ui build
cargo build -p grove-gui
```

## Scope

Grove is a local development environment, not a production server. Please keep
proposals aligned with that goal — see the open questions in the project
discussions before large changes.

## Reporting bugs / requesting features

Use the issue templates. For security issues, **do not** open a public issue —
see [SECURITY.md](SECURITY.md).

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).
