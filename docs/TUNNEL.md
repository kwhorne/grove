# Sharing sites publicly (Grove Tunnel)

`grove share` exposes a local `*.test` site to the public internet — for demos,
testing on real devices, or receiving webhooks during development. It is a
native, self-hostable alternative to Expose/ngrok, built into Grove with zero
external dependencies.

```
   Internet                              Your machine
┌──────────────┐   1 control conn       ┌──────────────────┐
│ grove-tunnel │◄──────────────────────►│   grove share    │
│  (your VPS,  │   N request streams    │   (CLI client)   │
│  *.example)  │   (yamux multiplexed)  │         │        │
└──────┬───────┘                        └─────────┼────────┘
  public HTTP                               127.0.0.1:80
  foo.example.com  ───────────────────────►  elyra-web.test
```

Requests are multiplexed over a single connection, HTTP is spoken end-to-end
with `hyper`, and the `Host` header is rewritten to the local site name — so
bodies stream without buffering and **webhooks work out of the box**.

> **Just want to share?** Grove ships pointed at the public server
> `grove.elyracode.com`, so `grove share <site>` works with **no configuration**
> and gives you a `https://<random>.grove.elyracode.com` URL. The rest of this
> page is for running your **own** server.

---

## 1. Run your own tunnel server

A ready-made deployment (Caddy for automatic HTTPS + a systemd unit) lives in
[`deploy/tunnel/`](../deploy/tunnel/README.md). The summary below covers the
moving parts.

The public half is the `grove-tunnel` binary. You run it once on any host with a
public IP and a wildcard DNS record.

### DNS

Point a wildcard at your server:

```text
*.tunnel.example.com.   A    203.0.113.10
tunnel.example.com.     A    203.0.113.10
```

### Start the server

```bash
grove-tunnel \
  --domain tunnel.example.com \
  --token "a-long-shared-secret" \   # required; --allow-anonymous to opt out
  --control 0.0.0.0:7000 \
  --http 0.0.0.0:80
```

```text
INFO grove_tunnel::server: tunnel server listening control=0.0.0.0:7000 http=0.0.0.0:80 domain=tunnel.example.com
```

All flags can also be set via environment variables: `GROVE_TUNNEL_DOMAIN`,
`GROVE_TUNNEL_TOKEN`, `GROVE_TUNNEL_CONTROL`, `GROVE_TUNNEL_HTTP`,
`GROVE_TUNNEL_SCHEME`.

### Public HTTPS

For public `https://` URLs, put a TLS terminator in front (it already has a
wildcard cert story) and tell Grove to advertise `https`:

- **Caddy** (automatic Let's Encrypt for `*.tunnel.example.com`):

  ```caddy
  *.tunnel.example.com {
      reverse_proxy 127.0.0.1:80
  }
  ```

- or **Cloudflare** proxied DNS with a wildcard certificate.

Then run the server with `--scheme https` so it reports `https://…` URLs.

> A `systemd` unit is the simplest way to keep `grove-tunnel` running. Point
> `ExecStart` at the binary with the flags above and set `Restart=always`.

---

## 2. Point Grove at your server

The default server is `grove.elyracode.com:7000`. To use your own, set it in
`~/Library/Application Support/Grove/config.toml`:

```toml
[tunnel]
server = "tunnel.example.com:7000"
# token  = "a-long-shared-secret"   # only if your server requires one
```

---

## 3. Share a site

```bash
grove share elyra-web
```

```text
  Sharing elyra-web.test — connecting to tunnel…

  🌿  Tunnel online
     Public   https://3kf9a2qp.tunnel.example.com
     Local    http://elyra-web.test

  Press Ctrl-C to stop sharing.
```

Anyone can now reach your local site at that public URL. Stop sharing with
`Ctrl-C`.

### Options

| Flag | Purpose |
|------|---------|
| `--subdomain blog` | Request a memorable subdomain (`blog.tunnel.example.com`) instead of a random one. |
| `--server host:7000` | Override the configured server for one run. |
| `--token …` | Override the configured token. |
| `--basic-auth user:pass` | Require HTTP Basic auth on the public URL. |

```bash
grove share elyra-web --subdomain demo --basic-auth team:s3cret
```

---

## 4. Receiving webhooks

Point a third-party webhook (Stripe, GitHub, …) at your public URL:

```text
https://demo.tunnel.example.com/webhooks/stripe
```

Requests are streamed straight to your local app with the original method,
path, headers and body, so signature verification keeps working.

---

## Security

The tunnel is the one part of Grove that faces the public internet, so it is
worth being precise about what it does and does not protect.

**A token is required.** `grove-tunnel` refuses to start without `--token`
(or `GROVE_TUNNEL_TOKEN`). Running an open server is still possible, but it
takes an explicit `--allow-anonymous` and logs a warning — it used to be what
you got by leaving the flag off.

**The control channel is not encrypted.** Client↔server traffic on `:7000` is
plain TCP: the token and every request and response body cross the internet in
the clear, and the client does not verify the server's identity. Put the
control port on a private network, or tunnel it (WireGuard, SSH) — and treat
the public HTTP side as the only part a TLS terminator protects. Encrypting
this channel is planned; until then, do not carry anything through a tunnel
that you would not send over plain HTTP.

**Subdomains are first-come.** Any client with the token may claim any free
name, minus a reserved list (`www`, `api`, `admin`, `grove`, …). There is no
ownership model. A request to the apex domain matches no tunnel at all.

**Headers the server sets, and you can trust:** `X-Forwarded-For` is
overwritten with the real peer address, so an app behind the tunnel cannot be
told a false client IP. `X-Forwarded-Proto` and `X-Grove-Site` are set by the
server.

---

## How it works

- **One control connection** (TCP, token-authenticated) per shared site.
- **yamux** multiplexes every public request as its own stream — full
  concurrency over a single socket.
- **hyper on both ends**: the server runs an HTTP client over each stream, the
  client runs an HTTP server that proxies to the local `.test` site.
- The **public `Host` is preserved** end-to-end so the app builds correct public
  URLs (Vite, assets, redirects). The local site is selected via an
  `X-Grove-Site` header instead of rewriting `Host`, and `X-Forwarded-Proto`
  tells Grove's proxy to present the request to PHP as HTTPS (`HTTPS=on`) — so
  apps emit `https://` URLs without needing TrustProxies configured.

See [`crates/grove-tunnel`](../crates/grove-tunnel) for the implementation and an
end-to-end loopback test.
