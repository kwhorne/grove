# Step-debugging with Xdebug

Grove can load **Xdebug** into its PHP-FPM pools on demand, so you can set
breakpoints and step through requests without editing `php.ini`.

```console
$ grove debug on
Xdebug enabled (FPM pools reloaded, DBGp port 9003).

$ grove debug status
Xdebug enabled (DBGp port 9003)
  php@8.4  ready (built into this PHP)
  php@8.3  unavailable — needs a PHP with Xdebug (grove php register)
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

## The static-PHP limitation

Grove's bundled PHP builds are **fully static**, and a fully-static PHP can't
load Xdebug (it can't `dlopen` an external `.so`, and static-php-cli can't
compile Xdebug in). So Grove's own builds report **unavailable** in
`grove debug status`.

To step-debug, register a PHP that **has** Xdebug — e.g. a dynamic Homebrew PHP:

```console
$ grove php register 8.4 /opt/homebrew/opt/php/sbin/php-fpm
$ grove isolate myapp 8.4     # use it for a site
$ grove debug on
```

Grove auto-detects Xdebug that's built into that PHP, or a loadable `xdebug.so`
in its `extension_dir`.
