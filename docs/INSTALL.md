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
grove 0.1.5
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
✓ config        created at /Users/you/Library/Application Support/Grove/config.toml
✓ root CA       generated at …/Grove/certs/grove-ca.pem
✓ root CA       added to the system trust store
✓ PHP 8.4       downloaded and registered
✓ resolver      /etc/resolver/test → 127.0.0.1:53
init complete — next run: sudo grove install
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
✓ service installed: /Library/LaunchDaemons/com.elyra.grove.plist (runs at boot, binds the ports, resolver ensured)
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
Grove 0.1.5
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
✓ root-ca        present at …/Grove/certs/grove-ca.pem
✓ privileges     http_port=80, elevated=true
✓ resolver       /etc/resolver/test present
✓ dns            127.0.0.1:53 answering
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
blog.test         laravel    8.4     no     http://blog.test
shop.test         laravel    8.4     no     http://shop.test
docs.test         static     8.4     no     http://docs.test
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
php = "8.4"
services = ["mysql", "redis"]
dev = true
```

```text
Bringing up myapp…
  ✓ link
  ✓ https on
  ✓ php 8.4
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

Grove installs and supervises its own PostgreSQL, MySQL and Redis — no Homebrew.

```bash
grove service list
```

```text
SERVICE      CATEGORY       INSTALLED  RUNNING   PORT
PostgreSQL   Database       no         no        5432
MySQL        Database       no         no        3306
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

### Snapshots (time-travel before a risky migration)

Because Grove owns the database, it can snapshot and roll it back in one command:

```bash
grove db snapshot --db myapp --note "before migrate"
# ...run the scary migration...
grove db list
grove db restore <id>     # data restored exactly as it was
```

Snapshots are plain SQL dumps under `$GROVE_HOME/snapshots/`. Works for MySQL
(omit `--db` for all databases) and PostgreSQL (`--engine postgres`).

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
them in your shell — so `php`, `composer`, `node`, `npm`, `npx` and `laravel`
resolve to whatever version each project pins — add the shims to your PATH:

```bash
grove path install
```

```text
✓ Installed shims for php, composer, node, npm, npx, laravel.
✓ provisioned toolchain: PHP 8.4 CLI, Composer, Node 22

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
