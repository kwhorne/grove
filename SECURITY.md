# Security Policy

## Supported versions

Security fixes target the latest released version.

| Version | Supported |
| --- | --- |
| 1.5.x | ✅ |
| < 1.5 | ❌ |

## Reporting a vulnerability

**Please do not report security issues in public GitHub issues.**

Use GitHub's private vulnerability reporting:
[Report a vulnerability](https://github.com/kwhorne/grove/security/advisories/new),
or email **security@kwhorne.com**.

Please include:

- a description of the issue and its impact,
- steps to reproduce (a proof of concept if possible),
- the Grove version and your OS.

You can expect an initial response within a few days. Once a fix is ready we'll
coordinate disclosure and credit you in the release notes if you wish.

## The threat model

Grove is a local development environment. Its interesting property is that a
small part of it runs as **root** — to bind ports 53/80/443, install the system
resolver, and add a CA to the trust store — while everything it supervises runs
as **you**. Most of what is worth attacking follows from that split, so the
boundaries are documented in
[docs/ARCHITECTURE.md § Trust boundaries](docs/ARCHITECTURE.md#trust-boundaries).

The assumption throughout is that a local user who is *already* root has won;
the boundaries defend against a non-root local user, a hostile project directory,
and a network attacker who can answer for a download host.

## Security-sensitive areas

Worth extra scrutiny, in rough order of blast radius:

- **The root daemon's IPC socket** — every privileged operation is reachable
  through it, so it is the primary authorization boundary (peer-credential
  checked, `0o660`, owner-restricted).
- **Privilege dropping** — the daemon spawns PHP-FPM, databases and Redis; each
  must land as the invoking user with root's supplementary groups gone.
- **Anything root reads out of `$GROVE_HOME`** — config, `php-builds.json`,
  registered binaries. A file that names a binary root will execute is as
  sensitive as the binary.
- **The local root CA** — its private key, and the `NameConstraints` that keep a
  leaked CA from signing anything outside the configured TLD.
- **Downloaded runtimes and services** — PHP, Node, Composer, cpx and the
  databases are fetched and then executed; see below for what is verified.
- **Team secret sync** — client-side age/X25519 encryption, and the local pin
  that stops a backend from adding its own recipient.
- **The tunnel server** (`grove-tunnel`) — the one component intended to face the
  public internet.

## Known-unverified downloads

Not every publisher gives us something to check against. These are shipped with
the gap documented rather than papered over, and a report that one of them is
unverified will be closed as known:

| Source | Why |
| --- | --- |
| static-php-cli upstream archives | no checksum published at all |
| Redis git-archive tarball | GitHub does not promise stable archive bytes |
| MySQL | publishes only `.md5` and a GPG signature |

Grove's own PHP builds, Node, Composer, cpx and the PostgreSQL binaries are
SHA-256 verified before use. Note what that proves: the hash comes from the same
publisher over the same TLS session, so it detects corruption, truncation and a
tampered artefact — not a publisher whose account is taken over.

## Out of scope

- Anything requiring root on the developer's own machine to begin with.
- The trust prompt on first run: Grove asks to install a CA, and installing one
  is the feature.
- Denial of service against a machine's own dev environment.
