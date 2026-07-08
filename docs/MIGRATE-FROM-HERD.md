# Migrate from Herd

Coming from Laravel Herd? Grove gives you the same effortless `*.test` + HTTPS
experience — but **open source**, with **no license wall** on databases, mail, or
dumps, the freedom to load **custom PHP extensions**, and a team layer Herd
doesn't have. This guide moves you over in a few minutes, with nothing lost.

## Why switch

| | Herd | **Grove** |
| --- | --- | --- |
| Open source | ❌ | ✅ (MIT core) |
| Databases / mail / dumps | Pro license | ✅ Free |
| Custom PHP extensions | ❌ | ✅ |
| Bundled PHP / Node / DBs | ✅ | ✅ |
| Public tunnels | Pro | ✅ Free |
| Request timeline, DB snapshots | ➖ | ✅ |
| **Encrypted team secret sync** | ❌ | ✅ (Grove Teams) |

Everything you rely on day-to-day is here — plus things Herd doesn't offer.

## 1. Install Grove

Download the app from [elyracode.com/grove](https://elyracode.com/grove), then:

```bash
sudo grove init       # config, root CA, a PHP build, resolver + trust
sudo grove install    # install the background service (binds 53/80/443)
```

## 2. Point Grove at your projects

```bash
grove park ~/Code           # every sub-folder becomes <name>.test
#   or, inside one project:
grove link
grove secure myapp          # enable HTTPS
```

Grove auto-detects the driver (Laravel, WordPress, plain PHP, static, proxy).

## 3. Bring your databases across

If you have databases in Herd's MySQL, migrate them in one step. Start Grove's
MySQL under **Services**, then use **Tools → Migrate MySQL from Herd** in the
desktop app — it dumps every database from Herd's MySQL and imports it into
Grove's. (Do this **before** removing Herd, while its MySQL data still exists.)

## 4. Put the toolchain on your PATH

Herd added `php`, `composer`, and `node` to your shell. Grove does too — pointing
at each project's pinned version:

```bash
grove path install
# add the printed line to your shell profile, then restart your shell
```

Now `php`, `composer`, `node`, `npm` and `laravel` come from Grove.

## 5. Vite over HTTPS

Use the standard `laravel-vite-plugin` in `vite.config.js` (no hard-coded cert
paths). Grove serves the Vite dev server over trusted HTTPS automatically when
you run `grove dev`, via the same `VITE_DEV_SERVER_CERT` / `VITE_DEV_SERVER_KEY`
mechanism Herd used — so a plain config works under both.

## 6. Remove Herd

Once your sites load and your data is migrated:

```bash
# 1. Remove Herd's lines from your shell profile (~/.zshrc):
#    the PATH export, HERD_PHP_*_INI_SCAN_DIR, and the NVM_DIR block.

# 2. Quit Herd, then remove its privileged helper (needs sudo):
sudo launchctl bootout system /Library/LaunchDaemons/de.beyondco.herd.helper.plist 2>/dev/null
sudo rm -f /Library/LaunchDaemons/de.beyondco.herd.helper.plist

# 3. Delete the app + its data:
rm -rf /Applications/Herd.app
rm -rf "$HOME/Library/Application Support/Herd"

# 4. Re-assert Grove's resolver + CA (in case Herd's uninstall touched them):
sudo grove install
```

Uninstalling Valet too? `valet uninstall --force` (or `composer global remove
laravel/valet` if you never ran `valet install`).

## 7. Verify

```bash
grove list                 # your sites
grove doctor               # should be all green
php -v                     # now Grove's PHP
```

Open a site over `https://…test` — trusted padlock, no warnings.

---

That's it. You keep the `*.test` + HTTPS workflow you're used to, gain custom
extensions and a genuinely free database/mail/dump/tunnel stack, and unlock
[Grove Teams](PRO.md) when your team is ready to sync secrets securely.

Questions or a snag? See [Installation](INSTALL.md) and [Commands](COMMANDS.md),
or reach out — we're happy to help you land.
