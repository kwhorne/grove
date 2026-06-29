<div align="center">

<img src="assets/logo.png" alt="Elyra Grove" width="96" height="96" />

# Elyra Grove

**A native local development environment in Rust.**

Grove serves `*.test` domains with automatic routing, local HTTPS, multi-version
PHP and zero external dependencies — from a single Rust core.

[![CI](https://github.com/kwhorne/grove/actions/workflows/ci.yml/badge.svg)](https://github.com/kwhorne/grove/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.80%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](#status)
[![GUI](https://img.shields.io/badge/GUI-Tauri%202%20%2B%20Svelte%205-24c8db.svg?logo=tauri)](#gui-tauri--svelte)

</div>

<p align="center">
  <img src="assets/dashboard.png" alt="Grove dashboard" width="820" />
</p>

---

## Why Grove

Local PHP/Laravel development today means choosing between friction points:

- **Laravel Valet** is elegant and light, but macOS-only and leans on Homebrew + Composer + dnsmasq.
- **Herd** ships static binaries but is closed, won't let you load custom PHP extensions, and gates databases, mail testing and dumps behind a Pro license.
- **Docker / Sail** is flexible but heavy and slow for simple local work.

Grove takes a different path: **one Rust codebase, three platforms, and nothing to install around it.**

| | Valet | Herd | Docker/Sail | **Grove** |
| --- | :---: | :---: | :---: | :---: |
| Cross-platform | macOS only | macOS + Win | ✅ | ✅ |
| No Homebrew/Composer/dnsmasq | ❌ | ✅ | ➖ | ✅ |
| Custom / bring-your-own PHP | ✅ | ❌ | ✅ | ✅ |
| Bundled static PHP | ❌ | ✅ | ➖ | ✅ |
| Idle footprint | tiny | small | heavy | tiny |
| Open / no license wall | ✅ | ❌ | ✅ | ✅ |

## Features

- 🌐 **Automatic `*.test` routing** via an embedded DNS resolver — no manual hosts editing.
- 🔒 **Local HTTPS** with a private root CA and on-demand per-site leaf certificates.
- 🐘 **Multi-version PHP** — global default plus per-site `isolate`, with lazy FPM pools.
- ⬢ **Bundled Node.js** — download and manage Node versions (node · npm · npx); no nvm or Homebrew needed.
- 📦 **Bundled PHP** — `grove php install 8.4` downloads a self-contained static binary.
- 🧩 **Driver system** — Laravel, WordPress, generic PHP, static sites, and reverse proxy (Vite/Node).
- 📧 **Built-in mail-catcher** — an SMTP server that captures outgoing mail, with a Mailpit-style viewer in the GUI.
- 🗄️ **Bundled services** — Grove downloads and supervises PostgreSQL, MySQL and Redis itself (Redis is compiled from source on install), so there is no database/cache or Homebrew to install separately.
- 🖥️ **GUI + CLI in parity** — both are thin clients over the same daemon (JSON-RPC).
- 🪶 **Low footprint** — target < 15 MB idle RAM, < 200 ms cold start.
- 🔌 **Zero external dependencies** — DNS, proxy, FastCGI and TLS are all built in.

## Zero external dependencies

Grove has no runtime dependency on Homebrew, Composer, dnsmasq, OpenSSL or
Laravel Valet. DNS, the reverse proxy, FastCGI and TLS are all built into the
Rust core. Even PHP can be downloaded as a self-contained static binary via
`grove php install` — it links only against the operating system's own
libraries. `grove import` *reads* an existing Valet config if one is present,
but it never requires Valet to be installed.

## Quick start

```bash
# 1. First-run setup: config, root CA, a static PHP build, resolver + trust
sudo grove init

# 2. Start the daemon (binds 80/443/53)
grove start

# 3. Point Grove at your projects
grove park ~/Code           # every subdirectory becomes <name>.test
#   or, inside one project:
grove link

# 4. Open https://myproject.test 🎉
grove secure myproject      # enable HTTPS
grove isolate myproject 8.3 # pin a PHP version for this site
```

From a clean machine to a running `*.test` Laravel app in under five minutes —
no Homebrew, no Composer, no Valet.

## Command reference

| Category | Commands |
| --- | --- |
| Setup | `init`, `ca trust` / `ca uninstall`, `install` / `uninstall` (service) |
| Lifecycle | `daemon`, `start`, `stop`, `restart` |
| Sites | `park` / `unpark`, `link` / `unlink`, `list`, `secure` / `unsecure`, `isolate` / `unisolate`, `proxy` |
| PHP | `php install`, `php register`, `php discover`, `php list`, `use` |
| Node | `node list`, `node install <version>`, `node use <site> <version>`, `node unuse <site>` |
| Services | `service list`, `service install`, `service start`, `service stop`, `service restart` |
| Mail | `mail`, `mail show <id>`, `mail clear` |
| Logs | `logs` (list sources), `logs <site>` (view entries) |
| Operations | `status`, `doctor`, `env [site]`, `import` (Valet) |

Every command supports `--json` for scripting and [Elyra Conductor](https://github.com/kwhorne/elyra-conductor) integration.

## GUI (Tauri + Svelte)

<p align="center">
  <img src="assets/about.png" alt="Grove about" width="380" />
</p>

The GUI is a thin client that proxies everything to the daemon over the same
`grove-ipc` JSON-RPC the CLI uses — they are always in parity. The frontend is
Svelte 5 + Vite and shares the Elyra Conductor look & feel (Tokyo Night palette,
JetBrains Mono). The dashboard surfaces every site with its driver, PHP version,
a one-click HTTPS toggle, isolate, and shortcuts to open in the browser or
Finder, alongside service, mail, logs and `doctor` panels. The Logs panel parses
per-site Laravel logs and Grove's own service logs into a level/date/message view
with a stacktrace detail pane. A Settings panel (⌘,)
manages parked paths, the TLD, default PHP, the mail-catcher port,
launch-at-login and the theme (auto/light/dark).

```bash
cd crates/grove-gui/ui && pnpm install && pnpm build   # build the frontend
cargo tauri dev        # development (requires cargo-tauri: cargo install tauri-cli)
cargo tauri build      # production build / bundling
```

## Configuration

Grove's source of truth is a single declarative TOML file
(`$GROVE_HOME/config.toml`). Runtime state that can be re-derived is kept out of
it, so the file stays human-readable and diff-friendly.

```toml
[general]
tld = "test"
default_php = "8.4"
auto_start = true

[[parked]]
path = "~/Code"

[[sites]]
name = "inside-next"
path = "~/Code/inside-next"
php = "8.4"
secure = true
driver = "laravel"

[[sites]]
name = "frontend"
path = "~/Code/frontend"
driver = "proxy"
proxy_to = "http://127.0.0.1:5173"
```

## Architecture

A single long-running daemon binds the privileged ports (DNS 53, HTTP 80,
HTTPS 443) and supervises the FPM pools. The CLI and GUI are thin clients that
talk to the daemon over local IPC.

```
grove-core      site registry, driver detection, config, paths   (pure, no OS I/O)
grove-ipc       JSON-RPC protocol + transport (CLI/GUI ↔ daemon)
grove-tls       root CA + leaf issuance (rcgen/rustls)
grove-dns       embedded resolver for *.<tld> (hickory)
grove-proxy     HTTP/HTTPS proxy + minimal FastCGI client (hyper)
grove-runtime   PHP version + FPM pool supervisor
grove-os        platform integration (resolver, trust store, elevation)
grove-daemon    long-running process: binds ports, serves IPC
grove-cli       clap frontend (binary: `grove`)
grove-gui       Tauri 2 + Svelte 5 desktop GUI (thin client over grove-ipc)
```

## Building from source

```bash
# Requirements: Rust 1.80+, and (for the GUI) Node 20+ with pnpm.
cargo build --release        # build the CLI + daemon
cargo test                   # run the test suite
```

For local testing without binding privileged ports, set an isolated home and
high ports:

```bash
export GROVE_HOME=/tmp/grove-home
mkdir -p "$GROVE_HOME"
cat > "$GROVE_HOME/config.toml" <<'EOF'
[general]
tld = "test"
default_php = "8.4"
http_port = 8080
https_port = 8443
dns_port = 5354

[[parked]]
path = "~/Code"
EOF
grove daemon
```

## Roadmap

- [x] Phase 0 — DNS + proxy + FastCGI proof of concept
- [x] Phase 1 — CLI MVP: park/link, drivers, local HTTPS, service install
- [x] Phase 2 — multi-version PHP, bring-your-own + bundled static PHP, proxy driver
- [x] Phase 3 — Tauri + Svelte GUI
- [x] Phase 4 (in progress) — mail-catcher + bundled PostgreSQL, MySQL & Redis supervisor
- [ ] Full Linux & Windows resolver/trust integration

## License

[MIT](LICENSE) — provisional; see PRD §14 open questions.

<div align="center">
<sub>Built by <a href="https://kwhorne.com">Knut W. Horne</a> · part of the Elyra ecosystem</sub>
</div>
