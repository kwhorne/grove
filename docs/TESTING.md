# Testing Grove before tagging a version

Two paths: a **quick smoke test** on high ports (no sudo), and a **full real test**
with `*.test` domains in the browser (needs one elevated step).

## 0. Build

```bash
# Frontend (required before building the GUI binary)
cd crates/grove-gui/ui && pnpm install && pnpm build && cd -

# Binaries
cargo build --release            # -> target/release/grove and grove-gui
```

Put the binaries on PATH for convenience (optional):

```bash
export PATH="$PWD/target/release:$PATH"
```

### Build the macOS app / .dmg

```bash
cargo install tauri-cli --version "^2.0" --locked    # once
cd crates/grove-gui/ui && pnpm install && pnpm build && cd -
cargo tauri build --manifest-path crates/grove-gui/Cargo.toml
# → target/release/bundle/dmg/Grove_<version>_<arch>.dmg  (+ Grove.app)
```

Releases build these automatically: pushing a `v*` tag runs
`.github/workflows/release.yml`, which publishes the CLI tarballs and the
`.dmg` / `.deb` / `.AppImage` bundles to a GitHub Release.

## 1. Quick smoke test (no sudo, high ports)

```bash
export GROVE_HOME=/tmp/grove-test
mkdir -p "$GROVE_HOME"
cat > "$GROVE_HOME/config.toml" <<'EOF'
[general]
tld = "test"
default_php = "8.4"
http_port = 8080
https_port = 8443
dns_port = 5354
[services]
mail_enabled = true
mail_port = 11025
EOF

# PHP: download a bundled static build, or register an existing php-fpm
grove php install 8.4
# grove php register 8.4 "$HOME/Library/Application Support/Herd/bin/php84-fpm"

grove start
grove park ~/Code              # or: grove link  inside a project
grove list

# Serve a site (Host header simulates DNS on the high port)
curl -H "Host: <yoursite>.test" http://127.0.0.1:8080/

# Services
grove service install postgres && grove service start postgres
grove service install redis    && grove service start redis
grove service list
grove env <yoursite>           # .env snippet for the bundled services

# Node
grove node install 22
grove node use <yoursite> 22

# Mail-catcher (send a test mail to 127.0.0.1:11025, then:)
grove mail

# GUI
grove gui                      # launches the desktop app against this daemon

grove stop
```

## 2. Full real test (`*.test` in the browser)

Uses the default ports 80/443/53, the system resolver and a trusted CA.

```bash
unset GROVE_HOME               # use the real ~/Library/Application Support/Grove

sudo grove init                # CA + resolver + a PHP build (one elevated step)
sudo grove start               # binds 80/443/53
grove park ~/Code
grove secure myproject         # HTTPS

# Now open https://myproject.test in your browser — no hosts editing needed.
grove doctor
```

To undo everything afterwards:

```bash
sudo grove uninstall           # removes service, resolver and CA trust
```

## 3. What to verify

- [ ] `*.test` resolves and serves (Laravel / static / proxy drivers)
- [ ] HTTPS works with the Grove CA (green padlock after `grove init`)
- [ ] Per-site PHP (`grove isolate`) and Node (`grove node use`) take effect
- [ ] `grove php install` / `grove node install` download and run self-contained
- [ ] `grove service install/start/stop/restart` for postgres/mysql/redis
- [ ] Mail-catcher captures mail; `grove mail` / GUI Mail panel show it
- [ ] GUI: Sites, Services, Mail, PHP, Node, Logs, Doctor + Settings (⌘,)
- [ ] `grove doctor` is all green

> Tip: run the daemon in the foreground with logs while testing:
> `GROVE_LOG=info grove daemon`
