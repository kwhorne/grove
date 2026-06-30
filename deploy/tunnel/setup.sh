#!/usr/bin/env bash
# One-shot setup for the Grove Tunnel server on Debian/Ubuntu.
#
#   sudo DOMAIN=grove.elyracode.com bash setup.sh
#
# Prerequisites:
#   - DNS:  grove.elyracode.com  and  *.grove.elyracode.com  → this server's IP
#           (if behind Cloudflare, use "DNS only" / grey cloud so HTTP-01 works)
#   - Ports 80, 443, 7000 reachable from the internet.

set -euo pipefail

DOMAIN="${DOMAIN:-grove.elyracode.com}"
TOKEN="${TOKEN:-}" # empty = open server; set TOKEN=... to require a token
ARCH="$(uname -m)"

case "$ARCH" in
  x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
  aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
  *) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

if [[ $EUID -ne 0 ]]; then echo "run as root (sudo)"; exit 1; fi

echo "==> Installing grove-tunnel ($TARGET) from the latest release"
TMP="$(mktemp -d)"
URL="https://github.com/kwhorne/grove/releases/latest/download/grove-LATEST-${TARGET}.tar.gz"
# The asset is named with the tag, so resolve the latest tag first.
TAG="$(curl -fsSL https://api.github.com/repos/kwhorne/grove/releases/latest \
  | grep -oE '"tag_name": *"[^"]+"' | head -1 | cut -d'"' -f4)"
curl -fsSL -o "$TMP/grove.tgz" \
  "https://github.com/kwhorne/grove/releases/download/${TAG}/grove-${TAG}-${TARGET}.tar.gz"
tar -C "$TMP" -xzf "$TMP/grove.tgz"
install "$TMP/grove-tunnel" /usr/local/bin/grove-tunnel
echo "    installed $(grove-tunnel --version 2>/dev/null || echo grove-tunnel)"

echo "==> Installing Caddy"
if ! command -v caddy >/dev/null; then
  apt-get update -y
  apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
    | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
    | tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
  apt-get update -y
  apt-get install -y caddy
fi

echo "==> Writing Caddyfile for *.$DOMAIN"
cat >/etc/caddy/Caddyfile <<EOF
{
	on_demand_tls {
		ask http://127.0.0.1:8000/__grove_ask
	}
}

*.$DOMAIN {
	tls {
		on_demand
	}
	reverse_proxy 127.0.0.1:8000
}
EOF
systemctl reload caddy || systemctl restart caddy

echo "==> Installing grove-tunnel systemd service"
TOKEN_ARG=""
[[ -n "$TOKEN" ]] && TOKEN_ARG="  --token $TOKEN \\"
cat >/etc/systemd/system/grove-tunnel.service <<EOF
[Unit]
Description=Grove Tunnel server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/grove-tunnel \\
  --domain $DOMAIN \\
  --control 0.0.0.0:7000 \\
  --http 127.0.0.1:8000 \\
$TOKEN_ARG
  --scheme https
Restart=always
RestartSec=2
DynamicUser=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now grove-tunnel

echo "==> Opening firewall (if ufw is active)"
if command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
  ufw allow 80,443,7000/tcp || true
fi

echo
echo "Done. grove-tunnel:"
systemctl --no-pager --full status grove-tunnel | head -n 6 || true
echo
echo "Test from a dev machine:  grove share <site>"
echo "Expect: https://<random>.$DOMAIN"
rm -rf "$TMP"
