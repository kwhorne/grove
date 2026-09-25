# Installing Grove

This is the complete, step-by-step guide to installing **Elyra Grove** and
serving your first `*.test` site with local HTTPS.

Grove is a single, self-contained Rust daemon with a thin CLI and a desktop GUI.
It has **zero external dependencies** — it downloads and manages its own PHP,
Node, PostgreSQL, MySQL and Redis. You do **not** need Homebrew, Composer,
dnsmasq, nvm or anything else.

- [1. Requirements](#1-requirements)
- [2. Install the app](#2-install-the-app)
- [3. First-run setup](#3-first-run-setup)
- [4. Install the background service](#4-install-the-background-service)
- [5. Verify it works](#5-verify-it-works)
- [6. Serve your first site](#6-serve-your-first-site)
- [7. Enable HTTPS and pin a PHP version](#7-enable-https-and-pin-a-php-version)
- [8. The desktop GUI](#8-the-desktop-gui)
- [9. Bundled databases & services](#9-bundled-databases--services)
- [10. PHP & Node versions](#10-php--node-versions)
- [11. Mail, logs & diagnostics](#11-mail-logs--diagnostics)
- [12. Troubleshooting](#12-troubleshooting)
- [13. Updating](#13-updating)
- [14. Uninstalling](#14-uninstalling)

---

## 1. Requirements

| | |
|---|---|
| **OS** | macOS 12 (Monterey) or newer |
| **Architecture** | Apple Silicon (`arm64`) or Intel (`x86_64`) |
| **Privileges** | An admin account (you will run `sudo` once during setup) |
| **Disk** | ~300 MB for the app + one PHP build; more as you add runtimes |

> Grove needs the privileged ports **53** (DNS), **80** (HTTP) and **443**
> (HTTPS). The installer sets up a small background service that owns those
> ports for you — see step 4.

---

## 2. Install the app

1. Download the latest `Grove_<version>_aarch64.dmg` (Apple Silicon) or
   `…_x64.dmg` (Intel) from the
   [releases page](https://github.com/kwhorne/grove/releases/latest).
2. Open the `.dmg` and drag **Grove** into **Applications**.
3. Launch **Grove** from Applications.

The app is **code-signed with a Developer ID and notarized by Apple**, so it
opens normally — no Gatekeeper warning, no `xattr` workarounds.

### Make the `grove` command available (recommended)

The CLI ships *inside* the app. Symlink it onto your `PATH` so you can type
`grove` anywhere:

```bash
sudo ln -sf "/Applications/Grove.app/Contents/MacOS/grove" /usr/local/bin/grove
```

```text
Password:
```

Verify:

```bash
grove --version
```

```text
grove 1.9.0
```

> Every example below uses `grove …`. If you skip the symlink, replace `grove`
> with the full path `/Applications/Grove.app/Contents/MacOS/grove`.

---

## 3. First-run setup

Run the one-time setup. This creates the config, generates a local root
Certificate Authority (for HTTPS), installs a static PHP build, and registers
the macOS DNS resolver for `.test`.

```bash
sudo grove init
```

```text
Password:
Grove setup:
  ✓ parked ~/Code (existing projects auto-imported)
  ✓ created config at /Users/you/Library/Application Support/Grove/config.toml
  ✓ root CA at /Users/you/Library/Application Support/Grove/certs/grove-ca.pem
  ✓ installed php@8.5
  ✓ resolver installed for .test
  ✓ root CA trusted in system store

Next: `sudo grove install` to run Grove as a service on 80/443/53, then open https://<project>.test
```

> **Why `sudo`?** Trusting the CA and writing `/etc/resolver/test` require
> administrator rights. Grove drops back to your user for everything else, so
> your files stay owned by you.

---

## 4. Install the background service

Install Grove as a **system service**. It runs in the background, binds ports
53/80/443, starts automatically at boot, and restarts if it ever crashes. PHP
itself still runs as **your** user, not root.

```bash
sudo grove install
```

```text
Password:
✓ service installed: /Library/LaunchDaemons/com.elyra.grove.plist (runs at boot as you, launchd binds the ports, resolver ensured)
```

That's it — Grove is now running. You never need `sudo grove start` again.

> A harmless `Boot-out failed: 5: Input/output error` line may appear before the
> success message. That is just Grove cleaning up a service that wasn't loaded
> yet — you can ignore it.

---

## 5. Verify it works

Check the daemon and environment:

```bash
grove status
```

```text
Grove 1.9.0
  TLD          .test
  HTTP         :80
  HTTPS        :443
  DNS          :53
  Sites        0
  ● dns
  ● mail
```

Run diagnostics:

```bash
grove doctor
```

```text
✓ config         loaded from /Users/you/Library/Application Support/Grove/config.toml
✓ root-ca        present at /Users/you/Library/Application Support/Grove/certs/grove-ca.pem
✓ root-ca-scope  constrained to .test
✓ resolver       *.test resolves to 127.0.0.1
✓ ipc-socket     mode 0660, owner uid 501
✓ grove-home     owned by uid 501, mode 0755
✓ site-certs     3 issued, soonest expires in 321 days (myapp_test); renewed within 30
✓ trust-store    this CA (sha256 fb7d743ac304614d…) is trusted, and no stale Grove CA is
✓ dns            listening on :53
✓ http           listening on :80
✓ https          listening on :443
✓ mail           listening on :1025
✓ privileges     http_port=80, elevated=false, sockets from the service manager: tcp/443, tcp/53, tcp/80, udp/53
✓ php-extensions 1 build(s), nothing required missing
```

`doctor` exits non-zero if anything shows `✗`, so it can gate a script. It
works with the daemon stopped too: the config, CA and resolver checks run
locally and the daemon line says so.

`privileges` is where you see the daemon is not root: `elevated=false` beside
the privileged ports it was handed. On a machine that still runs the daemon as
root — one where `grove install` could not work out who to serve — it reads
`elevated=true` and `sockets from the service manager: none`, and that is also
fine.

Four of the lines re-check, on every run, what 1.5.0 fixed once: `ipc-socket`
(the socket every privileged operation goes through must not be
world-accessible), `grove-home` (the tree root reads binaries out of must not be
world-writable), `site-certs` (soonest expiry; expired leaves are reissued on
the next request), and `trust-store` — is *this* CA what the machine trusts,
and is it the only Grove CA it trusts. That last one catches what `grove ca
rotate` used to leave behind: an old, unconstrained CA still in the keychain,
able to sign any hostname the machine will believe. When it finds one it prints
the exact removal command:

```text
✗ trust-store    an old, unconstrained Grove CA is still trusted — it can sign any hostname this machine will believe. Remove it: sudo security delete-certificate -Z 3F2A… /Library/Keychains/System.keychain
```

*Trusted*, not merely present: `grove ca rotate` strips the old CA's trust
settings but leaves the certificate itself sitting in the keychain, where a
search still finds it and nothing on the machine will chain to it. Doctor asks
the system's trust evaluator, so a leftover like that stays quiet and only a CA
the machine would actually believe raises the failure.

When another server already holds a port, the listener line names it:

```text
✗ http           could not bind :80: Address already in use (os error 48) — held by httpd (pid 412)
```

Confirm DNS resolution goes through Grove:

```bash
dig +short whatever.test
```

```text
127.0.0.1
```

If you see `127.0.0.1`, the system is correctly routing `*.test` to Grove. 🎉

---

## 6. Serve your first site

You have two ways to expose projects.

### Option A — Park a whole folder

Point Grove at a directory; **every** subdirectory becomes `<name>.test`.

```bash
grove park ~/Code
```

```text
✓ parked ~/Code — 12 sites now resolve as <name>.test
```

A project at `~/Code/blog` is instantly available at `http://blog.test`.

### Option B — Link a single project

From inside a project directory:

```bash
cd ~/Code/blog
grove link
```

```text
✓ linked blog → http://blog.test
```

List everything Grove serves:

```bash
grove list
```

```text
SITE              DRIVER     PHP     HTTPS  URL
blog.test         laravel    8.5     no     http://blog.test
shop.test         laravel    8.5     no     http://shop.test
docs.test         static     8.5     no     http://docs.test
```

Grove auto-detects the right driver (Laravel, WordPress, plain PHP, static, or a
reverse proxy) from each project's contents.

### Option C — Reproducible with `grove.toml`

Commit a `grove.toml` to a project so anyone can reproduce its environment. From
a fresh clone:

```bash
grove up --write     # scaffold a starter grove.toml (edit to taste)
grove up             # link + pin PHP/Node + start services + optional dev
```

```toml
# grove.toml
name = "myapp"
php = "8.5"
services = ["mysql", "redis"]
dev = true
```

```text
Bringing up myapp…
  ✓ link
  ✓ https on
  ✓ php 8.5
  ✓ mysql
  ✓ redis
  ✓ dev
✓ myapp is up → https://myapp.test
```

A teammate goes from `git clone` to a running, identical setup with one command.

---

## 7. Enable HTTPS and pin a PHP version

Turn on local TLS for a site (served from Grove's trusted CA):

```bash
grove secure blog
```

```text
✓ blog is now served over HTTPS → https://blog.test
```

Open **https://blog.test** — a valid padlock, no warnings.

Pin a specific PHP version for one site without affecting the others:

```bash
grove isolate blog 8.3
```

```text
✓ blog isolated to PHP 8.3
```

Revert when you're done:

```bash
grove unisolate blog
grove unsecure blog
```

### Create a brand-new project

Grove scaffolds with the official `laravel new` installer, so you can pick a
starter kit — `laravel` (plain), `livewire`, `react`, `vue`, or a community kit
`vendor/package`:

```bash
grove new myapp --kind vue
```

```text
✓ scaffolded a fresh Laravel app at ~/Code/myapp
✓ linked myapp → http://myapp.test
```

---

## 8. The desktop GUI

Launch **Grove** from Applications for a dashboard over the same daemon:

- **Sites** — view, secure, isolate and open every site
- **Services** — start/stop bundled databases and caches
- **Mail** — read captured outgoing email
- **PHP / Node** — install and switch runtime versions
- **Logs** — tail daemon, site and service logs
- **Doctor** — run diagnostics from the UI

The status pill (top-right) shows **● Running** once it connects to the
background service.

> The GUI is just a client. Don't use a "Start" button to launch a second
> daemon — the installed background service already owns the ports. If the GUI
> shows **Stopped** while sites clearly work, see
> [Troubleshooting](#gui-shows-stopped-but-sites-work).

---

## 9. Bundled databases & services

Grove installs and supervises its own PostgreSQL, MySQL, ElyraSQL and Redis —
no Homebrew.

```bash
grove service list
```

```text
SERVICE      CATEGORY       INSTALLED  RUNNING   PORT
PostgreSQL   Database       no         no        5432
MySQL        Database       no         no        3306
ElyraSQL     Database       no         no        3307
Redis        Cache & Queue  no         no        6379
```

Install and start one:

```bash
grove service install postgres
grove service start postgres
```

```text
✓ PostgreSQL installed (16.x)
✓ PostgreSQL running on :5432
```

Print a ready-made `.env` block wiring an app to Grove's services:

```bash
grove env
```

```text
DB_CONNECTION=pgsql
DB_HOST=127.0.0.1
DB_PORT=5432
DB_DATABASE=grove
DB_USERNAME=grove
DB_PASSWORD=
REDIS_HOST=127.0.0.1
REDIS_PORT=6379
MAIL_MAILER=smtp
MAIL_HOST=127.0.0.1
MAIL_PORT=1025
```

### On demand (a database that costs nothing while you are not using it)

An installed database runs from boot whether anything talks to it or not. An
idle MySQL takes 500–700 MB. In on-demand mode Grove holds the port instead,
and the server starts only when something connects:

```bash
grove service on-demand mysql on            # stops after 10 minutes with nothing connected
grove service on-demand redis on --idle 30m
grove service list
```

```text
SERVICE      CATEGORY       INSTALLED  RUNNING   PORT   MODE
MySQL        Database       yes        idle      3306   on demand, stops after 10m idle
Redis        Cache & Queue  yes        yes       6379   on demand, stops after 30m idle
```

`idle` means the port answers and the server is not running. The first
connection waits the fraction of a second the server takes to start, about
0.35 s for MySQL, and is then served normally. Grove starts the server as soon as
the site's name is looked up or its first request arrives, so the wait is
often shorter than that, and hidden entirely when the browser resolves the
name while you type it. Nothing in `.env` changes, since
the host and port stay the same. A queue worker or an open database client
keeps the server up for as long as it stays connected.

Connect over TCP (`DB_HOST=127.0.0.1`), which is Laravel's default. In this
mode the server's unix socket is moved to a private name. A client on the
socket would bypass Grove, go uncounted, and be cut off when the server went
idle. `grove service on-demand mysql off` makes it an always-on server again.

### Snapshots (time-travel before a risky migration)

Because Grove owns the database, it can snapshot and roll it back in one command:

```bash
grove db snapshot --db myapp --note "before migrate"
# ...run the scary migration...
grove db list
grove db restore <id>     # data restored exactly as it was
```

A MySQL restore puts each database in the snapshot back *exactly*. A table
created after the snapshot is gone afterwards, not left beside the restored
ones. Databases the snapshot does not contain are not touched, and MySQL's own
system schemas never are.

Snapshots live under `$GROVE_HOME/snapshots/`. MySQL (omit `--db` for all
databases) and PostgreSQL (`--engine postgres`) snapshots are plain SQL dumps;
an ElyraSQL snapshot (`--engine elyrasql`) is a hot, consistent copy of its
single database file, taken while it serves.

### Trying another branch without leaving yours

```bash
cd ~/Code/myapp
grove try feature/invoices      # a colleague's branch, fetched from origin if needed
grove try --list
grove try --done feature/invoices
```

A try is a second checkout, running at the same time as yours at
`https://myapp--feature-invoices.test`. It has its own git worktree under
`~/.grove/try/`, its own `.env` (a copy of yours with the hostname and
`DB_DATABASE` moved), and its own copy of your database, which the branch's
migrations then run against. Your checkout stays on its branch and your
database is not touched. `vendor/` and `node_modules/` are cloned from your
checkout, so `composer install` only fetches what the branch changed.

`--done` takes the site, the database copy and the worktree away. It refuses,
and lists the files, while the worktree holds uncommitted work, unless you pass
`--force`.

### Replaying a request against the same data

`grove replay <id>` sends a recorded request again. A request that writes then
finds its own work from last time, so the second run is not the first run
again. `--same-data` holds the database still:

```bash
grove replay 812 --same-data    # first time: snapshot the site's database, then replay
grove replay 812 --same-data    # every later time: put it back, then replay
grove replay 812 --forget       # done: drop the snapshot
```

The starting point is the data as it is at the first `--same-data`, so if the
original request wrote something, undo that first. It works for MySQL on
Grove's own server and for SQLite. Baselines last until the daemon restarts.

### Finding the commit that broke a request

Grove records every request it proxies (`grove requests`). When one that used
to work now fails, give `grove bisect` a commit where it worked:

```bash
grove requests myapp            # find the failing request's id
grove bisect --good v2.3.0 --request 812
```

Each commit is checked out beside your checkout, not in it, and gets a fresh
copy of your database migrated to that commit. The request is replayed exactly
as it was sent, and `git bisect` does the rest. A commit counts as good when it
answers below 500, or `--expect-status 200` when you want an exact code.

### A database per git branch

Checking out a branch changes the code in a second and leaves the database
where it was. A feature branch's migrations land in the tables `main` uses, and
going back to `main` means `migrate:fresh` or an app that no longer matches its
schema. Grove can keep one database per branch and swap it in when you check
the branch out:

```bash
cd ~/Code/myapp
grove db branches on
git checkout -b feature/invoices     # starts from main's data
php artisan migrate                  # only feature/invoices sees this
git checkout main                    # main's tables are back, untouched
```

```bash
grove db branches
```

```text
myapp  mysql myapp
  live      main  (checked out)
  parked   feature/invoices             3 tables    70.6 MB
```

The database keeps its name the whole time. Grove swaps its *contents*, so every
client sees the right branch without being told: the app, `php artisan` in a
terminal, a test run, TablePlus. On MySQL a swap is one `RENAME TABLE` moving
every table at once, which takes the same few tens of milliseconds for 70 MB as
for an empty schema. On SQLite it is two file renames. The first visit to a
branch is the one case that copies: the branch you left gets a parked copy of
the data, and the data you were using carries on as the new branch's starting
point.

While a switch runs, the site answers `503` with `Retry-After`, and its
`grove dev` processes stop and start again afterwards, so a queue worker runs
the new branch's code against the new branch's data. A detached `HEAD` never
moves anything, which keeps the database still through a rebase, a bisect or
`git checkout <sha>`.

Things to know:

- **MySQL on Grove's own server and SQLite files only, so far.** A remote MySQL,
  PostgreSQL and ElyraSQL are refused with a message, not half-followed.
- **A MySQL database with views, triggers, stored routines or events is
  refused.** They belong to the schema rather than to a table, so a table swap
  would leave them acting on the wrong branch's data.
- **Two worktrees cannot follow the same database.** Each would swap the other's
  data away; give one of them its own `DB_DATABASE`.
- **Parked copies live beside the live one:** MySQL as `<database>__gb_<id>`
  schemas, SQLite under `$GROVE_HOME/branch-databases/`. `grove db branches off`
  stops following and keeps them; `on` picks them up again. Deleting one is
  always explicit: `grove db branches drop <branch>`. The status names the
  copies whose git branch has been deleted.
- **A database client writing at the moment of a switch is outside what Grove
  can hold back.** On MySQL it sees one branch or the other, never a mix. On
  SQLite a switch waits while another process has the file open, and says which
  one.

### ElyraSQL

[ElyraSQL](https://github.com/kwhorne/ElyraSQL) is a MySQL-compatible SQL
server in one static binary, with the whole database in one file. Three things
to know when you pick it over MySQL:

- **Your app uses the MySQL driver.** It speaks MySQL's wire protocol, so
  `DB_CONNECTION=mysql` with `DB_PORT=3307`; Laravel migrations and Eloquent run
  unchanged, and any MySQL client (`mysql`, DBeaver, TablePlus) connects.
- **One database, named `elyra`.** Set `DB_DATABASE=elyra`. Laravel's
  `CREATE DATABASE IF NOT EXISTS` is a no-op there, so `php artisan migrate`
  works; an unconditional `CREATE DATABASE` is refused. `grove env` prints the
  right block when ElyraSQL is the installed database.
- **No accounts on loopback.** Like Grove's MySQL (`--initialize-insecure`) it
  runs without authentication on `127.0.0.1`; use `root` with an empty password.

```bash
grove service install elyrasql && grove service start elyrasql
grove env
```

```text
DB_CONNECTION=mysql
DB_HOST=127.0.0.1
DB_PORT=3307
DB_DATABASE=elyra
DB_USERNAME=root
DB_PASSWORD=
```

Grove tells a site on ElyraSQL apart from one on MySQL by the port it connects
to, so `grove db snapshot`, the agent-safe migration sandbox and `grove bundle`
all go to the right server. The desktop app's **Tools → Convert** takes it as a
source or target — the quickest way to move an existing MySQL or SQLite
database onto it. Published for macOS (Apple silicon) and Linux (x86_64,
aarch64); there is no Intel macOS build upstream. Needs ElyraSQL 1.11.2 or
later, which is what Grove installs.

---

## 10. PHP & Node versions

List, install and switch PHP:

```bash
grove php list
```

```text
php@8.4  →  …/Grove/runtimes/8.4/php-fpm
```

```bash
grove php install 8.3
grove use 8.3            # set the global default
```

Node works the same way:

```bash
grove node install 22
grove node use 22
```

```text
✓ Node 22.x installed
✓ default Node set to 22
```

### Use them in your terminal (drop Herd/Valet)

By default Grove uses these bundled runtimes to *serve* your sites. To also use
them in your shell — so `php`, `composer`, `cpx`, `node`, `npm`, `npx` and
`laravel` resolve to whatever version each project pins — add the shims to your
PATH:

```bash
grove path install
```

```text
✓ Installed shims for php, composer, cpx, node, npm, npx, laravel.
✓ provisioned toolchain: PHP 8.5 CLI, Composer, cpx, Node 22

    echo 'export PATH="$HOME/Library/Application Support/Grove/shims:$PATH"' >> ~/.zshrc
```

Add that line, restart your shell, and your terminal `php` / `composer` come from
Grove — auto-switching per project. This is what lets you uninstall Herd/Valet
entirely; afterwards run `sudo grove install` once to re-assert the resolver + CA.

---

## 11. Mail, logs & diagnostics

Grove runs a built-in mail-catcher on `127.0.0.1:1025`. Anything your apps send
is captured (never delivered) and viewable:

```bash
grove mail
```

```text
#  FROM                 TO                SUBJECT                 RECEIVED
1  hello@blog.test      you@example.com   Welcome aboard!         12:04:31
```

List and tail logs:

```bash
grove logs
grove logs daemon
```

```text
available logs: daemon, dns, mail, blog, shop
```

See a live timeline of every request Grove proxied — any site, any framework, no
setup (also the **Requests** panel in the GUI):

```bash
grove requests
```

```text
12:04:31.512  200  GET      8ms  blog     /
12:04:31.601  200  GET      3ms  blog     /build/assets/app.css
12:04:33.020  404  GET      1ms  blog     /favicon.ico
```

---

## Linux (beta)

Grove's core — the daemon, DNS, HTTPS, PHP-FPM, the bundled databases — is
platform-neutral Rust and builds and tests on Linux in CI. The OS integration is
newer than on macOS and has had less real-world use, so: beta.

```bash
sudo grove init            # config, CA, PHP; resolver + trust need root
sudo grove install         # /etc/systemd/system/grove.service
grove doctor
```

What `sudo grove install` sets up, and the assumptions behind it:

- **A system unit that runs the daemon as you** (`User=`), not as root.
  systemd binds the privileged ports and hands the descriptors over, so there
  is nothing left to need privilege for; children inherit your identity rather
  than being dropped to it. Earlier versions wrote a `systemctl --user` unit,
  which cannot bind privileged ports at all, and then a root unit that could.
  Since 1.9.0 the daemon refuses to run as root at all; if `grove install`
  cannot work out who to serve, run it with `sudo` from your own account.
- **A companion `grove.socket` unit.** systemd binds 80, 443 and 53 (both UDP
  and TCP) and hands the listening descriptors to the daemon, which serves on
  them without binding anything itself. It is `Wants=`, not `Requires=`: if the
  socket unit cannot bind, the daemon still starts and binds what it can.
  `grove doctor` names what arrived on the `privileges` line.
- **DNS through systemd-resolved.** Grove creates a dummy link `grove0`, points
  it at its own DNS on `127.0.0.1:53`, and routes `~test` to it; the unit
  recreates that on every boot (the settings do not persist on their own).
  Without `resolvectl` — some NetworkManager-only or musl setups — the install
  says so and you add sites to `/etc/hosts` or point your resolver at Grove.
- **CA trust in two places.** The system store (Debian/Ubuntu via
  `update-ca-certificates`, Fedora/RHEL/Arch via `update-ca-trust`) is what
  `curl`, PHP and OpenSSL read. **Chrome and Firefox do not read it** — they
  keep their own NSS databases — so Grove also adds the CA to `~/.pki/nssdb`
  and to each Firefox profile it finds, using `certutil`. Install
  `libnss3-tools` (Debian/Ubuntu) or `nss-tools` (Fedora) first, or the
  padlock stays red in the browser however green `curl` is.

`grove doctor` reports the resolver and the listeners on Linux the same way it
does on macOS. Windows is not supported: the code has stubs, nothing works
end to end, and the badge no longer claims otherwise.

## 12. Troubleshooting

### `DNS_PROBE_FINISHED_NXDOMAIN` / `ERR_NAME_NOT_RESOLVED` in the browser

The browser isn't routing `.test` to Grove. Re-create the resolver and flush the
DNS cache:

```bash
sudo mkdir -p /etc/resolver
printf 'nameserver 127.0.0.1\nport 53\n' | sudo tee /etc/resolver/test
sudo dscacheutil -flushcache
sudo killall -HUP mDNSResponder
```

```text
nameserver 127.0.0.1
port 53
```

Confirm:

```bash
dig +short blog.test      # must print 127.0.0.1
```

> If it still fails, disable **Chrome → Settings → Privacy → Use secure DNS**.
> "Secure DNS" (DoH) bypasses `/etc/resolver` and sends `.test` to a public
> resolver that doesn't know your domains.
>
> From v0.1.5, `sudo grove install` re-creates this resolver automatically, so
> re-running it also fixes the problem.

### `Address already in use` / ports 80/443 won't bind

Another daemon (often a leftover `sudo grove start`) is holding the ports.
Restart the service cleanly:

```bash
sudo launchctl bootout system/com.elyra.grove 2>/dev/null
sudo pkill -f "grove daemon" 2>/dev/null
sleep 2
sudo launchctl bootstrap system /Library/LaunchDaemons/com.elyra.grove.plist
```

Check who is listening:

```bash
sudo lsof -nP -iTCP:80 -iTCP:443 | grep LISTEN
```

### GUI shows "Stopped" but sites work

This was fixed in **v0.1.5** (the GUI and daemon now share the exact same home
directory). Update the app to v0.1.5 or newer. As an immediate workaround on
older builds:

```bash
cd "$HOME/Library/Application Support"
rm -rf "com.elyra.Grove"
ln -s "Grove" "com.elyra.Grove"
```

The GUI re-checks every few seconds and will flip to **● Running**.

### "Grove is damaged and can't be opened"

Only happens on unsigned builds. The official `.dmg` is notarized. If you built
from source yourself:

```bash
xattr -dr com.apple.quarantine /Applications/Grove.app
```

### Inspect the daemon's own logs

```bash
tail -f "$HOME/Library/Application Support/Grove/daemon.out.log"
tail -f "$HOME/Library/Application Support/Grove/daemon.err.log"
```

---

## 13. Updating

The app updates itself: when a new signed release is published, Grove shows an
in-app banner — click **Update** and relaunch.

To update the CLI symlink target, nothing is needed; it always points at the
installed app bundle.

---

## 14. Uninstalling

If you added the toolchain to your PATH, remove the shims first (and delete the
PATH line from your shell profile):

```bash
grove path uninstall
```

Remove the background service, the DNS resolver and the CA trust:

```bash
sudo grove uninstall
```

```text
Password:
✓ service removed
✓ resolver removed (/etc/resolver/test)
✓ root CA untrusted
```

Then drag **Grove** from Applications to the Trash, remove the symlink, and (if
you want a clean slate) delete the state directory:

```bash
sudo rm -f /usr/local/bin/grove
rm -rf "$HOME/Library/Application Support/Grove"
```

---

Questions or problems? Open an issue at
<https://github.com/kwhorne/grove/issues> or start a discussion at
<https://github.com/kwhorne/grove/discussions>.
