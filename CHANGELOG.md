# Changelog

All notable changes to Elyra Grove are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5] — 2026-06-30

### Fixed

- **GUI now connects to the daemon reliably.** `GrovePaths` uses a fixed
  `Grove` directory (e.g. `~/Library/Application Support/Grove`) instead of a
  reverse-DNS ProjectDirs name, so the CLI, root daemon and GUI always agree on
  the same home + IPC socket. Previously the GUI looked in `com.elyra.Grove`
  while the daemon ran in `Grove`, so it showed “Stopped”.

### Added

- `sudo grove install` now also **ensures the system resolver and root CA**, so
  `*.test` keeps resolving even if another tool (e.g. Herd) removed
  `/etc/resolver/<tld>`.

## [0.1.4] — 2026-06-30

### Added

- **Root background service** on macOS: `sudo grove install` now installs a
  system **LaunchDaemon** that binds the privileged ports (53/80/443), starts at
  boot, and runs PHP workers as your user (`GROVE_RUN_USER`). This is the piece
  that makes `*.test` serving work after just installing the app + running
  `sudo grove install` — no more manual `sudo grove start`.

### Fixed

- The daemon's IPC socket is now world-accessible, so the user-level GUI can
  talk to the root daemon.

## [0.1.3] — 2026-06-30

### Fixed

- **PHP now serves under a privileged (root) daemon**: PHP-FPM workers run as
  the real user (`SUDO_USER`/`GROVE_RUN_USER`) with `--allow-to-run-as-root` on
  the master, instead of php-fpm refusing to start as root.
- **Static assets are served directly** (try_files): existing files such as
  built Vite assets under `/build/` are returned as-is instead of being routed
  through `index.php`, so SPA/Vite front-ends render correctly.

### Changed

- Bumped `tauri-action` to v1 and several GUI dev-dependencies (Dependabot).

## [0.1.2] — 2026-06-29

### Added

- The desktop app now **bundles the `grove` CLI** as a sidecar, so it can locate
  and start the daemon (with fallbacks to common install paths).
- macOS builds are **code-signed and notarized**, so the app opens without the
  configured — no more “app is damaged” on download.

### Fixed

- GUI “spawning daemon: No such file or directory” when the CLI wasn't on PATH.

## [0.1.1] — 2026-06-29

### Added

- **In-app auto-update** (macOS/Linux GUI): the app checks for new releases on
  launch and offers a one-click “Install & restart”. Updates are cryptographically
  signed; the release pipeline publishes signed updater artifacts + `latest.json`.

## [0.1.0] — 2026-06-29

First public release. A native, cross-platform local development environment in
Rust that serves `*.test` domains with local HTTPS, multi-version PHP/Node and
bundled services — with zero external dependencies.

### Core

- **Embedded DNS resolver** for `*.<tld>` (default `test`) → loopback; refuses
  any other TLD so it can't act as an open resolver (hickory).
- **HTTP/HTTPS reverse proxy** binding 80/443, routing by `Host` header
  (hyper), with a **minimal built-in FastCGI client** to PHP-FPM.
- **Driver system**: Laravel, WordPress, generic PHP, static, and reverse-proxy
  (Vite/Node) — auto-detected from filesystem signatures.
- **Local TLS**: a private root CA generated on first run, with per-site leaf
  certificates issued on demand via SNI (rcgen + rustls, `ring` provider).
- **Declarative TOML config** as the single source of truth.
- Single long-running **daemon** binding the privileged ports; CLI and GUI are
  thin clients over a Unix-socket JSON-RPC (`grove-ipc`).

### Runtimes

- **Bundled PHP**: download self-contained static PHP-FPM builds
  (`grove php install 8.5|8.4|8.3`) — no Homebrew/Herd. Plus bring-your-own
  (`grove php register`) and auto-discovery.
- **Per-site PHP** version (`grove isolate`) with lazy, on-demand FPM pools.
- **Bundled Node.js**: download official node/npm/npx builds
  (`grove node install 22`); **per-site Node** version (`grove node use`).

### Services (bundled, no separate install)

- **PostgreSQL** and **MySQL** via portable prebuilt binaries; **Redis** built
  from source on install — all downloaded and supervised by Grove under
  `$GROVE_HOME/services`.
- `grove service install|start|stop|restart`, persisted auto-start that only
  runs **installed** services on daemon boot, and per-service port config.
- Built-in **mail-catcher**: an SMTP server that captures outgoing mail, with a
  Mailpit-style viewer.
- `grove env [site]` generates a `.env` snippet wiring an app to the bundled
  services (DB/Redis/mail).

### Sites

- `grove new` — scaffold a fresh **Laravel** project (bundled PHP CLI +
  Composer) or a **static** site, or **link an existing** project.
- `grove park` / `link` / `secure` / `proxy`; `~/Code` is parked by default on
  `grove init`.
- **Valet import** (`grove import`) for migrating existing setups.

### GUI (Tauri 2 + Svelte 5)

- Desktop app sharing the Elyra Conductor look & feel (Tokyo Night palette,
  JetBrains Mono), as a thin client over the daemon.
- Panels: **Sites** (driver, per-site PHP/Node, HTTPS toggle, open in
  browser/Finder), **Services**, **Mail**, **PHP**, **Node**, **Logs**,
  **Doctor**, plus **Settings** (⌘,) and **About**.
- **Create New Site** wizard and **Park folder** import.
- **macOS menu-bar icon**: click to open, right-click to quit; closing the
  window hides Grove to the menu bar.
- Animated boot splash.

### Lifecycle & ops

- `grove init` (first-run setup), `start` / `stop` / `restart`, `gui`,
  `install` / `uninstall` as an OS service (launchd/systemd), `doctor`,
  `logs`, and `--json` everywhere for scripting / elyra-conductor.
- macOS resolver + trust-store integration; Linux/Windows stubs.

### Notes

- macOS is the verified platform for 0.1.0. Linux/Windows resolver and trust
  integration are stubbed and tracked for a later release.

[0.1.5]: https://github.com/kwhorne/grove/releases/tag/v0.1.5
[0.1.4]: https://github.com/kwhorne/grove/releases/tag/v0.1.4
[0.1.3]: https://github.com/kwhorne/grove/releases/tag/v0.1.3
[0.1.2]: https://github.com/kwhorne/grove/releases/tag/v0.1.2
[0.1.1]: https://github.com/kwhorne/grove/releases/tag/v0.1.1
[0.1.0]: https://github.com/kwhorne/grove/releases/tag/v0.1.0
