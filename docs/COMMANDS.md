# CLI reference

Every command speaks to the daemon over local IPC and accepts a global
`--json` flag for machine-readable output.

```
grove <command> [args] [--json]
```

## Lifecycle

| Command | Description |
| --- | --- |
| `grove init [--php 8.4] [--no-php]` | First-run setup: config, root CA, a PHP build, and (when elevated) the resolver + CA trust. Parks `~/Code` by default. |
| `grove daemon` | Run the daemon in the foreground (used by the service manager). |
| `grove start` | Start the daemon in the background. |
| `grove stop` | Stop the daemon gracefully. |
| `grove restart` | Restart the daemon. |
| `grove gui` | Start the daemon if needed and launch the desktop GUI. |
| `grove install` / `uninstall` | Install/remove Grove as an OS service (launchd/systemd); `uninstall` also removes the resolver + CA trust. |
| `grove status` | Daemon + environment status. |
| `grove doctor` | Diagnostics. |

## Sites

| Command | Description |
| --- | --- |
| `grove new <name> [--kind laravel\|static] [--path ~/Code] [--php 8.4] [--git]` | Scaffold a new project and link it. |
| `grove park [<dir>]` | Park a directory — every subfolder becomes `<name>.<tld>`. |
| `grove unpark [<dir>]` | Stop parking a directory. |
| `grove link [name] [--path <dir>]` | Link a single directory as a site. |
| `grove unlink <name>` | Remove a linked site. |
| `grove list` (alias `links`) | List every served site. |
| `grove secure <name>` / `unsecure <name>` | Toggle HTTPS for a site. |
| `grove isolate <name> <version>` / `unisolate <name>` | Pin / clear a site's PHP version. |
| `grove proxy <name> <url>` | Route `<name>.<tld>` to a running dev server. |
| `grove import` | Import parked dirs + linked sites from an existing Laravel Valet config. |

## PHP

| Command | Description |
| --- | --- |
| `grove php install <version>` | Download a self-contained static PHP-FPM build (e.g. `8.5`, `8.4`, `8.3`). |
| `grove php register <version> <fpm-binary>` | Register a custom php-fpm binary (bring-your-own). |
| `grove php discover` | Auto-discover php-fpm binaries on this machine. |
| `grove php list` | List registered PHP builds. |
| `grove use <version>` | Set the global default PHP version. |

## Node.js

| Command | Description |
| --- | --- |
| `grove node install <version>` | Download a Node.js build (major like `22`, or exact `22.23.1`). |
| `grove node list` | List installed + installable Node versions. |
| `grove node use <site> <version>` / `unuse <site>` | Pin / clear a site's Node version. |

## Services

| Command | Description |
| --- | --- |
| `grove service list` | List bundled services and their state. |
| `grove service install <key>` | Download + initialise a service (`postgres`, `mysql`, `redis`). |
| `grove service start\|stop\|restart <key>` | Control a service. |
| `grove service port <key> <port>` | Override a service's listen port. |
| `grove env [site]` | Print a `.env` snippet for the bundled services. |

## Mail

| Command | Description |
| --- | --- |
| `grove mail` | List captured emails. |
| `grove mail show <id>` | Show one captured email. |
| `grove mail clear` | Discard all captured emails. |

## Logs

| Command | Description |
| --- | --- |
| `grove logs` | List available log sources (per-site Laravel logs + Grove service logs). |
| `grove logs <site> [--lines 100]` | View recent entries from a source. |

## TLS / CA

| Command | Description |
| --- | --- |
| `grove ca trust` | Generate (if needed) and trust the Grove root CA in the system store. |
| `grove ca uninstall` | Remove the Grove root CA from the system store. |
