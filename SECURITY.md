# Security Policy

## Supported versions

Grove is pre-1.0. Security fixes target the latest released version on `main`.

| Version | Supported |
| --- | --- |
| 0.1.x | ✅ |
| < 0.1 | ❌ |

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

## Security-sensitive areas

Grove performs a few privileged operations worth extra scrutiny:

- the local **root CA** and per-site leaf certificate issuance,
- the **DNS resolver** (it answers only the configured TLD and never acts as an
  open resolver),
- the minimal **elevated step** used for binding ports 80/443/53 and installing
  the resolver / trust store,
- **bring-your-own PHP** binaries and downloaded runtimes/services.
