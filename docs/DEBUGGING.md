# Step-debugging with Xdebug

Grove can load **Xdebug** into its PHP-FPM pools on demand, so you can set
breakpoints and step through requests without editing `php.ini`.

```console
$ grove debug on
Xdebug enabled (FPM pools reloaded, DBGp port 9003).

$ grove debug status
Xdebug enabled (DBGp port 9003)
  php@8.5  ready (built into this PHP)
  php@8.4  unavailable — needs a PHP with Xdebug (grove php register)
```

There's also an **Xdebug toggle** in the GUI's **Tools** panel.

## How it works

- Grove loads Xdebug per FPM pool via `-d` startup flags — your global
  `php.ini` is never touched, and pools respawn instantly when you toggle.
- Xdebug runs in `start_with_request=trigger` mode: it's resident but dormant,
  so ordinary requests pay almost nothing. A request opts in with the
  `XDEBUG_TRIGGER` cookie / query param (use the "Xdebug helper" browser
  extension), or the env from `grove debug env` for CLI processes.
- Grove is the *runtime* half. Your editor is the other half: start a DBGp /
  "Listen for Xdebug" session on port **9003** (configurable via
  `[general].xdebug_port`); Xdebug connects out to it.

## Browser requests

1. In your editor, start a listener on port 9003.
2. `grove debug on`.
3. Flip the browser extension to *Debug* and reload your `*.test` page.

## CLI (artisan, tests)

```console
$ eval "$(grove debug env)"
$ php artisan queue:work    # now connects to your editor's listener
```

## Grove's own PHP builds don't ship Xdebug

Grove's bundled PHP does not include Xdebug — it is in the extension catalogue
as *optional* (`grove php ext` lists it), and not in the set Grove builds. So
Grove's own builds report **unavailable** in `grove debug status`.

Grove looks for it in three places, in order:

1. **Built into the PHP** — a user-registered or custom build that already has
   it. Only the mode directives are then needed.
2. **A drop-in at `<runtimes>/xdebug/<version>/xdebug.so`** — loaded with
   `-d zend_extension=`. This is the escape hatch for a Grove build.
3. **`xdebug.so` in the build's own `extension_dir`**, found by asking `php -i`.

Whether (2) works depends on the platform, and it is worth being precise rather
than optimistic. On Linux the builds are genuinely static and cannot `dlopen`
anything. On macOS they are not fully static — they link dynamically against
system libraries and report `enable_dl => On` — so a `.so` *can* be loaded in
principle. The real obstacle there is ABI: the extension has to be compiled
against the same PHP version and build flags, and Grove does not ship one.

So the reliable route is to register a PHP that **has** Xdebug — e.g. a dynamic
Homebrew PHP:

```console
$ grove php register 8.5 /opt/homebrew/opt/php/sbin/php-fpm
$ grove isolate myapp 8.5     # use it for a site
$ grove debug on
```
