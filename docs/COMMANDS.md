# CLI reference

Every command speaks to the daemon over local IPC and accepts a global
`--json` flag for machine-readable output.

```
grove <command> [args] [--json]
```

## Lifecycle

| Command | Description |
| --- | --- |
| `grove init [--php 8.5] [--no-php]` | First-run setup: config, root CA, a PHP build, and (when elevated) the resolver + CA trust. Parks `~/Code` by default. |
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
| `grove new <name> [--kind <kit>] [--path ~/Code] [--php 8.5] [--git]` | Scaffold a new project via `laravel new` and link it. `--kind`: `laravel` (plain) \| `livewire` \| `react` \| `vue` \| a community kit `vendor/package` (`--using`) \| `static`. |
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

Grove runs a site's long-running dev processes for you — typically the **Vite dev
server** (HMR) and a **queue worker**, plus anything else the application
declares — with the site's own Node/PHP. No `artisan serve` needed (Grove already
serves). Output goes to the Logs panel (`dev-<site>-*.log`). In the GUI it's the
⚡ toggle per site.

| Command | Description |
| --- | --- |
| `grove dev start [site]` | Start the site's dev processes. Defaults to the site in the current directory. |
| `grove dev stop [site]` | Stop them, including anything they spawned. Defaults to the current directory. |
| `grove dev list` | List sites with dev processes running. |

On Laravel 13.16+, Grove asks the application what to run via
`php artisan dev:list --json --except-vendor`, so processes you register with
`DevCommands` are supervised too:

```php
use Illuminate\Foundation\Console\DevCommands;

DevCommands::artisan('reverb:start', 'reverb')->orange();
DevCommands::register('stripe listen --forward-to '.config('app.url'))->green();
```

Two entries are deliberately skipped: `server` (Grove already serves the site
over FPM on its `.test` domain) and `logs` (`grove logs` tails the app log
already). Vendor-registered processes are excluded so an arbitrary Composer
package can't start processes inside the daemon.

Each process writes to `dev-<site>-<name>.log` in Grove's log directory, listed
by `grove logs`. `php` and the Node package manager are resolved to the versions
the site pins, not whatever is on your `PATH`.

Grove falls back to a Vite dev server plus a queue worker for non-Laravel sites
and Laravel versions without `dev:list`.

> **Run `grove dev` instead of `php artisan dev`, not alongside it.** Both
> supervise the same processes, so running both gives you two Vite servers
> fighting over port 5173 and two queue workers competing for the same jobs.
> `grove dev start` warns when it sees a `php artisan dev` process running. The
> check can't tell which project that process belongs to, so it is a warning
> rather than an error.

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
| `grove php install <version> [--variant grove\|common\|bulk]` | Download a self-contained static PHP-FPM build (e.g. `8.5`, `8.4`, `8.3`). |
| `grove php craft [--php <version>]` | Print the static-php-cli config for Grove's own PHP build. |
| `grove php ext [<version>] [--all]` | Audit a build's extensions against the ones the ecosystem expects. |
| `grove php register <version> <fpm-binary>` | Register a custom php-fpm binary (bring-your-own). |
| `grove php discover` | Auto-discover php-fpm binaries on this machine. |
| `grove php list` | List registered PHP builds, with a one-line extension summary each. |
| `grove use <version>` | Set the global default PHP version. |

### Extensions and the `--variant` flag

