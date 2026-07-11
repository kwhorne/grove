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

## Reproducible environments (`grove.toml`)

Commit a `grove.toml` to a project so a teammate can go from `git clone` to a
running, identical environment in one command.

| Command | Description |
| --- | --- |
| `grove up` | Bring the current project up from its `grove.toml` (link, pin PHP/Node, start services, optional dev). |
| `grove up <path>` | Target a different project directory. |
| `grove up --write` | Scaffold a starter `grove.toml` for the current project. |
| `grove up --no-dev` | Bring up but skip starting dev processes. |

```toml
# grove.toml
name = "myapp"
php = "8.4"
node = "22"
secure = true
services = ["mysql", "redis"]
dev = true
```

## Reproducible bundles

Package a whole project environment — `grove.toml`, `.env`, and its database —
into one shareable file, and restore it with a single command. Reproducible dev
environments without Docker; ideal for onboarding a teammate.

| Command | Description |
| --- | --- |
| `grove bundle export` | Bundle the current project into `<name>.grovebundle`. |
| `grove bundle export <path> --out <file>` | Choose the project and output file. |
| `grove bundle export --no-env` | Exclude the project's `.env` (secrets). |
| `grove bundle import <file>` | Unpack, bring the environment up, and load the database. |
| `grove bundle import <file> --into <dir>` | Restore into a specific directory. |

## Team secrets (Grove Teams)

End-to-end encrypted `.env` sync for your team. Secrets are encrypted on your
machine (age / X25519) to members' public keys; the backend only stores
ciphertext. Requires an active Teams license (`grove license activate`).

| Command | Description |
| --- | --- |
| `grove secret set <project> KEY=VALUE` | Encrypt + push a secret. |
| `grove secret pull <project> [--write]` | Fetch + decrypt (optionally write `.env`). |
| `grove secret share <project> <public-key>` | Grant a teammate access + re-encrypt. |
| `grove secret revoke <project> <public-key>` | Remove a teammate + re-encrypt. |
| `grove secret members <project>` | List members with access. |
| `grove secret whoami` | Print your member public key. |

The backend URL defaults to `https://teams.elyracode.com` (`GROVE_TEAMS_SERVER`
overrides it).

## License (Grove Pro / Teams)

Activate a purchased license to unlock Pro/Teams features. Verified offline
(Ed25519) — no connection required. The free, open-source core is never gated.

| Command | Description |
| --- | --- |
| `grove license activate <key>` | Activate a license key (from your purchase email). |
| `grove license status` | Show the current entitlement (plan, seats, renewal). |
| `grove license deactivate` | Remove the stored license. |

Also available in the desktop app under **Settings → License**.

## Request timeline

Grove proxies every `*.test` site, so it records a live, framework-agnostic log
of recent requests — method, path, status, duration — with zero setup. Also shown
in the desktop app's **Requests** panel.

| Command | Description |
| --- | --- |
| `grove requests` | Recent requests across all sites (newest first), with ids. |
| `grove requests <site>` | Filter to one site. |
| `grove requests --limit <n>` | Cap the number of entries. |
| `grove replay <id>` | Re-issue a captured request through Grove (id from `grove requests`). |
| `grove request <id> --as <fmt>` | Print the request as `curl`, `http`, or `pest`. |

In the desktop app, click any request to see its headers and body, replay it, or
copy it as a curl command, a `.http` file, or a Pest test — a framework-agnostic
way to re-run a failed request (or turn it into a regression test) while you fix
the code.

## AI tools (MCP)

Expose your local environment to AI clients (Claude, Cursor) over the Model
Context Protocol — read-only, local-only. See [MCP.md](MCP.md) for client setup.

| Command | Description |
| --- | --- |
| `grove mcp` | Run the MCP server over stdio (your AI client launches this for you). |

## Webhooks

Grove captures any request to `/__grove/hooks/<bucket>` on a site and answers
`200` — a local webhook.site. Expose it publicly with `grove share <site>` and
point Stripe, GitHub, etc. at `https://<public-url>/__grove/hooks/<bucket>`.
Inspect each delivery and **re-deliver it** to your app while you fix the handler.

| Command | Description |
| --- | --- |
| `grove hooks` | List captured webhooks (newest first), with ids. |
| `grove hooks replay <id> --to <url>` | Re-deliver a webhook to a local handler. |
| `grove hooks clear` | Drop all captured webhooks. |

Also available as the **Webhooks** panel in the desktop app, where you can
inspect payloads and copy any delivery as a curl/`.http`/Pest test.

## Database snapshots

> Looking to **browse or edit** data? That's the **Database** panel in the
> desktop app (auto-connects from each site's `.env`) — see [DATABASE.md](DATABASE.md).
> The commands below are for point-in-time **snapshots**.

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
