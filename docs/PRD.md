# PRD: Elyra Grove

**Produktnavn:** Elyra Grove
**Tagline:** Native lokalt utviklingsmiljø i Rust — din egen Valet/Herd, cross-platform og uten lisensvegg
**CLI/crate-prefiks:** `grove`
**Type:** Native, cross-platform utviklingsmiljø for PHP/Laravel og generelle web-apper
**Stack:** Rust (kjerne), Tauri + Svelte (GUI), clap (CLI)
**Status:** Utkast v0.1
**Eier:** Knut / Wirelabs AS

---

## 1. Sammendrag

Elyra Grove er et native lokalt utviklingsmiljø skrevet i Rust som gjør for `*.test`-domener
det Laravel Valet og Herd gjør i dag — automatisk ruting, lokal HTTPS, PHP-versjonshåndtering
og tjenestestyring — men med tre forskjeller som er hele poenget med å bygge selv:

- **Én kodebase, tre plattformer.** macOS, Windows og Linux fra samme Rust-kjerne.
- **Null eksterne avhengigheter.** Ingen Homebrew, Composer eller dnsmasq som systempakke.
- **Del av elyra-conductor-økosystemet.** Kan kjøre frittstående, men designet som
  "dev-environment-motoren" inne i elyra-conductor sitt cockpit.

Grove er ikke et produksjonsmiljø og ikke en erstatning for Docker i komplekse oppsett.
Det er et raskt, lett verktøy for lokal utvikling.

## 3. Mål og ikke-mål

**Mål:** servere `*.test` uten manuell vhost, automatisk lokal HTTPS med egen rot-CA, flere
PHP-versjoner byttbart per prosjekt, cross-platform, lavt ressursavtrykk (< 15 MB RAM idle,
kald oppstart < 200 ms), GUI + CLI, utvidbart driver-system.

**Ikke-mål (v1):** produksjonsdrift, container-orkestrering, on-the-fly extension-kompilering,
CI/staging-erstatning.

## 7. Ikke-funksjonelle krav

- Idle RAM < 15 MB for kjerne (DNS + proxy); proxy-overhead < 1 ms p50.
- Daemon kald oppstart < 200 ms. FPM-pooler lazy, stoppes etter inaktivitet.
- Rot-CA-nøkkel med riktige filrettigheter; privilegerte operasjoner isolert.
- Daemon som OS-tjeneste, auto-restart; korrupt site-config tar ikke ned daemonen.
- Strukturert logging (tracing), `grove doctor` for diagnostikk.

## 8. Teknisk arkitektur

`grove-core`, `grove-dns`, `grove-proxy`, `grove-tls`, `grove-runtime`, `grove-services`,
`grove-os`, `grove-cli`, `grove-gui`, `grove-ipc`.

Crates: hickory-dns, hyper/tower, rustls + rcgen, clap, serde/toml, tokio, tracing, tauri.

Én langtkjørende daemon binder 80/443/53; CLI og GUI er tynne klienter over lokal IPC.

## 9. Datamodell / konfigurasjon

```toml
[general]
tld = "test"
default_php = "8.4"
auto_start = true

[[parked]]
path = "~/Code"

[[sites]]
name = "inside-next"
path = "~/Code/inside-next"
php = "8.4"
secure = true
driver = "laravel"

[[sites]]
name = "frontend"
path = "~/Code/frontend"
driver = "proxy"
proxy_to = "http://127.0.0.1:5173"
```

## 11. Faser

- **Fase 0 — Spike:** DNS + proxy ruter ett `*.test`-site til PHP-FPM via FastCGI (macOS).
- **Fase 1 — MVP (CLI-først):** park/link/list, Laravel/statisk-driver, HTTPS + CA, én
  PHP-versjon, macOS + Linux, OS-tjenesteinstall, Valet-import.
- **Fase 2 — Multi-runtime + Windows:** flere PHP-versjoner + isolate, bring-your-own PHP,
  proxy-driver, Windows, mail-catcher.
- **Fase 3 — GUI (Tauri):** dashboard, loggviewer, tjenestepanel, PHP-håndtering.
- **Fase 4 — Tjenester + økosystem:** DB/Redis-supervisor, `--json`/IPC for elyra-conductor,
  plugin-drivere.

## 14. Åpne spørsmål

1. Frittstående produkt, modul i elyra-conductor, eller begge (delt kjerne-crate)?
2. Hvor bredt utover PHP i v1 — Node/Vite-proxy MVP eller fase 2?
3. Skal DB-tjenester bundles eller kun oppdages/styres?
4. Lisens- og distribusjonsmodell?
5. Driver-system rent deklarativt (TOML) eller WASM/skript-plugins?

> Dette er en forkortet referanse. Den fulle PRD-teksten finnes i prosjektets opprinnelige
> oppdrag og i issue-trackeren.
