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
| `grove new <name> [--kind <kit>] [--path ~/Code] [--php 8.4] [--git]` | Scaffold a new project via `laravel new` and link it. `--kind`: `laravel` (plain) \| `livewire` \| `react` \| `vue` \| a community kit `vendor/package` (`--using`) \| `static`. |
| `grove park [<dir>]` | Park a directory — every subfolder becomes `<name>.<tld>`. |
| `grove unpark [<dir>]` | Stop parking a directory. |
| `grove link [name] [--path <dir>]` | Link a single directory as a site. |
| `grove unlink <name>` | Remove a linked site. |
| `grove forget <name>` | Remove a site from the list without deleting its files. |
| `grove restore <name>` | Restore a previously forgotten site. |
| `grove list` (alias `links`) | List every served site. |
| `grove secure <name>` / `unsecure <name>` | Toggle HTTPS for a site. |
| `grove isolate <name> <version>` / `unisolate <name>` | Pin / clear a site's PHP version. |
| `grove proxy <name> <url>` | Route `<name>.<tld>` to a running dev server. |
| `grove share <name> [--subdomain s] [--server host:port] [--token t] [--basic-auth user:pass]` | Share a site publicly through a tunnel server (see [TUNNEL.md](TUNNEL.md)). |
| `grove import` | Import parked dirs + linked sites from an existing Laravel Valet config. |

## Dev processes

Grove runs a site's long-running dev processes for you — the **Vite dev server**
(`npm run dev`, HMR) and, for a non-`sync` queue, a **queue worker** — with the
site's own Node/PHP. No `artisan serve` needed (Grove already serves). Output
goes to the Logs panel (`dev-<site>-*.log`). In the GUI it's the ⚡ toggle per
site.

| Command | Description |
| --- | --- |
| `grove dev start <site>` | Start the site's dev processes (Vite + queue worker). |
| `grove dev stop <site>` | Stop them. |
| `grove dev list` | List sites with dev processes running. |

## Debugging (Xdebug)

| Command | Description |
| --- | --- |
| `grove debug on` / `off` | Load / unload Xdebug into FPM pools (trigger mode). |
| `grove debug status` | Show whether Xdebug is enabled and available per PHP build. |
| `grove debug env` | Print shell exports for debugging a CLI process: `eval "$(grove debug env)"`. |

See [DEBUGGING.md](DEBUGGING.md). Grove's fully-static PHP can't load Xdebug —
register a PHP that has it (`grove php register`).

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

## Request timeline

Grove proxies every `*.test` site, so it records a live, framework-agnostic log
of recent requests — method, path, status, duration — with zero setup. Also shown
in the desktop app's **Requests** panel.

| Command | Description |
| --- | --- |
| `grove requests` | Recent requests across all sites (newest first). |
| `grove requests <site>` | Filter to one site. |
| `grove requests --limit <n>` | Cap the number of entries. |

## Database snapshots

Point-in-time snapshots of Grove's bundled MySQL / PostgreSQL — snapshot before a
risky migration and roll back in one command. Stored as SQL under
`$GROVE_HOME/snapshots/`.

| Command | Description |
| --- | --- |
| `grove db snapshot [--engine mysql\|postgres] [--db NAME] [--note TEXT]` | Snapshot a database (MySQL: omit `--db` for all). |
| `grove db list` | List stored snapshots. |
| `grove db restore <id>` | Restore a snapshot by id. |
| `grove db rm <id>` | Delete a snapshot. |

## Toolchain on your PATH

Expose Grove's bundled `php`, `composer`, `node`, `npm`, `npx` and `laravel`,
auto-switching to whatever version each project pins (`grove isolate` /
`grove node use`) — so you can drop Herd/Valet entirely.

| Command | Description |
| --- | --- |
| `grove path install` | Create the shims + provision the toolchain, then print the PATH line to add. |
| `grove path show` | Show whether the shims are installed and on your PATH. |
| `grove path uninstall` | Remove the shims. |

## Docker / OrbStack

Running containers are discovered automatically and served as `<name>.test` with
trusted HTTPS — no command needed. They appear in `grove list` with the `proxy`
driver, and can be started/stopped from the GUI. Toggle with `[general].docker`.
See [DOCKER.md](DOCKER.md).

## GUI-only tools

The desktop app's **Tools** panel adds actions without a CLI equivalent:
**Restart daemon**, **Migrate MySQL from Herd**, **Convert database**
(MySQL/PostgreSQL/SQLite), and the **Xdebug** toggle.

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
