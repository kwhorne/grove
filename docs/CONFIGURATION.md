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

[services]
mail_enabled = true     # run the built-in SMTP mail-catcher
mail_port = 1025

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

### `[services]`

| Key | Default | Notes |
| --- | --- | --- |
| `mail_enabled` | `true` | Run the SMTP mail-catcher. |
| `mail_port` | `1025` | SMTP port apps connect to. |

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
