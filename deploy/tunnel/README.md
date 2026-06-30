# Deploying the Grove Tunnel server

This sets up the public half of `grove share` at `grove.elyracode.com`, so that
sharing a local site yields a URL like `https://3kf9a2qp.grove.elyracode.com`.

Architecture:

```
  Browser ──HTTPS──► Caddy (:443, on-demand certs)
                       │  reverse_proxy
                       ▼
                  grove-tunnel (:8000 http, :7000 control)
                       ▲  yamux
                       │
                  grove share  (each developer's machine)
```

- **Caddy** terminates public HTTPS and mints a Let's Encrypt certificate per
  subdomain on demand (no DNS-provider API needed).
- **grove-tunnel** holds the developer control connections (`:7000`) and serves
  relayed HTTP on `:8000` (localhost only; Caddy fronts it).
- Random subdomains are assigned automatically; `grove share --subdomain x`
  requests a specific one.

---

## 1. DNS

Point a wildcard and the apex at the server's public IP:

```text
grove.elyracode.com.     A     <SERVER_IP>
*.grove.elyracode.com.   A     <SERVER_IP>
```

(Add `AAAA` records too if the server has IPv6.)

## 2. Open the firewall

```bash
# Public HTTPS/HTTP (Caddy) + the tunnel control port
sudo ufw allow 80,443,7000/tcp
```

## 3. Install the binaries

`grove-tunnel` ships in every Grove CLI release tarball:

```bash
# On the server (Linux x86_64):
curl -sSL -o grove.tgz \
  https://github.com/kwhorne/grove/releases/latest/download/grove-v0.2.2-x86_64-unknown-linux-gnu.tar.gz
tar xzf grove.tgz
sudo install grove-tunnel /usr/local/bin/

# Caddy (Debian/Ubuntu):
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
  | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
  | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update && sudo apt install -y caddy
```

## 4. Configure Caddy

```bash
sudo cp Caddyfile /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

## 5. Run grove-tunnel as a service

```bash
sudo cp grove-tunnel.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now grove-tunnel
sudo systemctl status grove-tunnel
```

## 6. Test from a developer machine

`grove.elyracode.com:7000` is the **default** server, so no client config is
needed:

```bash
grove share myapp
# 🌿 Tunnel online → https://<random>.grove.elyracode.com
```

---

## Notes

- **Open vs. token**: the unit runs an open server (anyone can connect). To
  restrict it, add `--token <secret>` to `ExecStart` and have clients set
  `[tunnel].token` in their `config.toml`.
- **Abuse / rate limits**: an open tunnel can be misused. Consider Caddy
  `rate_limit`, fail2ban on `:7000`, or switching to token auth.
- **Capacity**: each shared site uses one control connection; requests are
  multiplexed, so a small VPS handles many tunnels.
