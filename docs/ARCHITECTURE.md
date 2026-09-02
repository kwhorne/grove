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
| `grove-core` | Site registry, driver detection, TOML config, paths — and the primitives the privileged parts need: `securefs` (mode-at-`open`, `O_NOFOLLOW`), `privdrop` (setuid/setgid for spawned children), `checksum` (SHA-256 verification), `redact` (stripping secrets out of logs). No port binding, no supervision. |
| `grove-ipc` | JSON-RPC protocol types + newline-delimited transport, and the client used by CLI/GUI. |
| `grove-tls` | Root CA generation, `NameConstraints`-scoped to the configured TLD, + on-demand leaf issuance and renewal (rcgen/rustls). |
| `grove-dns` | Embedded authoritative resolver for `*.<tld>` (hickory). |
| `grove-proxy` | HTTP/HTTPS listeners, per-driver dispatch, SNI cert resolution, and a minimal FastCGI client. |
| `grove-runtime` | PHP version management + lazy FPM pools; Node version management; project scaffolding; the bundled toolchain (Composer, Laravel installer) exposed by `grove path`. |
| `grove-services` | Bundled service manager (PostgreSQL/MySQL/Redis) + the SMTP mail-catcher + cross-dialect database conversion + point-in-time database snapshots. |
| `grove-tunnel` | Native public tunnels: `grove share` client + the self-hostable `grove-tunnel` server (yamux + hyper). |
| `grove-license` | Offline Ed25519 verification of Grove Pro/Teams license keys against a baked-in public key. |
| `grove-secrets` | End-to-end encrypted team secrets (age/X25519): identities, `EnvSecrets`, `SecretStore` (file mock + HTTP), `SecretsClient`, and the local recipient/version pins that keep the backend from choosing who can decrypt. |
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

### Protocols and upgrades

Both listeners speak HTTP/1.1 and HTTP/2, chosen per connection — by ALPN on
the TLS listener, by preface sniffing on the plain one. h2 requests carry the
host as `:authority` rather than a `Host` header; the handler reads either and
puts a `Host` into the header map so FastCGI's `HTTP_HOST`, the proxy driver
and the timeline all see it.

A request that asks to switch protocols (`Upgrade: websocket` with
`Connection: upgrade`) on a proxy-driver site bypasses the pooled upstream
client, which cannot hand a connection back: it gets a dedicated HTTP/1.1
connection to the upstream, and if that answers `101`, both sides' upgraded
streams are copied bidirectionally until either closes. That is what lets Vite
HMR on a proxy site, Next/Nuxt dev servers and Reverb over `wss://` work.

A secured site reached over plain HTTP is redirected to its HTTPS origin
(`301` for GET/HEAD, `308` otherwise), with the configured HTTPS port in the
`Location`.

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

Static files carry `Accept-Ranges: bytes` and answer single-range requests with
`206`, are gzipped when the client accepts it and the type compresses (with a
distinct `ETag` for that representation), and fall back to the root
`index.html` only for extension-less paths from clients that accept HTML — a
missing asset is a `404` that names the file, not the SPA shell as `text/html`.

Every error Grove generates itself — no site for the host, an upstream that
refused the connection, no PHP for the version, a request too large — is an
HTML page that states the situation and, where Grove knows it, the command
that fixes it.

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

## Trust boundaries

Grove's shape follows from one fact: a small part of it runs as **root** — it has
to, to bind 53/80/443, install the system resolver and add a CA to the trust
store — while everything it supervises runs as **you**. That line is where the
interesting failure modes live, so it is drawn explicitly.

### Root supervises, but nothing it starts stays root

The daemon spawns PHP-FPM pools, PostgreSQL, MySQL, Redis, `grove dev` servers,
and the scaffolding tools (Composer, the Laravel installer). Every one of them is
dropped to the invoking user before `exec`: `setgroups` (so root's
supplementary groups do not survive), then `setgid`, then `setuid` — in that
order, because the reverse leaves no privilege to drop the others. The identity
comes from `GROVE_RUN_USER_ID`/`GROVE_RUN_GROUP_ID` (written into the service
unit at install time), falling back to `GROVE_RUN_USER`/`SUDO_USER` resolved
through `id`. After dropping, the child verifies its own `geteuid`/`getegid`: a
*partial* drop fails the spawn rather than quietly running a database as root.

The same applies to Grove probing its own runtimes — `php -m`, `php -i` and
friends exec a binary out of `$GROVE_HOME`, so they go through the same drop.
(The mail-catcher needs none of this: it is an in-process Rust SMTP listener, not
a child process.)

### The IPC socket is the authorization boundary

Every privileged operation the daemon can perform is reachable through
`run/groved.sock`, which makes the socket — not any individual command — the
thing that has to be right:

- it is chowned to the run user and set to `0o660`, so mode alone excludes
  other local users;
