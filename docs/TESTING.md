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
default_php = "8.5"
http_port = 8080
https_port = 8443
dns_port = 5354
[services]
mail_enabled = true
mail_port = 11025
EOF

# PHP: download a bundled static build, or register an existing php-fpm
grove php install 8.5
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

## 2b. The suites that don't run by default

`cargo test --workspace` deliberately skips two groups. Both exist because the
code they cover cannot be exercised honestly from an ordinary test run, so
neither being green in CI means they passed — they have to be run on purpose.

### Privileged tests

Dropping privileges can only be *proved* by a process that has some. `setgroups`
is privileged even when it would change nothing, so an unprivileged run can only
demonstrate the refusal path. These files skip themselves unless they happen to
be running as root — a no-op on your machine, real evidence in a container:

```bash
docker run --rm -v "$PWD:/w" -w /w \
  -v grove-linux-target:/target -e CARGO_TARGET_DIR=/target \
  rust:alpine sh -c '
    apk add --no-cache musl-dev openssl-dev &&
    cargo test -p grove-core    --test privdrop_root     -- --nocapture &&
    cargo test -p grove-tls     --test ca_ownership_root -- --nocapture &&
    cargo test -p grove-runtime --test probe_root        -- --nocapture'
```

The named volume is worth the extra flags. Cargo keys artefacts by target
triple, so a container writing into `./target` does *not* destroy your host
build — but it does grow the directory by a second platform's worth of objects,
and it starts from cold every run. In a volume the first run is a ~12-minute
build and every run after it is seconds.

They cover: that a dropped child really comes out as the requested user and
group, with none of root's supplementary groups surviving; that with no target
recorded the child stays root rather than dropping somewhere arbitrary; that a
root-created CA is owned by root while a *user-owned* key is claimed on the next
root load, and the certificate stays readable either way; and that runtime probes
(`php -m` and friends) do not exec as root when a run user is known.

### Network tests

Checksum verification is only as good as its agreement with what publishers
actually serve today: a parser that handles *our idea* of `SHASUMS256.txt` and
not Node's would pass every unit test and fail every install. These are
`#[ignore]`d because they download real artefacts:

```bash
cargo test -p grove-runtime --test download_verification -- --ignored --nocapture
```

The cheapest of them (`published_checksum_documents_still_parse`) fetches only
the manifests, and is the one that catches a publisher changing format from
under us. Worth running before a release even if you skip the rest.

## 3. What to verify

- [ ] `*.test` resolves and serves (Laravel / static / proxy drivers)
- [ ] HTTPS works with the Grove CA (green padlock after `grove init`)
- [ ] Per-site PHP (`grove isolate`) and Node (`grove node use`) take effect
- [ ] `grove php install` / `grove node install` download and run self-contained
- [ ] `grove service install/start/stop/restart` for postgres/mysql/redis
- [ ] Mail-catcher captures mail; `grove mail` / GUI Mail panel show it
- [ ] `grove requests` / GUI Requests panel show proxied requests live
- [ ] `grove path install` puts php/composer/node on PATH; `grove db snapshot`/`restore` round-trips
- [ ] `grove up --write` scaffolds `grove.toml`; `grove up` links + configures the project
- [ ] `grove license activate/status` works; `grove secret set/pull/share/revoke` round-trips against the Teams backend
- [ ] GUI: Sites, Services, Mail, PHP, Node, Tunnels, Requests, Tools, Logs, Doctor + Settings (⌘,)
- [ ] `grove doctor` is all green

> Tip: run the daemon in the foreground with logs while testing:
> `GROVE_LOG=info grove daemon`
