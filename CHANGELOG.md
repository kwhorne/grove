# Changelog

All notable changes to Elyra Grove are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`cpx` is part of the bundled toolchain.** `grove path install` now creates a
  `cpx` shim alongside `php`, `composer`, `node`, `npm`, `npx` and `laravel`, so
  the Composer Package Executor — npx, but for Composer packages — is simply on
  your PATH:

  ```console
  $ cpx laravel/pint
  $ cpx friendsofphp/php-cs-fixer fix ./src
  $ cpx phpstan analyse
  ```

  Nothing to `composer global require`: Grove fetches the self-contained cpx
  PHAR on first use and runs it on the PHP the current directory resolves to
  (`grove isolate` / `grove use`). cpx 2's ad-hoc PHP commands come with it —
  `cpx exec -r '…'` and `cpx tinker` boot your Laravel app (config, facades,
  `.env`, `$app`), and `cpx tinker` hands off to the project's own
  `php artisan tinker` when it has one.

  The PHAR lives at `~/.grove/cpx.phar` rather than under the root-owned
  `$GROVE_HOME`, so `cpx self-update` can replace it in place. cpx needs PHP
  8.3+; a directory pinned to something older gets an explanation instead of a
  parse error out of the PHAR.

- **`grove php ext` — an extension audit.** Grove's bundled PHP comes from
  prebuilt static archives with a *fixed* extension set, and until now nothing
  told you what was in the one you got. `grove php ext` diffs a build's real
  `php -m` against the extensions the ecosystem expects, and says what each gap
  costs:

  ```console
  $ grove php ext
  php@8.5 (common) — 49 modules, 1 required missing, 5 recommended missing

    Missing (required):
      ✗ mysqli         WordPress — its only MySQL driver

    Missing (recommended):
      ✗ intl           Laravel Number/dates, Filament, Nova
      ✗ sodium         modern crypto (sodium_*), passkeys
      ✗ readline       history/editing in tinker and cpx tinker
      ✗ apcu           in-process cache (Laravel apc store)
      ✗ xsl            XSLT transforms (ext-xsl)
  ```

  The same gap now shows up in three other places you'd want it: `grove php
  install` prints required-tier misses right after installing, `grove php list`
  carries a one-line summary per build, `grove doctor` has a `php-extensions`
  check, and the GUI's PHP panel has an **Extensions** section.

- **Grove builds its own PHP.** The prebuilt static-PHP archives Grove used to
  download are not supersets of each other, and each is missing something that
  matters: upstream `common` has `pdo_sqlite`/`pdo_pgsql` but no `intl`,
  `mysqli`, `sodium`, `readline`, `apcu` or `xsl`; upstream `bulk` has those six
  but drops both PDO drivers. Laravel's default database is SQLite, Grove bundles
  PostgreSQL, WordPress speaks only `mysqli`, and a large slice of Packagist
  `require`s `ext-intl` — so no choice between the two was the right one.

  [`.github/workflows/php-build.yml`](.github/workflows/php-build.yml) now builds
  the union with static-php-cli, for macOS and Linux on both architectures, and
  publishes it to a rolling `php-runtimes` release. `grove php install` fetches
  that by default (`--variant grove`, `[general].php_variant = "grove"`).

  The extension list is authored once, in
  `grove_runtime::extensions::BUILD_SET`, and `grove php craft` prints the
  static-php-cli config generated from it — so the binary that audits a build is
  the same one that specified it, and CI can't drift from the audit.

  Until a Grove build exists for a given version, `grove php install` falls back
  to upstream `common`, names the extensions that costs you, and labels the build
  with the set it actually got rather than the one requested. Asking for
  `--variant common` or `--variant bulk` explicitly never falls back: silently
  trading one extension hole for the other would be worse than an error.

  `GROVE_PHP_MIRROR` still points the downloader at a different host for all
  three variants, for a team mirror or an air-gapped cache.

### Changed

- **The default PHP version is now 8.5** (was 8.4) — `[general].default_php`,
  `grove init --php`, and the versions the docs use in examples.
- `grove php install` now fetches the matching **CLI** binary along with
  `php-fpm`, and records which variant a build came from. `grove php ext`,
  `grove php list` and the PATH shims all need the CLI, and auditing a build
  whose `php -m` came from a different archive than the one serving requests
  would have been worse than not auditing at all. Switching a version's variant
  replaces both binaries, so they can never disagree.

## [1.4.2] — 2026-08-08

**Upgrade immediately if you are on 1.4.0 or 1.4.1.** Those releases could not
serve a single request: every connection was reset, on every site.

### Fixed

- **Every request was reset (`ERR_CONNECTION_RESET`) on 1.4.0 and 1.4.1.** The
  header-read timeout added in 1.4.0 was configured without giving hyper a
  timer. hyper does not fall back to a default and does not complain at setup:
  it panics the first time a connection reaches the timeout code, which is every
  connection — *"timeout `header_read_timeout` set, but no timer set"*. The HTTP
  and HTTPS listeners now install `TokioTimer`.

  Two changes in 1.4.0 combined to hide it. Panics no longer abort the process,
  which is right — but it meant the daemon stayed up and kept reporting itself
  healthy, with `grove status` and the GUI both showing **Running**, while not a
  single site rendered. Under the previous `panic = "abort"` the first request
  would have killed the daemon outright and the fault would have been obvious.
- **A request now goes through a connection in the test suite.** Every layer had
  tests of its own — DNS answers, TLS chaining, static revalidation, FastCGI
  framing — and none of them put a request through a connection built the way the
  listeners build it, which is why a misconfigured builder shipped. That test now
  exists, and it fails with the exact panic above when the timer is removed.

## [1.4.1] — 2026-08-08

Saying the right minimum. 1.4.0's dependency updates quietly moved the lowest
Rust version Grove can be built with from 1.80 to 1.94 — `sqlx` 0.9 requires it —
but the manifest, the README badge and CONTRIBUTING all still promised 1.80.
Anyone who followed the documentation got a wall of errors from a transitive
dependency instead of a sentence naming the cause. Nothing about the shipped
binaries changes; this is about being able to build them.

### Fixed

- **The declared minimum Rust version is now true.** `rust-version` said `1.80`
  while the real floor had moved to `1.94`, so `cargo build` on the documented
  version failed inside `time-core` with *"the package requires the Cargo feature
  called `edition2024`"* — which names neither Grove nor the dependency that
  raised the bar. With the correct value, cargo says
  *"rustc 1.80 is not supported by the following package: grove-core requires
  rustc 1.94"* before it compiles anything. The README badge and CONTRIBUTING's
  build instructions said 1.80 too, and now say 1.94.
- **CI checks the minimum instead of assuming it.** Every job used `stable`,
  which is always new enough, so a dependency bump could raise the real floor
  without anything failing — which is exactly how 1.4.0 shipped a manifest that
  was wrong. A new `MSRV` job builds the workspace with the declared version, so
  the claim and the code cannot drift apart again.

### Dependencies

- **The npm group, without the TypeScript 7 jump.** `svelte` 5.56.8, `vite`
  8.2.0, `@sveltejs/vite-plugin-svelte` 7.2.0, `svelte-check` 4.7.4,
  `@fontsource/jetbrains-mono` 5.3.0 and `@tauri-apps/plugin-dialog` 2.7.2.
  TypeScript stays on 6: version 7 is the Go port, which needs both 7 *and* 6
  installed plus a `--tsgo` flag, and `svelte-check` does not drive it yet.
  Dependabot is now told to skip TypeScript majors, so one package that cannot
  move no longer fails the whole grouped update every week.
- `base64` 0.23.1, `thiserror` 2.0.20, `toml` 1.1.3, and `pnpm/action-setup`
  4 → 6.0.10 in the workflows.

## [1.4.0] — 2026-08-08

The same work, once. 1.3.x stopped Grove holding whole bodies in memory; this
release stops it *redoing* things. A proxied request built a new HTTP client —
and a client is the connection pool, so every request to a Vite dev server paid a
fresh TCP handshake. A static asset was re-read and re-sent on every reload,
because responses carried nothing a browser could revalidate against. DNS answers
were marked uncacheable, so the system resolver asked again for every single
connection. And the local CA minted a brand-new certificate each time it was
loaded from disk.

The other half is about staying up. The release profile used `panic = "abort"`,
which turned any single failed request into a dead daemon — no DNS, no TLS, no
sites. The accept loop answered failure with a bare `continue`, which under
`EMFILE` is a busy loop that never recovers. Nothing bounded a TLS handshake or
the wait for request headers, so a peer that connected and went quiet kept a
task and a file descriptor for as long as it liked.

Also: the dependency tree is current again, including four crates whose major
bumps needed real migration rather than a version bump.

### Fixed

- **Reloading the local CA no longer mints a new certificate.** Loading it from
  disk parsed the PEM into params and called `self_signed`, creating a fresh
  certificate — new serial, new validity window — on every daemon start and every
  CLI call. Leaf certificates still chained, since the name and key matched, but
  `cert_pem()` reported a certificate that was neither the file on disk nor the
  one the OS trust store had been told to trust.
- **A panic no longer takes the whole daemon down.** The release profile built
  with `panic = "abort"`, so one unwrap on a poisoned mutex anywhere — in a
  single request, for a single site — aborted the process and with it DNS, TLS
  and every other site. Panics now unwind, which keeps the failure inside the
  tokio task that caused it.
- **The accept loop no longer spins a core when it runs out of file
  descriptors.** `accept` failing was answered with a bare `continue`; under
  `EMFILE` that fails again immediately and forever, so the listener burned 100%
  of a core and never recovered. Failures now back off exponentially (5 ms to
  1 s), which also gives descriptors time to be released.
- **Half-open connections are no longer held forever.** Neither the TLS
  handshake nor the wait for request headers had a deadline, so a peer that
  connected and went quiet — a crashed browser, a port scanner — kept a task and
  a descriptor indefinitely. Handshakes now time out after 10 s and headers
  after 30 s.
- **Blocking filesystem calls off the async path.** `Path::is_file`/`is_dir`/
  `exists` were called on every request to a PHP or static site straight from the
  request task; on a slow volume (a network share, a Docker bind mount) that
  stalls a runtime worker and every other request scheduled on it. They are now
  async stats.
- **Starting a PHP-FPM pool no longer stalls unrelated requests.** Pool lookup
  forks php-fpm and then polls for its socket for up to a second, synchronously.
  It now runs on the blocking pool.

### Changed

- **Proxied requests reuse connections.** A new `hyper` client was constructed
  per request, and a client *is* the connection pool — so every proxied request
  paid a fresh TCP handshake and a Vite dev server saw one connection per asset
  instead of a few kept alive. One shared pooled client now serves the proxy
  driver and replay.
- **Static files revalidate instead of re-transferring.** Responses carry an
  `ETag` (from size + mtime, so no extra read) and answer `If-None-Match` with
  `304`, so reloading a dev site with unchanged assets no longer re-reads and
  re-sends every byte. `Cache-Control: no-cache` keeps an edit from ever serving
  stale.
- **Static files over 256 KiB stream from disk.** A large asset — a video, a
  sourcemap — was read into memory in full, per concurrent request, before the
  first byte reached the browser.
- **DNS answers are cacheable.** Records were served with `TTL 0`, which forbids
  caching, so the system resolver re-queried Grove for every connection — with
  macOS's `mDNSResponder` in that path on every first byte. The answer is always
  loopback and never changes, so it is now `TTL 300`.
- **The release build is optimised for speed rather than size.** `opt-level = "z"`
  optimised the one thing that does not matter for a daemon on the request path.
  Now `opt-level = 3` with fat LTO, which costs about 3 MB of binary.

### Dependencies

- **The cargo group's 25 updates, including the breaking ones.** Four crates
  needed code changes rather than a version bump: `rcgen` 0.13→0.14 (`Issuer`
  replaces passing a certificate and key to `signed_by`), `sqlx` 0.8→0.9
  (non-literal queries now require an explicit `AssertSqlSafe`, with the audit
  written down where the conversion happens), `hickory-dns` 0.24→0.26 (`Server`,
  `zone_handler`, `Metadata`/`HeaderCounts`, `request_info()`), and `age`
  0.10→0.12 (borrowed recipients, and `is_scrypt()` in place of the `Decryptor`
  enum). Also `thiserror` 1→2, `ed25519-dalek` 2→3, `rand` 0.8→0.10, `toml`
  0.8→1.1, `directories` 5→6, `base64` 0.22→0.23.
  - Requests to the DNS resolver carrying anything other than exactly one
    question are now refused rather than guessed at.
  - The `age` upgrade is pinned by a fixture encrypted with the previous
    version, so a format regression cannot silently orphan secrets already on
    disk.

### Upgrading

Nothing to do. Existing certificates, encrypted secrets and `config.toml` are
read as before; the CA on disk is now used as-is rather than re-minted, and
age files written by earlier versions are covered by a regression test.

## [1.3.2] — 2026-07-31

The other direction. 1.3.1 stopped buffering responses; this stops buffering
uploads. A 400 MB upload used to cost Grove **1.2 GB of RSS** — three copies of
the body: one to collect it, one cloned for the request that gets forwarded, one
for the timeline. The same upload now costs about 3 MB.

### Fixed

- **Request bodies stream instead of being buffered whole.** The body was
  collected at the top of dispatch, before Grove had even resolved which site the
  request belonged to, so every upload was held in memory in triplicate.
  - The **proxy driver** (Vite, Node) now forwards the body as it arrives,
    whatever its transfer encoding. CGI's `CONTENT_LENGTH` requirement does not
    apply to an HTTP upstream, so there was nothing to decide here.
  - The **FastCGI path** branches on the body's exact size. When the length is
    known — essentially every browser form post and API upload — the body is
    streamed into `STDIN` records with `CONTENT_LENGTH` set from it.
  - **Chunked requests** declare no length, but CGI must be told one before the
    body is sent. Grove keeps such a body in memory up to 1 MiB and spills to a
    private spool file beyond that, measuring it as it goes. Refusing them with
    `411` would have made Grove the reason a valid request fails; nginx and
    Apache spool, so Grove spools too. Spool files are `0600` inside a `0700`
    directory and are removed when the body is dropped — tied to `Drop`, so
    neither an error nor a client disconnect can leave upload contents on disk.
  - Bodies small enough for the timeline to store in full are still collected, so
    `grove replay` and the curl / `.http` / Pest export are byte-for-byte
    unchanged for what is almost every request. Only uploads larger than the
    timeline's own 1 MiB cap take the streaming path, where a truncated capture
    is already what the timeline would have kept.
- **A request body is now bounded.** Because CGI forces Grove to measure an
  undeclared body before it can forward anything, an unbounded chunked upload was
  a disk-filling vector. Bodies beyond 2 GiB are refused with `413`.
- **FastCGI reads and writes concurrently.** Writing the whole body before
  reading any response would deadlock on a large upload that PHP rejects early:
  Grove blocking on `STDIN` while PHP blocks on its response, each waiting for
  the other's socket buffer to drain. Harmless when bodies were capped by memory;
  a real hazard now that they are not.
- **The curl / `.http` / Pest export says when a body was truncated.** The
  timeline keeps at most 1 MiB of a request body, and the generator ignored the
  flag that recorded it — so an export of a large upload looked complete and
  quietly sent a partial body, which then surfaced as a puzzling response from the
  app rather than as Grove's own limit. Every format now warns first, and a
  truncated JSON body is reported as truncated instead of as invalid JSON, which
  had blamed the application for Grove's capture limit.
- **A truncated capture is recorded as truncated.** The timeline inferred
  truncation from the stored body being longer than the cap, which stopped being
  true once bodies were captured *as they streamed*: the capture ends at exactly
  the cap, indistinguishable from a body that happened to fit. `Record` now
  carries the fact explicitly.

## [1.3.1] — 2026-07-31

Streaming, and two disclosures found while testing it. Grove's proxy buffered
every response whole, so a Server-Sent Events endpoint delivered nothing until
PHP closed the request. Fixing that meant looking closely at the request path,
which turned up a `.php` file being served as source and a `.env` being served
at all. If you use `grove share`, both were reachable from outside.

### Fixed

- **Responses stream instead of being buffered whole.** The FastCGI client
  accumulated every `STDOUT` record and returned only on `END_REQUEST`, and
  `Full<Bytes>` could not express a stream in any case. Grove now returns the
  headers as soon as PHP flushes them and forwards each FastCGI record as an
  HTTP chunk, so SSE arrives live and a large download is no longer held in
  memory — a 2 GB download used to mean 2 GB of RSS. The proxy driver passes the
  upstream body through for the same reason, so Vite HMR and Node SSE endpoints
  behave too. This is deliberately not conditional on `text/event-stream`: the
  problem was general.
- **`.php` files are executed, not served as source.** With a PHP driver, any
  existing `.php` file in the document root was handed back as text — so
  `/index.php` disclosed the front controller on every PHP site. It also meant
  WordPress could not work at all, since `wp-login.php` and `wp-admin/*.php`
  must execute. Such requests now go to PHP-FPM with `SCRIPT_FILENAME` pointing
  at the file, matching what nginx and Apache do. Matched case-insensitively,
  because a case-insensitive filesystem resolves `/INDEX.PHP` to the same file.
- **Dot-prefixed paths are refused.** A plain PHP project's document root is the
  project root, so `/.env` returned `APP_KEY` in full. Any path with a
  dot-prefixed component is now a 404, never served and never executed.
  `.well-known/` is exempt, so ACME HTTP-01 challenges keep working.

### Notes

- `duration_ms` in the request timeline now measures time to headers rather than
  total time, for streaming responses. A 16-second SSE stream used to log 16 s
  and now logs a few milliseconds. That is closer to time-to-first-byte than to
  duration; the Requests panel labels it as duration and will be revisited.
- Request *bodies* are still buffered whole, so a large upload still costs
  memory. Fixing that needs a decision about `CONTENT_LENGTH` for chunked
  requests and about what the request timeline promises when a body is too large
  to capture, so it is deliberately not in a patch release.

## [1.3.0] — 2026-07-30

The application decides. Laravel 13.16 moved dev-process configuration out of
`composer.json` and into the application itself, via `DevCommands`. Grove used to
guess — a Vite server and a queue worker, hardcoded — which meant it was blind to
Reverb, Horizon or `stripe listen`, and assumed `npm` even in a bun project. Now
Grove asks: it reads `artisan dev:list` and supervises exactly what the app
declares, minus the processes Grove already *is*. The app owns the list; Grove
owns the supervision — no open terminal, per-process logs, autostart at boot.

### Added

- **`grove dev` now runs the processes your app declares.** On Laravel 13.16+,
  Grove reads `php artisan dev:list --json --except-vendor` and supervises that
  list instead of guessing, so userland processes registered through
  `DevCommands` — Reverb, Horizon, `stripe listen` — are started alongside Vite
  and the queue worker, each with its own `dev-<site>-<name>.log`. `server` and
  `logs` are skipped: Grove already serves the site over FPM, and `grove logs`
  already tails the app log. Vendor-registered processes are excluded so a
  Composer package can't start processes inside the daemon. Non-Laravel sites and
  older Laravel versions keep the previous behaviour (Vite + queue worker).
  Because Grove reuses Laravel's `NodePackageManager` detection by way of the
  declared command, `pnpm`, `yarn` and `bun` projects now work without special
  casing. Run `grove dev` *instead of* `php artisan dev`, not alongside it.
- **`grove path install` puts the `grove` CLI itself on your `PATH`,** next to
  the `php` / `composer` / `node` / `npm` / `npx` / `laravel` shims. Previously a
  user who installed the macOS app and ran `grove path install` still had no
  `grove` command, because the binary only existed inside the `.app` bundle.
  `grove path show --json` reports this as `cli_installed`.
- **`grove dev start` warns about a competing `php artisan dev`.** Both supervise
  the same processes, so running both silently doubles them. The check cannot
  tell which project the other process belongs to, so it is reported as a
  warning alongside the started processes rather than as an error.

### Changed

- **`grove dev start` / `grove dev stop` take an optional site argument,**
  defaulting to the site in the current directory (resolved from `grove.toml`'s
  `name`, else the directory name) — matching `grove link`, `grove secure` and
  `grove up`.

### Fixed

- **`grove dev stop` no longer orphans processes.** Dev processes were killed
  directly, but `npm run dev` spawns Vite as a *grandchild*, which survived and
  kept holding port 5173 — breaking the next `grove dev start`. Each dev process
  now runs in its own process group and is stopped group-wide (`SIGTERM`, then
  `SIGKILL`).

### Notes

- **Intel macOS is no longer shipped.** Notarization on GitHub's `macos-13`
  runner routinely hangs, so the release workflow builds Apple Silicon and Linux
  only. Intel Macs can still build from source with `cargo build --release`. This
  took effect after 1.2.1; 1.3.0 is the first release to state it.
- **`grove dev` replaces `php artisan dev`** rather than complementing it.
  Running both doubles every process; `grove dev start` now warns when it sees a
  competing `php artisan dev`.
- On Laravel 13.16+, `grove dev start` boots the application once to read
  `dev:list`, which adds a moment of startup latency. If the app cannot boot,
  Grove falls back to the previous heuristic rather than failing.

## [1.2.1] — 2026-07-18

The causal chain, closed loop. 1.1 gave sandboxed writes automatic rollback and
gave requests a causal chain — but the two didn't meet: a rollback that can't
tell you *what a change touched* is just a nicer undo button. 1.2.1 links them.
Every sandboxed write now reports its own **blast radius**, so an agent (or you)
can see exactly what a migration ran before deciding to keep it.

### Added

- **Attributed blast radius for sandboxed writes.** `grove_migrate_sandboxed`
  and `grove_sql_sandboxed` now return a `chain` alongside the schema diff — the
  SQL the operation actually ran and any mail it sent, correlated to the
  operation's time window. Grove enables SQL capture for the duration
  automatically (MySQL) and restores your previous setting afterwards, so an
  agent (or you) can inspect exactly what a migration touched before deciding to
  keep it. Backed by a new `ChainForWindow` IPC command that generalizes the
  request causal chain to any time window.

### Fixed

- **Auto-updater 403s.** The release now rewrites `latest.json` to use the
  public `browser_download_url` for each artifact instead of the
  rate-limited `api.github.com` asset endpoint, so in-place updates no longer
  fail intermittently with `403 Forbidden`.

### Internal

- `grove-services` now declares the `time` crate's `parsing` feature explicitly,
  so the crate builds and tests standalone (previously masked by workspace
  feature unification).

## [1.1.0] — 2026-07-18

The AI release. Grove 1.0 opened its local environment to AI clients over MCP,
read-only. 1.1 turns `grove mcp` into a full AI debugging companion: **safe,
sandboxed writes** with automatic rollback, a **per-request causal chain** that
ties each request to the SQL it ran and the mail it sent, and one-click
**"explain this error"** bundles that gather the request, its side effects, and
the stacktrace for your assistant. The core stays free and open source.

### Added — agent-safe MCP write tools (opt-in)

- **`grove mcp --allow-write`.** The MCP server is read-only by default; write
  tools appear only with this flag (and are refused otherwise). Every write is
  recorded to an audit log at `$GROVE_HOME/logs/mcp-writes.log`.
- **`grove_migrate_sandboxed`.** Runs `php artisan <command>` (default
  `migrate --force`) inside an automatic snapshot sandbox: Grove snapshots the
  database first, runs the migration, reports the schema diff, and
  **automatically rolls back on failure**. Pass `roll_back: true` for a pure dry
  run; on success the snapshot id is returned for manual rollback.
- **`grove_sql_sandboxed`.** Runs a single write statement
  (INSERT/UPDATE/DELETE/DDL) through the same snapshot → run → schema-diff →
  auto-rollback flow, returning `rows_affected`. Read-only statements are
  refused — use `grove_db_query` for those.
- Both write tools cover bundled **MySQL/PostgreSQL** (daemon snapshot) and
  **SQLite** (snapshotted by copying the `.sqlite` file).

### Added — request causal chain

- **`grove_request_chain`** MCP tool (and `RequestChain` IPC command) correlate
  a captured request with the side effects Grove observed inside its time
  window, plus derived metrics (duration, query count, side-effect counts).
  Grove sits in front of every request and captures mail centrally, so it does
  this with zero app instrumentation.
- **`grove sql-capture on|off|status`.** Turns on MySQL's general query log
  (written to a Grove-owned file) so each request's chain includes the SQL it
  issued, correlated by time window — because Grove owns the database service.
- **Desktop app.** The Requests panel expands each request to show its causal
  chain (SQL + mail + metrics), with a toolbar toggle for SQL capture.

### Added — "explain this error"

- **`grove explain <id>`** and the **`grove_explain`** MCP tool curate a
  debugging bundle for one request — the request (headers + body), its causal
  chain (SQL + mail + metrics), and the matching error-log entries with
  stacktraces — gathered in one place and structured for an AI assistant. Logs
  are chased only for failures, and the absence of a stacktrace is handled
  gracefully. In the desktop app, an **✨ Explain** button on each request copies
  the bundle to the clipboard. See [docs/MCP.md](docs/MCP.md).

## [1.0.0] — 2026-07-11

Grove 1.0. A native, zero-dependency local dev environment for macOS: `*.test`
sites with trusted HTTPS, bundled multi-version PHP/Node, databases, mail,
tunnels, a request timeline with replay, a webhook hub, a database client,
reproducible environment bundles, and end-to-end encrypted team secret sync —
with the entire core free and open source.

### Added

- **AI tools (MCP server).** `grove mcp` runs a Model Context Protocol server
  that exposes your local environment — sites, request timeline, webhooks, logs,
  and database schema/queries — to AI clients like Claude and Cursor. Read-only
  and local-only; point your client at `grove mcp` and ask it about what's
  actually running. See [docs/MCP.md](docs/MCP.md).

## [0.13.1] — 2026-07-11

### Changed

- The in-app logo (header, About dialog, and splash screen) now matches the new
  **lime** app icon.

## [0.13.0] — 2026-07-11

### Added

- **Local webhook hub.** Any request to `/__grove/hooks/<bucket>` on a site is
  captured and acknowledged with `200` — a local webhook.site. Expose it with
  `grove share <site>`, point Stripe/GitHub at it, then inspect each delivery
  (headers + payload) and **re-deliver it to your app** while you fix the handler.
  New **Webhooks** panel in the app; `grove hooks` on the CLI.
- **Turn a request into a test.** From any captured request or webhook, copy it
  as a `curl` command, a `.http` file, or a **Pest feature test** (`grove request
  <id> --as pest`, or the buttons in the app) — turn a failing request into a
  regression test in one click.

### Changed

- Fresh **lime** app icon.

## [0.12.0] — 2026-07-11

### Added

- **Request inspection & replay.** The request timeline now lets you expand any
  request to see its headers and body, and **replay it** with one click (or
  `grove replay <id>`) — a framework-agnostic way to re-run a failed request while
  you fix the code. Works for any site, any framework, zero setup.
- **Reproducible environment bundles.** `grove bundle export` packages a
  project's `grove.toml`, `.env`, and database into one shareable file;
  `grove bundle import` unpacks it, brings the environment up, and loads the
  database — reproducible dev environments without Docker. Great for onboarding.
- **SQL syntax highlighting** in the database client's query editor.

## [0.11.1] — 2026-07-11

### Changed

- Internal packaging change for how Grove Pro features are built. No user-facing
  changes — the free core and Pro features behave exactly as in 0.11.0.

## [0.11.0] — 2026-07-08

### Added

- **Built-in database client.** A new **Database** panel browses and queries your
  sites' databases — auto-connected from each project's `.env`, with no
  connection details to enter. Browsing tables and running `SELECT` queries is
  free; **Grove Pro** adds inline row editing, a schema inspector (columns,
  indexes, foreign keys), and a production-safety guard. See the
  [Pro & Teams guide](docs/PRO.md).

## [0.10.0] — 2026-07-08

### Added

- **Team secret sync (Grove Teams).** Share a project's `.env` with your team
  securely — encrypted end-to-end, so it never gets pasted into a chat window.
  Manage access with `grove secret`. Requires a Grove Teams license; see the
  [Pro & Teams guide](docs/PRO.md).

## [0.9.0] — 2026-07-07

### Added

- **License activation for Grove Pro / Teams.** Activate a purchased license key
  and Grove verifies it **offline** (Ed25519, `grove-license`) against a baked-in
  public key — no network call needed, so entitlements keep working without a
  connection.
    - `grove license activate <key>` / `grove license status` / `grove license
      deactivate`.
    - A **License** section in the desktop app's Settings (activate, status,
      remove).
    - Entitlement gates (`require_pro` / `require_teams`) that Pro/Teams features
      check; the free, open-source core is never gated.

## [0.8.1] — 2026-07-07

### Fixed

- **`grove path install` no longer fails with a permission error.** The shims
  now live under `~/.grove/bin` (a user-owned directory) instead of under the
  root-owned `$GROVE_HOME`, so the command works when Grove runs as a root
  LaunchDaemon. Add `~/.grove/bin` to your PATH (the command prints the line).

## [0.8.0] — 2026-07-07

### Added

- **`grove.toml` + `grove up` — reproducible project environments.** Commit a
  small `grove.toml` describing what a project needs (PHP/Node versions, bundled
  services, HTTPS, dev processes); a teammate then goes from `git clone` to a
  running, identical setup with a single command:
    - `grove up` links the project, pins its PHP/Node, ensures its services are
      installed + running, and (optionally) starts its dev processes.
    - `grove up --write` scaffolds a friendly, commented starter `grove.toml`.
    - `grove up [path]` targets a directory other than the cwd; `--no-dev`
      skips dev processes.
  It orchestrates the same daemon operations you'd run by hand, in one step.

## [0.7.0] — 2026-07-07

### Added

- **Request timeline.** Grove sits in front of every `*.test` site, so it now
  records a live, framework-agnostic timeline of the requests it proxies —
  method, path, status, and duration — with zero configuration and no per-app
  instrumentation.
    - New **Requests** panel in the desktop app: a live-updating table with
      status colour-coding, slow-request highlighting, a per-site filter, and
      shown/avg-ms/error-rate stats.
    - `grove requests [site] [--limit N]` on the CLI (`--json` supported).
  Captured at the proxy layer into a bounded in-memory ring buffer (last 500),
  so it costs nothing at rest and never grows unbounded.

## [0.6.0] — 2026-07-07

### Added

- **`grove path` — the bundled toolchain on your PATH.** Installs shims for
  `php`, `composer`, `node`, `npm`, `npx` and `laravel` that resolve to whatever
  version each project pins (via `grove isolate` / `grove node use`), falling
  back to the defaults — zero-config per-directory version switching, so you can
  finally drop Herd/Valet entirely. Runtimes are provisioned by the (root)
  daemon so the shims only ever read them.
    - `grove path install` / `grove path show` / `grove path uninstall`.
- **`grove db` — database time-travel.** Point-in-time snapshots of Grove's
  bundled MySQL / PostgreSQL so you can experiment (or migrate) without fear:
    - `grove db snapshot [--engine mysql|postgres] [--db NAME] [--note TEXT]`
    - `grove db list`, `grove db restore <id>`, `grove db rm <id>`.
  Snapshots are plain SQL dumps under `$GROVE_HOME/snapshots/` with a JSON index.

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

[1.4.2]: https://github.com/kwhorne/grove/releases/tag/v1.4.2
[1.4.1]: https://github.com/kwhorne/grove/releases/tag/v1.4.1
[1.4.0]: https://github.com/kwhorne/grove/releases/tag/v1.4.0
[1.3.2]: https://github.com/kwhorne/grove/releases/tag/v1.3.2
[1.3.1]: https://github.com/kwhorne/grove/releases/tag/v1.3.1
[1.3.0]: https://github.com/kwhorne/grove/releases/tag/v1.3.0
[1.2.1]: https://github.com/kwhorne/grove/releases/tag/v1.2.1
[1.1.0]: https://github.com/kwhorne/grove/releases/tag/v1.1.0
[1.0.0]: https://github.com/kwhorne/grove/releases/tag/v1.0.0
[0.13.1]: https://github.com/kwhorne/grove/releases/tag/v0.13.1
[0.13.0]: https://github.com/kwhorne/grove/releases/tag/v0.13.0
[0.12.0]: https://github.com/kwhorne/grove/releases/tag/v0.12.0
[0.11.1]: https://github.com/kwhorne/grove/releases/tag/v0.11.1
[0.11.0]: https://github.com/kwhorne/grove/releases/tag/v0.11.0
[0.10.0]: https://github.com/kwhorne/grove/releases/tag/v0.10.0
[0.9.0]: https://github.com/kwhorne/grove/releases/tag/v0.9.0
[0.8.1]: https://github.com/kwhorne/grove/releases/tag/v0.8.1
[0.8.0]: https://github.com/kwhorne/grove/releases/tag/v0.8.0
[0.7.0]: https://github.com/kwhorne/grove/releases/tag/v0.7.0
[0.6.0]: https://github.com/kwhorne/grove/releases/tag/v0.6.0
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