- the daemon reads the peer's credentials (`SO_PEERCRED` / `LOCAL_PEERCRED`)
  **before it reads the request**, so an unauthorized caller's bytes are never
  parsed;
- an unauthorized peer gets an error *response* rather than a dropped
  connection, because a hangup is indistinguishable from a crashed daemon and
  sends people debugging the wrong thing.

Permitted: root (it can already do everything the daemon can), the daemon's own
uid, the configured run user, and the owner of `$GROVE_HOME`. Both of the last
two are needed, and neither alone would do: the owner covers a daemon someone
started by hand in their own tree, but after `sudo grove install` the owner *is*
root — so trusting only the owner would lock the user out of the daemon that
exists to serve them.

### Two trees, two owners

| Tree | Owner | Holds |
| --- | --- | --- |
| `$GROVE_HOME` | shared, root-writable | config, runtimes, services, certs, sockets |
| `~/.grove` | you | identity, secret pins, `cpx.phar`, PATH shims |

The split is load-bearing in both directions. Anything recording *your* trust
decisions stays in `~/.grove`, where the root daemon has no part in it — and
root never reads from there. Conversely, anything root *does* read out of
`$GROVE_HOME` is as privileged as root itself. `php-builds.json` is the sharp
example: it is a JSON file that names the `php-fpm` binary root will execute, so
being able to write it is being able to choose that binary. Replacing a binary is
the obvious attack; naming a different one is the cheaper one.

### Files are created with the mode they need

`grove-core::securefs` passes the mode to `open(2)` rather than `chmod`-ing
afterwards, so there is no window — however short — in which a freshly written
private key is world-readable. It also opens with `O_NOFOLLOW` and refuses a
symlink at the destination, so a planted link cannot redirect a privileged write
into a file of the attacker's choosing.

### What is not trusted from the wire

- **Downloads are verified before they are executed** — SHA-256, against a
  document from the same publisher. The exceptions, and what the check does and
  does not prove, are in [SECURITY.md](../SECURITY.md).
- **Forwarded headers are honoured only from loopback.** `X-Forwarded-Proto` and
  `X-Grove-Site` decide how a request is routed and whether it looks like HTTPS
  to the app, so off-loopback they are ignored rather than believed.
- **A client-supplied `Proxy:` header never becomes a CGI variable** — that is
  httpoxy (CVE-2016-5385): as `HTTP_PROXY` it would redirect the *application's*
  outbound HTTP through an attacker's host.
- **The tunnel server refuses to start open.** `grove-tunnel` is the one
  component meant to face the public internet, so it requires `--token` or an
  explicit `--allow-anonymous`; it rejects reserved subdomains, overwrites
  `X-Forwarded-For` with the real peer, and puts a timeout on header reads.

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
  can read it. Framework-agnostic; nothing is persisted to disk. What *leaves*
  the daemon is redacted — `Authorization`, `Cookie`, `Set-Cookie`, API-key
  headers, and query/body fields whose name looks like a credential are replaced
  with `[redacted]`, so `grove request --as curl`, the `.http`/Pest exports, the
  MCP tools and webhook payloads are all safe to paste. Replay is the deliberate
  exception: it reads the unredacted copy that never left the process, so a
  replayed request still authenticates.
- **Local HTTPS** — `grove-tls` keeps the root CA that was generated on first run
  and trusted once, and signs per-site leaves from it on demand. The certificate
  it reports is the one on disk, which is the one the OS trust store was pointed
  at. The CA carries an X.509 `NameConstraints` extension permitting only
  `.<tld>`, so a machine that trusts it cannot be served a valid certificate for
  `google.com` even if the CA key leaks — the TLD it was constrained to is
  recorded in `ca-meta.json`, and changing `tld` therefore requires
  `grove ca rotate`. Leaves last 397 days and are re-issued once they are within
  30 days of expiry, so a long-lived site does not silently serve an expired
  certificate.

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
certs/                 root CA + ca-meta.json + issued leaf certs (incl. certs/dev for Vite HTTPS)
runtimes/              PHP/Node builds, FPM configs, php-builds.json, composer.phar
services/              bundled DB/cache binaries + data dirs + state.json
snapshots/             database snapshots (SQL dumps) + index.json
logs/                  per-service logs
run/                   daemon IPC socket (groved.sock), pidfile, FPM/service sockets
```

`ca-meta.json` records the TLD the CA was name-constrained to; it is what lets
Grove notice that `tld` changed and tell you to `grove ca rotate` rather than
issue leaves the CA is not permitted to sign.

A second, **user-owned** tree holds the things root should have no part in —
see [Trust boundaries](#trust-boundaries):

```
~/.grove/identity      your age/X25519 secret-sync key pair (0600)
~/.grove/secrets/      per-project recipient + version pins
~/.grove/cpx.phar      the Composer Package Executor
~/.grove/bin/          PATH shims written by `grove path`
```
