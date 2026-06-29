# Elyra Grove

> Native lokalt utviklingsmiljø i Rust — din egen Valet/Herd, cross-platform og uten lisensvegg.

Grove serverer `*.test`-domener med automatisk ruting, lokal HTTPS, PHP-versjons­håndtering
og tjenestestyring — fra én Rust-kjerne på macOS, Linux og Windows, uten Homebrew/Composer/dnsmasq.

Se [PRD](docs/PRD.md) for full produktbeskrivelse.

## Status

Fase 0 (PoC) + kjernen av Fase 1 er på plass og verifisert ende-til-ende på macOS:

| Område | Status |
| --- | --- |
| Embedded DNS-resolver (`*.test` → loopback, nekter andre TLD-er) | ✅ |
| HTTP/HTTPS reverse proxy (binder 80/443, Host-basert oppslag) | ✅ |
| FastCGI-klient → PHP-FPM (lazy pools, `pm = ondemand`) | ✅ |
| Drivere: Laravel, WordPress, generisk PHP, statisk, proxy | ✅ |
| Lokal CA + on-demand SNI leaf-sertifikater | ✅ |
| Innebygd PHP: last ned statisk PHP-FPM (`grove php install 8.4`) | ✅ |
| Bring-your-own PHP-binær (`grove php register`) | ✅ |
| Deklarativ TOML-config som kilde til sannhet | ✅ |
| CLI ↔ daemon over Unix-socket (JSON-RPC), `--json` | ✅ |
| OS-integrasjon: macOS resolver + trust store | ✅ |
| Tjeneste-livssyklus: `start`/`stop`/`restart` (pidfil + graceful shutdown) | ✅ |
| OS-tjenesteinstall: `install`/`uninstall` (launchd/systemd) | ✅ |
| Valet-import (`grove import`) | ✅ |
| Linux/Windows OS-integrasjon | 🚧 stubs |
| GUI (Tauri + Svelte) | ⏳ Fase 3 |
| Tjenester (DB/Redis/mail-catcher) | ⏳ Fase 4 |

## Arkitektur (workspace)

```
grove-core      site-register, driver-deteksjon, config, paths   (ren, ingen I/O på OS)
grove-ipc       JSON-RPC protokoll + transport (CLI/GUI ↔ daemon)
grove-tls       rot-CA + leaf-utstedelse (rcgen/rustls)
grove-dns       embedded resolver for *.<tld> (hickory)
grove-proxy     HTTP/HTTPS proxy + minimal FastCGI-klient (hyper)
grove-runtime   PHP-versjon + FPM-pool-supervisor
grove-os        plattformintegrasjon (resolver, trust store, elevasjon)
grove-daemon    langtkjørende prosess: binder porter, betjener IPC
grove-cli       clap-frontend (binær: `grove`)
```

## Kom i gang (utvikling)

```bash
cargo build

# Kjør med en isolert "grove home" og ikke-privilegerte porter for testing:
export GROVE_HOME=/tmp/grove-home
mkdir -p $GROVE_HOME
cat > $GROVE_HOME/config.toml <<'EOF'
[general]
tld = "test"
default_php = "8.4"
http_port = 8080
https_port = 8443
dns_port = 5354

[[parked]]
path = "~/Code"
EOF

# Last ned en innebygd, statisk PHP-FPM (ingen Homebrew/Herd nødvendig)
grove php install 8.4
# ...eller pek på din egen binær med ekstra extensions:
# grove php register 8.4 /path/to/php-fpm

# Start daemonen (binder porter, serverer sites)
grove daemon &

# Bruk CLI-en (snakker med daemonen over IPC)
grove list
grove secure mittprosjekt
grove proxy frontend http://127.0.0.1:5173
grove doctor --json
```

I produksjon binder daemonen 80/443/53 og krever ett minimalt elevert steg for
port-binding + resolver/trust store (PRD §10).

### Null eksterne avhengigheter

Grove har ingen kjøretidsavhengighet til Homebrew, Composer, dnsmasq, OpenSSL eller
Laravel Valet. DNS, proxy, FastCGI og TLS er innebygd i Rust-kjernen. Selv PHP kan
lastes ned som en selvstendig statisk binær via `grove php install` — den lenker
kun mot operativsystemets egne biblioteker. `grove import` *leser* en eksisterende
Valet-config hvis den finnes, men krever ikke at Valet er installert.

## Tester

```bash
cargo test
```

## Lisens

MIT (foreløpig — se PRD §14 åpne spørsmål).
