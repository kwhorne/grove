# Architecture

Grove is a Cargo workspace. A single long-running **daemon** binds the
privileged ports (DNS 53, HTTP 80, HTTPS 443) and supervises runtimes and
services. The **CLI** and **GUI** are thin clients that drive the daemon over a
local Unix-socket JSON-RPC.

```
                 ┌───────────┐        ┌───────────┐
                 │  grove-cli│        │  grove-gui│   (Tauri 2 + Svelte 5)
                 └─────┬─────┘        └─────┬─────┘
                       │   JSON-RPC (grove-ipc) over Unix socket
                       └───────────┬────────┘
                              ┌─────▼──────┐
                              │ grove-daemon│  binds 53/80/443, serves IPC
                              └─────┬──────┘
        ┌───────────────┬──────────┼───────────┬───────────────┐
   ┌────▼────┐    ┌─────▼────┐ ┌───▼────┐  ┌───▼─────┐    ┌────▼─────┐
   │grove-dns│    │grove-proxy│ │grove-  │  │grove-   │    │grove-os  │
   │ (*.test)│    │ + FastCGI │ │runtime │  │services │    │resolver/ │
   └─────────┘    └───────────┘ │PHP/Node│  │DB/Redis │    │trust/svc │
                                └────────┘  │+ mail   │    └──────────┘
                                            └─────────┘
                          ┌───────────┐
                          │ grove-core│  config, site registry, drivers (pure)
                          └───────────┘
```

## Crates

| Crate | Responsibility |
| --- | --- |
| `grove-core` | Site registry, driver detection, TOML config, paths. Pure — no OS I/O or port binding. |
| `grove-ipc` | JSON-RPC protocol types + newline-delimited transport, and the client used by CLI/GUI. |
| `grove-tls` | Root CA generation + on-demand leaf issuance (rcgen/rustls). |
| `grove-dns` | Embedded authoritative resolver for `*.<tld>` (hickory). |
| `grove-proxy` | HTTP/HTTPS listeners, per-driver dispatch, SNI cert resolution, and a minimal FastCGI client. |
| `grove-runtime` | PHP version management + lazy FPM pools; Node version management; project scaffolding; the bundled toolchain (Composer, Laravel installer) exposed by `grove path`. |
| `grove-services` | Bundled service manager (PostgreSQL/MySQL/Redis) + the SMTP mail-catcher + cross-dialect database conversion + point-in-time database snapshots. |
| `grove-tunnel` | Native public tunnels: `grove share` client + the self-hostable `grove-tunnel` server (yamux + hyper). |
| `grove-license` | Offline Ed25519 verification of Grove Pro/Teams license keys against a baked-in public key. |
| `grove-secrets` | End-to-end encrypted team secrets (age/X25519): identities, `EnvSecrets`, `SecretStore` (file mock + HTTP), `SecretsClient`. |
| `grove-os` | Platform integration: resolver setup, trust store, OS service install, elevation checks. |
| `grove-daemon` | The long-running process: boots listeners, supervises runtimes/services, serves IPC. |
| `grove-cli` | clap frontend (binary `grove`). |
| `grove-gui` | Tauri 2 + Svelte 5 desktop app + macOS menu-bar icon. Hosts the Pro database client (reuses the `e-db` engine). |

## Request flow

1. A browser requests `https://myapp.test`.
2. The OS resolver (configured by `grove-os`) sends `*.test` to `grove-dns`,
   which answers loopback.
3. The request hits `grove-proxy` on 443. SNI selects/issues a leaf cert from
   the local CA. The `Host` header is matched against the site registry.
4. The site's driver decides handling: PHP → FastCGI to a lazily-started
   FPM pool for the site's PHP version; static → serve files; proxy → forward
   to the upstream dev server.

### Bodies are streamed, not buffered

Neither direction is held whole in memory:

- **Responses** are forwarded as they arrive. Grove returns the headers as soon
  as PHP flushes them and turns each FastCGI record into an HTTP chunk, so
  Server-Sent Events and other long-lived streams work, and a large download
  costs no memory. PHP sets no `Content-Length` on a streamed response, so hyper
  selects `Transfer-Encoding: chunked`.
- **Request bodies** are streamed into `STDIN` when the length is known. Because
  CGI must be told `CONTENT_LENGTH` before the body, a chunked request — which
  declares no length — is measured first: kept in memory up to 1 MiB, and spilled
  to a private `0600` spool file beyond that, removed as soon as the body is
  dropped. Bodies beyond 2 GiB are refused with `413`.
- Reads and writes on the FastCGI connection proceed concurrently. Writing a
  whole body before reading any response would deadlock on a large upload that
  PHP rejects early.

The request timeline stores up to 1 MiB of a request body and flags the entry as
truncated beyond that, so `grove replay` and the curl / `.http` / Pest export see
smaller bodies in full.

### What the PHP drivers serve

For a PHP site, an existing file under the document root is served directly (so
built assets under `/build/` do not go through PHP), with two rules:

- **`.php` and `.phtml` files are executed, never served.** Handing them back as
  text would disclose source, and WordPress addresses scripts directly —
  `wp-login.php`, `wp-admin/*.php`. The extension is matched case-insensitively,
  since a case-insensitive filesystem resolves `/INDEX.PHP` to the same file.
- **Dot-prefixed paths are refused with 404.** A plain PHP project's document
  root is the project root, so `/.env` would otherwise be readable. `.well-known/`
  is exempt so ACME HTTP-01 challenges keep working.

Anything else falls through to the site's front controller, which receives the
request path as `PATH_INFO`.

### What is not repeated per request

A local dev proxy is asked the same questions over and over — the same assets on
every reload, the same hostname on every connection — so the request path is
built to answer them cheaply the second time:

- **Upstream connections are pooled.** One shared `hyper` client serves the proxy
  driver and replay. A client *is* the connection pool, so constructing one per
  request meant a fresh TCP handshake per request — and a Vite dev server saw one
  connection per asset instead of a few kept alive.
- **Static responses carry a validator.** An `ETag` is derived from the file's
  size and mtime, which a `stat` already provides, so no extra read is needed;
  `If-None-Match` is answered with `304`. `Cache-Control: no-cache` means the
  browser always asks — an edit is never served stale — but an unchanged asset
  costs a round trip rather than a re-transfer. Files over 256 KiB stream from
  disk instead of being read into memory whole.
- **DNS answers are cacheable** (`TTL 300`). The answer is always loopback and
  never changes; a `TTL 0` forbade caching, which put the system resolver — and
  on macOS `mDNSResponder` — in the path to the first byte of every connection.
- **Filesystem checks are async.** The existence checks on the PHP and static
  paths run on every request; issued as blocking syscalls from the request task
  they would stall a runtime worker on a slow volume, such as a network share or
  a Docker bind mount. Starting an FPM pool — which forks php-fpm and waits for
  its socket — runs on the blocking pool for the same reason.

### Staying up

The daemon serves every site on the machine, so a failure in one request must not
be able to end the process:

- **Panics unwind.** The release profile deliberately does not set
  `panic = "abort"`: with it, one panic anywhere takes down DNS, TLS and every
  site at once. Unwinding keeps the failure inside the tokio task that caused it.
- **The accept loop backs off** (5 ms to 1 s) instead of retrying immediately.
  Out of file descriptors, `accept` fails instantly and forever, so a bare retry
  is a busy loop that burns a core and never recovers.
- **Silent connections are bounded.** A TLS handshake has 10 s and the request
  headers 30 s; without a deadline, a peer that connects and says nothing holds a
  task and a descriptor indefinitely.

## Beyond native sites

- **Docker / OrbStack** — `grove-daemon` polls the Docker socket and merges
  running containers into the site registry as `proxy` sites (label- or
  compose-based). They get the same trusted HTTPS + dashboard, and can be
  started/stopped over IPC. See [DOCKER.md](DOCKER.md).