Grove's bundled PHP is a static build with a **fixed** extension set, and which
set you get matters. The two that [static-php-cli](https://github.com/crazywhalecc/static-php-cli)
publishes are not supersets of each other, so Grove builds the union itself:

| Variant | Size | Notes |
| --- | --- | --- |
| `grove` (default) | ~25 MB | Grove's own build. Everything below, in one binary: the PDO drivers **and** `intl`, `mysqli`, `sodium`, `readline`, `apcu`, `xsl`, plus `igbinary`, `shmop` and the System V extensions. Built by [`.github/workflows/php-build.yml`](../.github/workflows/php-build.yml). |
| `common` | ~14 MB | Upstream. Has `pdo_sqlite`, `pdo_pgsql`, `opcache`, `redis`, `gd`, `gmp`, `soap`. **Missing** `intl`, `mysqli`, `sodium`, `readline`, `apcu`, `xsl`. |
| `bulk` | ~36 MB | Upstream. Has those six plus `imagick`, `imap`, `swoole`, `dba`. **Missing** `pdo_sqlite` and `pdo_pgsql`. |

Both upstream gaps hurt in practice. Without `pdo_sqlite` a fresh Laravel app —
and its entire test suite — can't reach its default database, and `pdo_pgsql` is
how an app talks to Grove's bundled PostgreSQL. Without `mysqli` the WordPress
driver can't connect at all, and without `intl` you lose Laravel's `Number`
helpers, Filament, Nova and every package that `require`s `ext-intl`.

When no Grove build exists yet for a version, `grove php install` says so and
falls back to upstream `common` rather than refusing:

```console
$ grove php install 8.5
  resolving latest 8.5 for macos-aarch64 (grove)…
  no grove build for 8.5 yet — using upstream `common` instead (`grove php ext` shows what it's missing)
✓ php@8.5 (common) ready at …

  Heads up — this build is missing:
    mysqli       WordPress — its only MySQL driver
  See `grove php ext 8.5` for the full picture.
```

The label always reflects what you actually got, not what was asked for. Ask for
an upstream set explicitly with `--variant common|bulk`; those never fall back,
because silently swapping one extension hole for the other would be worse than
an error.

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

  20 optional extension(s) also missing — `--all` to list them.
```

`grove doctor` reports the same gap as a `php-extensions` check, and the GUI's
PHP panel shows it per build.

### Reproducing or extending Grove's PHP

The extension list lives in one place — `grove_runtime::extensions::BUILD_SET` —
and `grove php craft` prints the static-php-cli config generated from it:

```console
$ grove php craft --php 8.5 > craft.yml
$ spc craft craft.yml          # needs a compiler toolchain; see below
```

Grove's CI runs exactly this. Building it yourself needs a C toolchain
(Xcode command line tools **and** Homebrew on macOS, or a musl toolchain on
Linux) — which is precisely why Grove doesn't do it on your machine. If you need
an extension the build set doesn't have, the quicker route is a PHP you already
have, which is the thing Herd won't let you do:

```console
$ grove php register 8.5 /opt/homebrew/opt/php@8.5/sbin/php-fpm
$ grove isolate myapp 8.5
```

Set the default variant for future installs with `[general].php_variant`, or
point Grove at your own mirror with `GROVE_PHP_MIRROR` (it must keep upstream's
`<variant>/php-<version>-<cli|fpm>-<os>-<arch>.tar.gz` layout).

### Are downloads verified?

Grove checks a SHA-256 for everything it downloads **where the publisher offers
one**, before the file is written or executed:

| | Verified against |
| --- | --- |
| Grove's own PHP builds | the release asset's `digest` |
| Node | `SHASUMS256.txt` |
| Composer | `composer-stable.phar.sha256` |
| cpx | the release asset's `digest` |
| PostgreSQL | the asset's `.sha256` |
| Redis | the SHA-256 in `redis/redis-hashes` |

Two sources publish nothing usable, and Grove says so per download rather
than implying otherwise:

- **Upstream `common` / `bulk`** archives have no checksum at all — no
  `.sha256`, no signature, nothing in the directory listing. This is one more
  reason the `grove` variant is the default.
- **MySQL** publishes `.md5` and a GPG signature but no SHA-256. MD5 catches a
  corrupt transfer, not a chosen collision, and checking the signature needs an
  OpenPGP implementation and MySQL's key.

What this proves is worth stating plainly: a hash fetched from the same host as
the file catches storage corruption, a truncated transfer and a tampered
artefact. It does not defend against a publisher whose account is taken over
and who replaces both. Only Node publishes an independent chain — a GPG
signature over `SHASUMS256.txt` — and Grove does not check that yet.

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
php = "8.5"
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

### How much the backend is trusted

Not much, deliberately. It stores ciphertext and it does **not** get to decide
who can read it.

The first time your client sees a project it records that project's member list
under `~/.grove/secrets/`. From then on the recipients are whatever *you* last
agreed to — the server's copy is compared to yours, never obeyed. If the two
have diverged in either direction, `grove secret set` refuses and names the keys
involved rather than encrypting to a list you did not approve:

```console
$ grove secret set myapp APP_KEY=…
Error: the recipient list for "myapp" does not match what you agreed to
       (added: ["age1qy…"], removed: []). Refusing to encrypt.
```

`grove secret share` and `revoke` are how you record a change. That makes adding
a teammate a deliberate act rather than something the backend can announce —
which is the whole point, and the cost: a legitimate new colleague is refused
until someone runs `share`.

Each payload also carries a version **inside** the encryption, where the server
cannot edit it. A client refuses a payload older than the newest it has already
seen, so replaying yesterday's blob — reinstating a rotated secret, or a revoked
member's access — fails instead of passing silently.

Two things this does not yet do, both needing backend support: payloads are not
*signed*, so a compromised backend could in principle forge one your key would
still decrypt; and the API token is the license key rather than a separate
credential.

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
| `grove explain <id>` | Curate a debugging bundle (request + causal chain + error log) for an AI assistant. |
| `grove sql-capture on\|off\|status` | Correlate SQL queries with the timeline (MySQL). |

### Causal chain

With `grove sql-capture on`, Grove turns on MySQL's general query log and reads
it back to build a per-request **causal chain**: expand a request in the desktop
app's **Requests** panel (or via the MCP `grove_request_chain` tool / `grove
requests --json`) to see the SQL it issued and the mail it sent within its time
window, plus derived metrics (duration, query count). A toolbar toggle turns SQL
capture on/off. Grove sits in front of every request and captures mail centrally,
so this needs zero app instrumentation.

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

Expose Grove's bundled `php`, `composer`, `cpx`, `node`, `npm`, `npx` and
`laravel`, auto-switching to whatever version each project pins (`grove isolate`
/ `grove node use`) — so you can drop Herd/Valet entirely.

| Command | Description |
| --- | --- |
| `grove path install` | Create the shims + provision the toolchain, then print the PATH line to add. |
| `grove path show` | Show whether the shims are installed and on your PATH. |
| `grove path uninstall` | Remove the shims. |

### `cpx` — the Composer Package Executor

`cpx` is to Composer what `npx` is to npm: run a CLI from any Composer package
without installing it in your project or globally. Grove ships it as one of the
PATH shims, so it is simply there after `grove path install` — no
`composer global require`, and it runs on Grove's bundled PHP.

```console
$ cpx laravel/pint                       # or just: cpx pint
$ cpx friendsofphp/php-cs-fixer fix ./src
$ cpx phpstan analyse
```

Packages are installed into a cache under `~/.cpx` and run isolated from both
your project's and your global Composer dependencies. Like `npx`, cpx prefers a
binary your project already has in `vendor/bin` (pass `--skip-local` to force
the isolated copy).

cpx 2 also runs ad-hoc PHP with your app booted:

| Command | What it does |
| --- | --- |
| `cpx exec script.php` | Run a file with Composer's autoloader and, in a Laravel app, a fully booted application (`$app`, config, facades, `.env`). `--no-boot` skips the boot. |
| `cpx exec -r '<php>'` | Run raw PHP the same way. |
| `cpx tinker` | Hands off to your project's own `php artisan tinker`; elsewhere it opens a PsySH shell with the project booted. |
| `cpx alias laravel/pint pint` | Make your own shortcut for a package. |
| `cpx installed` / `cpx update` / `cpx clean` | Manage what cpx has cached. |

cpx needs **PHP 8.3 or newer**. Grove uses whatever PHP the current directory
resolves to (`grove isolate` / `grove use`) and refuses with an explanation if
that version is too old. `readline` makes `cpx tinker` pleasant to use — see the
variant table under [PHP](#php).

Grove keeps the cpx PHAR at `~/.grove/cpx.phar`, deliberately in your home
rather than in Grove's root-owned tree, so `cpx self-update` works.

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
| `grove ca rotate` | Replace the root CA with a freshly generated one and trust it. Needed once for CAs made before Grove constrained them to its TLD, and after changing `[general].tld`. |
| `grove ca uninstall` | Remove the Grove root CA from the system store. |
