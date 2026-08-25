# Configuration

Grove's single source of truth is `config.toml` under `$GROVE_HOME`
(default `~/Library/Application Support/Grove/config.toml` on macOS). Anything
that can be re-derived (issued certs, hot FPM pools, installed runtimes) is kept
out of it, so the file stays human-readable and diff-friendly.

Set `GROVE_HOME` to use an isolated tree (handy for testing on high ports).

## Example

```toml
[general]
tld = "test"            # sites are served on *.test
default_php = "8.5"     # used by sites without an explicit isolate
php_variant = "grove"   # extension set: "grove" (union), "common" or "bulk"
auto_start = true       # launch the daemon at login
http_port = 80
https_port = 443
dns_port = 53
docker = true           # auto-discover Docker/OrbStack containers
xdebug = false          # load Xdebug into FPM pools (trigger mode)
xdebug_port = 9003      # DBGp port the debugger listens on

[services]
mail_enabled = true     # run the built-in SMTP mail-catcher
mail_port = 1025

[tunnel]
server = "grove.elyracode.com:7000"  # default; used by `grove share`
# token = "secret"                    # only if your server requires one

# Sites removed from the list with `grove forget` (files kept)
ignored = []

# Every immediate subdirectory of a parked path becomes <name>.test
[[parked]]
path = "~/Code"

# Explicit sites override parked discovery on name collision
[[sites]]
name = "inside-next"
path = "~/Code/inside-next"
php = "8.5"             # per-site PHP (isolate)
node = "22"            # per-site Node version
secure = true          # HTTPS
driver = "laravel"     # optional; auto-detected otherwise

[[sites]]
name = "frontend"
path = "~/Code/frontend"
driver = "proxy"
proxy_to = "http://127.0.0.1:5173"
```

## Fields

### `[general]`

| Key | Default | Notes |
| --- | --- | --- |
| `tld` | `test` | Top-level domain. Changing it requires a daemon restart — and `sudo grove ca rotate`, since the root CA is name-constrained to the TLD it was generated for and cannot sign the new one. `grove doctor` flags the mismatch. |
| `default_php` | `8.5` | Fallback PHP version for sites without `php`. |
| `php_variant` | `grove` | Which static-PHP extension set `grove php install` fetches: `grove` (Grove's own build — has both the PDO SQLite/PostgreSQL drivers *and* `intl`/`mysqli`/`sodium`/`readline`/`apcu`/`xsl`), or upstream `common` / `bulk`, each missing one of those groups. See [COMMANDS.md](COMMANDS.md#extensions-and-the---variant-flag). Existing builds are not re-downloaded when this changes. |
| `auto_start` | `true` | Start the daemon at login. |
| `http_port` | `80` | Use a high port (e.g. `8080`) to run without elevation. |
| `https_port` | `443` | — |
| `dns_port` | `53` | — |
| `docker` | `true` | Auto-discover Docker/OrbStack containers as `<name>.test`. |
| `xdebug` | `false` | Load Xdebug into FPM pools (see [DEBUGGING.md](DEBUGGING.md)). |
| `xdebug_port` | `9003` | DBGp port the debugger/adapter listens on. |

### `[services]`

| Key | Default | Notes |
| --- | --- | --- |
| `mail_enabled` | `true` | Run the SMTP mail-catcher. |
| `mail_port` | `1025` | SMTP port apps connect to. |

### `[tunnel]`

| Key | Default | Notes |
| --- | --- | --- |
| `server` | `grove.elyracode.com:7000` | Tunnel server `grove share` connects to. |
| `token` | — | Shared secret, only if your server requires one. |

### `ignored`

A list of site names hidden with `grove forget` (their files are kept). Restore
with `grove restore <name>`.

### `[[parked]]`

A list of directories; each immediate subdirectory becomes a site. Paths
support `~` and environment variables.

### `[[sites]]`

| Key | Notes |
| --- | --- |
| `name` | Site name → `<name>.<tld>`. |
| `path` | Project directory (omit for `proxy`). |
| `php` | Per-site PHP version override. |
| `node` | Per-site Node version. |
| `secure` | Enable HTTPS. |
| `driver` | `laravel` \| `wordpress` \| `php` \| `static` \| `proxy` (auto-detected if omitted). |
| `proxy_to` | Upstream URL for the `proxy` driver. |

> Tip: most changes are easiest via the CLI (`grove secure`, `grove isolate`,
> `grove node use`, …) or the GUI Settings panel (⌘,), which write this file for
> you and reload the daemon atomically.

## Environment variables

| Variable | Purpose |
| --- | --- |
| `GROVE_HOME` | Base directory for all state (default `~/Library/Application Support/Grove`). |
| `GROVE_PHP_MIRROR` | Base URL for static PHP archives, for all variants including `grove` (default: Grove's GitHub release for `grove`, `https://dl.static-php.dev/static-php-cli` for the upstream sets). Must keep static-php-cli's `<variant>/php-<version>-<cli\|fpm>-<os>-<arch>.tar.gz` layout. |
| `GROVE_TEAMS_SERVER` | Grove Teams backend URL (default `https://teams.elyracode.com`). |
| `GROVE_LOG` | Log filter for the daemon (e.g. `info`, `debug`). |

## Other on-disk files

Beyond `config.toml`, Grove keeps a few files outside the config:

| Path | What |
| --- | --- |
| `$GROVE_HOME/license.key` | The activated Pro/Teams license (via `grove license activate`). |
| `~/.grove/bin/` | PATH shims created by `grove path install`. |
| `~/.grove/identity` | Your Grove Teams member key pair (private — never leaves the machine). |
| `$GROVE_HOME/snapshots/` | Database snapshots (`grove db`). |
| `$GROVE_HOME/certs/` | Root CA + issued leaf certificates. The CA **private key** is `0600` and owned by root once a root daemon has seen it — it signs certificates the system trust store believes, and nothing unprivileged needs it. The certificate stays world-readable, and leaf keys stay user-readable so `grove dev` can hand them to Vite. |
| `~/.grove/bin/` | PATH shims from `grove path install` (`php`, `composer`, `cpx`, `node`, …). |
| `~/.grove/cpx.phar` | The bundled cpx binary. In your home, not `$GROVE_HOME`, so `cpx self-update` can replace it. |