- **Public tunnels** — `grove share` (in `grove-tunnel`) proxies a local
  `*.test` site — native *or* container-backed — to a public tunnel server over
  a yamux-multiplexed connection. See [TUNNEL.md](TUNNEL.md).
- **Xdebug** — when enabled, FPM pools are respawned with `-d` Xdebug INI
  overrides (trigger mode). See [DEBUGGING.md](DEBUGGING.md).
- **Toolchain on PATH** — `grove path` writes read-only shims that resolve each
  project's pinned `php`/`node`/`composer` version and `exec` it. Runtimes are
  provisioned by the (root) daemon (`ProvisionToolchain`) into the shared
  `runtimes/` dir, so the user-run shims never need write access. The shims
  themselves live in `~/.grove/bin` (user-owned, added to PATH).
- **Database client** — the GUI's Database panel reuses the `e-db` engine to
  browse/edit databases, auto-discovering connections from each site's `.env`.
  Free tier is read-only; editing + schema inspection are gated behind an active
  Pro license (client-side, since it's a local feature). See [DATABASE.md](DATABASE.md).
- **Database snapshots** — `grove db` dumps/restores the bundled MySQL /
  PostgreSQL via their own client tools, indexed under `snapshots/`.
- **Reproducible environments** — `grove up` reads a project's committed
  `grove.toml` (`grove-core::ProjectFile`) and orchestrates the existing daemon
  operations (link, isolate, node pin, service install/start, dev start) so a
  fresh clone comes up identically in one command.
- **Request timeline** — the proxy handler records every request (method, path,
  status, duration) into a bounded in-memory ring buffer in `grove-core`
  (`RequestLog`), shared with the daemon so `grove requests` and the GUI panel
  can read it. Framework-agnostic; nothing is persisted to disk.
- **Local HTTPS** — `grove-tls` keeps the root CA that was generated on first run
  and trusted once, and signs per-site leaves from it on demand. The certificate
  it reports is the one on disk, which is the one the OS trust store was pointed
  at; leaves are short-lived (397 days) and renewed by the daemon.

## Licensing & Teams (Grove Pro)

The free core is never gated; Pro/Teams features sit behind an entitlement.

- **License keys** are Ed25519-signed by the store (elyracode.com) and verified
  **offline** by `grove-license` against a baked-in public key. `grove license
  activate` stores the key at `$GROVE_HOME/license.key` (written by the root
  daemon); the daemon exposes `require_pro` / `require_teams` gates.
- **Team secrets** (`grove secret`) are encrypted **client-side** (`grove-secrets`,
  age/X25519) to the current members' public keys. `HttpStore` talks to the
  hosted, zero-knowledge backend, which stores only ciphertext + public keys and
  **independently** verifies the license + enforces seats (real enforcement is
  server-side, so the open client is safe to inspect). Your member identity lives
  at `~/.grove/identity`. See [PRO.md](PRO.md).

## Zero external dependencies

DNS, the reverse proxy, FastCGI and TLS are built into the Rust core (no
dnsmasq, nginx or OpenSSL). PHP, Node and the databases are downloaded as
self-contained binaries into `$GROVE_HOME`. The only host requirement for
scaffolding Redis from source / new Laravel projects is a C toolchain and
network access, which dev machines already have.

## State on disk

Everything lives under one base directory (`$GROVE_HOME`, or the platform
default such as `~/Library/Application Support/Grove`):

```
config.toml            declarative source of truth
certs/                 root CA + issued leaf certs (incl. certs/dev for Vite HTTPS)
runtimes/              PHP/Node builds, FPM configs, php-builds.json, composer.phar
services/              bundled DB/cache binaries + data dirs + state.json
snapshots/             database snapshots (SQL dumps) + index.json
logs/                  per-service logs
run/                   daemon IPC socket, pidfile, FPM/service sockets
```
