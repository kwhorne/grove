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
default_php = "8.4"     # used by sites without an explicit isolate
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
php = "8.4"             # per-site PHP (isolate)
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
| `tld` | `test` | Top-level domain. Changing it requires a daemon restart. |
| `default_php` | `8.4` | Fallback PHP version for sites without `php`. |
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
| `$GROVE_HOME/certs/` | Root CA + issued leaf certificates. |
