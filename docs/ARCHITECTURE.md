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
| `grove-runtime` | PHP version management + lazy FPM pools; Node version management; project scaffolding. |
| `grove-services` | Bundled service manager (PostgreSQL/MySQL/Redis) + the SMTP mail-catcher + cross-dialect database conversion. |
| `grove-tunnel` | Native public tunnels: `grove share` client + the self-hostable `grove-tunnel` server (yamux + hyper). |
| `grove-os` | Platform integration: resolver setup, trust store, OS service install, elevation checks. |
| `grove-daemon` | The long-running process: boots listeners, supervises runtimes/services, serves IPC. |
| `grove-cli` | clap frontend (binary `grove`). |
| `grove-gui` | Tauri 2 + Svelte 5 desktop app + macOS menu-bar icon. |

## Request flow

1. A browser requests `https://myapp.test`.
2. The OS resolver (configured by `grove-os`) sends `*.test` to `grove-dns`,
   which answers loopback.
3. The request hits `grove-proxy` on 443. SNI selects/issues a leaf cert from
   the local CA. The `Host` header is matched against the site registry.
4. The site's driver decides handling: PHP → FastCGI to a lazily-started
   FPM pool for the site's PHP version; static → serve files; proxy → forward
   to the upstream dev server.

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
certs/                 root CA + issued leaf certs
runtimes/              PHP/Node builds, FPM configs, php-builds.json, composer.phar
services/              bundled DB/cache binaries + data dirs + state.json
logs/                  per-service logs
run/                   daemon IPC socket, pidfile, FPM/service sockets
```
