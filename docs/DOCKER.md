# Docker & OrbStack

Grove serves your Docker / OrbStack containers as `*.test` sites with trusted
local HTTPS — right next to native sites, in the same dashboard, and shareable
through the same public tunnels. There's nothing to configure: start your
containers as usual and they show up.

```console
$ docker compose up -d
$ grove list
SITE              DRIVER   PHP   HTTPS   URL
elyra-web.test    laravel  8.4   yes     https://elyra-web.test
inside2.test      proxy          yes     https://inside2.test    🐳

$ curl -sI https://inside2.test | head -1
HTTP/1.1 200 OK
```

Grove terminates TLS with its own trusted CA and reverse-proxies to the
container, so every container gets a green padlock without any per-container
certificate setup.

## How a container is discovered

Grove polls the Docker socket (`/var/run/docker.sock`, or OrbStack's) every few
seconds and picks the site host + upstream like this, in order:

1. **`grove.host` label** — explicit. Optional `grove.upstream` for the target.
   ```yaml
   labels:
     - "grove.host=myapi"                 # → myapi.test
     - "grove.upstream=http://127.0.0.1:3000"
   ```
2. **`dev.orbstack.domains` label** — Grove reuses OrbStack's own routing.
   A container with `dev.orbstack.domains=inside2.local` becomes `inside2.test`,
   proxied to `http://inside2.local` (OrbStack handles the internal port).
3. **`docker compose` project, no labels** — Grove picks the web container of a
   compose project (by service name / published web port) and serves
   `<project>.test` → `127.0.0.1:<published-port>`.

Database, cache and queue containers (MySQL, Redis, …) are ignored — you don't
want `mysql.test`.

## Starting & stopping

Container-backed sites carry a 🐳 badge in the GUI's **Sites** table with their
own controls:

- **▶ Start** a stopped container
- **⏹ Stop** / **↻ Restart** a running one

A stopped site serves a friendly "start it from the Sites list" page instead of
a connection error.

## Sharing a container publicly

Because a container is a first-class site, `grove share` tunnels it too:

```console
$ grove share inside2
  🌿  Tunnel online
     Public   https://7f3a2c9k.grove.elyracode.com
     Local    http://inside2.test
```

See [TUNNEL.md](TUNNEL.md).

## Turning it off

Set `docker = false` under `[general]` in `config.toml` (or via the GUI
Settings), and Grove stops touching Docker entirely.

## Notes

- Discovery is best-effort and can never hang the daemon (hard timeout).
- If a compose project's published port isn't the app's real port (some setups
  publish a decoy), add an explicit `grove.host` + `grove.upstream` label, or a
  `dev.orbstack.domains` label, to route it precisely.
