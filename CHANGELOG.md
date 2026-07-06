# Changelog

All notable changes to Elyra Grove are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.2] — 2026-07-06

### Fixed

- **Dev processes are no longer orphaned when the daemon restarts.** On graceful
  shutdown (including `launchctl kickstart`) Grove now kills the per-site Vite /
  queue children, so restarting the daemon doesn't leave stray `vite` servers
  squatting ports (which caused `public/hot` to point at a stale server).

## [0.5.1] — 2026-07-06

### Added

- **Vite over HTTPS, automatically.** When `grove dev` starts the Vite server
  for an HTTPS site, Grove issues a CA-trusted leaf certificate for the host and
  passes it via the standard `VITE_DEV_SERVER_CERT` / `VITE_DEV_SERVER_KEY` env
  vars that `laravel-vite-plugin` reads. Vite then serves HTTPS with a trusted
  cert — no mixed-content, HMR just works — with **no Herd/Valet directories
  involved**. (Use the standard `laravel-vite-plugin` in `vite.config.js`; a
  custom hard-coded Herd cert path won't pick this up.)

## [0.5.0] — 2026-07-06

### Added

- **Per-site dev processes** — Grove runs and supervises a site's long-running
  dev tasks so you don't have to: the **Vite dev server** (`npm run dev`, HMR)
  and, for a non-`sync` queue, a **queue worker** — each with the site's own
  Node/PHP, run as your user, output streamed to the Logs panel. Because Grove
  already serves the app, there's no `artisan serve` to run. Toggle it with the
  ⚡ button per site in the GUI, or `grove dev start|stop|list <site>` — a
  Grove-aware replacement for `composer run dev`.

## [0.4.2] — 2026-07-06

### Fixed

- **Proxy sites now hit the right virtual host.** The reverse proxy set `Host`
  to the upstream authority (and forwards the public host as `X-Forwarded-Host`
  + `X-Forwarded-Proto`), so name-based vhosts — e.g. an nginx container with
  `server_name inside2.local`, or an OrbStack domain — match instead of falling
  through to a default server block. Previously a Docker site could show the
  bare nginx welcome page instead of the app.

## [0.4.1] — 2026-07-06

### Added

- **Compose auto-detection.** Running `docker compose` projects are now served
  as `<project>.test` even without labels — Grove picks the web container
  (by service name / published web port) and proxies to it. Explicit
  `dev.orbstack.domains` / `grove.host` labels still take precedence.
- **Start / stop / restart containers from the GUI.** Docker sites in the Sites
  table gain ▶ / ⏹ / ↻ controls; stopped containers show as `stopped` with a
  Start button, and a stopped site serves a friendly “start it” page.

## [0.4.0] — 2026-07-06

### Added

- **Docker / OrbStack integration.** Grove now auto-discovers running containers
  and serves them as `<name>.test` with its trusted local HTTPS — right next to
  native sites, in the same dashboard. A container is picked up when it carries a
  `dev.orbstack.domains` label (Grove reuses OrbStack's own routing) or an
  explicit `grove.host` label; Grove terminates TLS and reverse-proxies to it.
  Containers appear/disappear live (polled), show a 🐳 badge in the Sites table,
  and — because they're first-class sites now — `grove share` can tunnel them
  publicly too. Toggle with `[general].docker` in `config.toml`.

## [0.3.1] — 2026-07-03

### Added

- **Community starter kits** when creating a site: pick **Custom** in the New
  Site dialog (or `grove new <name> --kind vendor/package`) to scaffold any
  community kit — e.g. a **Svelte** kit — via `laravel new --using=<repo>`.

## [0.3.0] — 2026-07-03

### Changed

- **New sites now scaffold with the official `laravel new` installer** (latest
  Laravel) and a **starter-kit picker** — None, **Livewire**, **React** (Inertia)
  or **Vue** (Inertia) — replacing `composer create-project`. The GUI's “Create a
  new site” dialog gained a Starter kit selector; on the CLI use
  `grove new <name> --kind livewire|react|vue`. Grove installs the Laravel
  installer and a Node runtime on demand (for the asset build) against its
  bundled PHP/Composer/Node, and hands the finished project to your user.

## [0.2.9] — 2026-07-01

### Added

- **Xdebug panel in the GUI** (Tools → *Xdebug step-debugging*): a live on/off
  toggle, the DBGp port, and per-PHP-build availability with a one-click
  *Install debug build* for versions that lack Xdebug.
- **Xdebug step-debugging** (`grove debug on|off|status|env`, and the GUI
  toggle). Grove loads Xdebug into its FPM pools on demand via per-pool `-d` INI
  overrides — the global `php.ini` is never touched, and pools respawn instantly
  when toggled. Xdebug runs in `start_with_request=trigger` mode, so it stays
  dormant (near-zero overhead) until a request opts in with the `XDEBUG_TRIGGER`
  cookie/param; `grove debug env` prints the matching env for debugging CLI
  processes (`eval "$(grove debug env)"`). Grove speaks the runtime half: your
  editor's DAP client listens on DBGp port 9003 and Xdebug connects out to it.

  Step-debugging requires a PHP that **has** Xdebug — a `grove php register`-ed
  dynamic PHP with Xdebug built in, or a loadable `xdebug.so` in its
  `extension_dir`. Grove's own fully-static builds can't load Xdebug (static PHP
  can't `dlopen`, and static-php-cli can't compile it in), so those report as
  unavailable in `grove debug status` / the GUI panel.

[0.5.2]: https://github.com/kwhorne/grove/releases/tag/v0.5.2
[0.5.1]: https://github.com/kwhorne/grove/releases/tag/v0.5.1
[0.5.0]: https://github.com/kwhorne/grove/releases/tag/v0.5.0
[0.4.2]: https://github.com/kwhorne/grove/releases/tag/v0.4.2
[0.4.1]: https://github.com/kwhorne/grove/releases/tag/v0.4.1
[0.4.0]: https://github.com/kwhorne/grove/releases/tag/v0.4.0
[0.3.1]: https://github.com/kwhorne/grove/releases/tag/v0.3.1
[0.3.0]: https://github.com/kwhorne/grove/releases/tag/v0.3.0
[0.2.9]: https://github.com/kwhorne/grove/releases/tag/v0.2.9

## [0.2.8] — 2026-07-01

### Added

- **Convert database** in the Tools panel: copy a whole database between
  **MySQL, PostgreSQL and SQLite** — tables, columns (mapped by category),
  primary keys and all rows. Ideal for turning a MySQL database into a portable
  SQLite file and back. Values transfer as text (blobs as bytes), so dates,
  decimals, JSON and UUIDs survive across dialects. Views, stored routines,
  triggers and foreign keys are not copied.

## [0.2.7] — 2026-07-01

### Added

- **“Restart daemon”** in the Tools panel — restarts Grove's background service
  with one click (no password), so the running daemon picks up a freshly updated
  app. The root LaunchDaemon re-execs itself via `launchctl kickstart`.

## [0.2.6] — 2026-07-01

### Added

- **Tools panel** in the GUI, starting with **“Migrate MySQL from Herd”**: copy
  all databases from another MySQL server (e.g. Laravel Herd) into Grove's MySQL
  via a safe logical dump &amp; restore using Grove's own client tools. The source
  databases are left untouched.

## [0.2.5] — 2026-07-01

### Fixed

- **MySQL and PostgreSQL now start when Grove runs as a root service.** Both
  refuse to run as root (`mysqld`/`postgres`), which broke “Start” under the
  macOS LaunchDaemon (the service flickered green → idle). Grove now runs bundled
  databases as the invoking user — like PHP-FPM — dropping privileges before
  exec, owning their data directories to match (`chown`), and placing their unix
  sockets inside the user-owned data dir. Existing installs are repaired
  automatically on the next start.

## [0.2.4] — 2026-06-30

### Fixed

- **Tunnelled sites now render assets correctly (Vite, CSS, JS).** The tunnel no
  longer rewrites the `Host` header to the local site name — it preserves the
  public host so the app builds correct public asset URLs, and routes locally
  via a new `X-Grove-Site` header instead. It also sets `X-Forwarded-Proto`, and
  Grove's proxy maps it to FastCGI `HTTPS=on`, so apps generate `https://` URLs
  (no mixed-content blocking) without needing TrustProxies configured.

  > Update **both** the macOS app *and* the `grove-tunnel` server on your host to
  > 0.2.4 — the server is what preserves the public host.

## [0.2.3] — 2026-06-30

### Added

- The **public tunnel URL now shows inline** in the Sites row (a 🌍 chip you can
  click to copy) while a site is shared — not just in the transient toast. The
  Tunnels panel continues to list every active tunnel.
- A turnkey `deploy/tunnel/setup.sh` for standing up your own tunnel server in
  one command.

## [0.2.2] — 2026-06-30

### Added

- **Zero-config tunnels.** Grove now defaults to the public tunnel server
  `grove.elyracode.com`, so `grove share <site>` works out of the box and gives
  a `https://<random>.grove.elyracode.com` URL — no `[tunnel]` config needed.
- **Open-server mode.** `grove-tunnel` can run without a token (omit `--token`)
  for a public community server; clients no longer need a token.
- **On-demand HTTPS authorization.** `grove-tunnel` exposes `/__grove_ask` so a
  fronting Caddy can mint per-subdomain Let's Encrypt certificates safely
  (only for hostnames under the server's own domain) — no DNS API required.
- **Deployment kit** in [`deploy/tunnel/`](deploy/tunnel/README.md): Caddyfile,
  systemd unit and a step-by-step guide for running your own server.

## [0.2.1] — 2026-06-30

### Added

- **Tunnel management in the GUI** — a new **Tunnels** panel and a per-row
  **Share** button in the Sites table. The daemon now owns tunnel lifecycles, so
  the GUI/CLI can start, stop and list public tunnels.
- **Request inspector** — a live table of recent tunnelled requests (time, site,
  method, path, status, duration), ideal for debugging webhooks. `grove share`
  also prints requests live in the terminal.
- **Remove a site from the list** — `grove forget <name>` (and a trash button in
  the GUI) hides a site **without deleting its files**; `grove restore <name>`
  brings it back. Backed by a new `ignored` list in `config.toml`.

### Removed

- `docs/SIGNING.md` (internal signing notes) is no longer part of the docs.

## [0.2.0] — 2026-06-30

### Added

- **Public tunnels (`grove share`)** — a native, self-hostable alternative to
  Expose/ngrok, built in with zero external dependencies:
  - `grove share <site>` exposes a local `*.test` site at a public URL for
    demos, real-device testing and **webhooks**.
  - New `grove-tunnel` server binary you deploy on a host with a wildcard
    domain. Requests are multiplexed over a single yamux connection and proxied
    with `hyper` end-to-end (streaming bodies, rewritten `Host`).
  - Options: `--subdomain`, `--server`, `--token`, `--basic-auth`.
  - `[tunnel]` config section (`server`, `token`) so the flags can be omitted.
  - See [docs/TUNNEL.md](docs/TUNNEL.md).

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

[0.2.8]: https://github.com/kwhorne/grove/releases/tag/v0.2.8
[0.2.7]: https://github.com/kwhorne/grove/releases/tag/v0.2.7
[0.2.6]: https://github.com/kwhorne/grove/releases/tag/v0.2.6
[0.2.5]: https://github.com/kwhorne/grove/releases/tag/v0.2.5
[0.2.4]: https://github.com/kwhorne/grove/releases/tag/v0.2.4
[0.2.3]: https://github.com/kwhorne/grove/releases/tag/v0.2.3
[0.2.2]: https://github.com/kwhorne/grove/releases/tag/v0.2.2
[0.2.1]: https://github.com/kwhorne/grove/releases/tag/v0.2.1
[0.2.0]: https://github.com/kwhorne/grove/releases/tag/v0.2.0
[0.1.5]: https://github.com/kwhorne/grove/releases/tag/v0.1.5
[0.1.4]: https://github.com/kwhorne/grove/releases/tag/v0.1.4
[0.1.3]: https://github.com/kwhorne/grove/releases/tag/v0.1.3
[0.1.2]: https://github.com/kwhorne/grove/releases/tag/v0.1.2
[0.1.1]: https://github.com/kwhorne/grove/releases/tag/v0.1.1
[0.1.0]: https://github.com/kwhorne/grove/releases/tag/v0.1.0
