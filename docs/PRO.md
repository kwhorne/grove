# Grove Pro & Teams

Everything you need for local development is **free and open source, forever** —
serving `*.test` with HTTPS, bundled PHP/Node/databases, mail, tunnels, the
request timeline, and database snapshots. The core is never gated.

**Grove Pro** adds the layer that appears the moment you stop working alone:
shared, synced team infrastructure.

- **End-to-end encrypted team secret sync** — a project's `.env`, shared securely,
  never pasted into a chat window.
- Priority support and a commercial license.

Pricing is **$99 per seat, per year**. Buy as many seats as your team needs and
expand any time.

---

## 1. Buy — no account needed first

Pick your number of seats at [elyracode.com/grove](https://elyracode.com/grove)
and check out. Your account is created **on payment** — there's no separate
sign-up. The moment your payment clears you receive one email with:

- your **license key**, and
- your **login** (email + a temporary password) for the customer portal.

Manage your license, add seats, download invoices, or change payment details any
time from the portal (**Manage billing & seats** uses Stripe's hosted portal).

---

## 2. Activate

Paste the key into the desktop app under **Settings → License**, or from the
terminal:

```bash
grove license activate GROVE-…
```

```text
✓ Grove Teams active
  seats  : 4
  email  : you@yourteam.com
  renews : in 364 days
```

Other commands:

| Command | Description |
| --- | --- |
| `grove license status` | Show the current entitlement. |
| `grove license deactivate` | Remove the stored license. |

Verification is **offline** — the key is checked against a public key baked into
the app, so Pro features keep working without a connection.

---

## 3. Team secret sync

Share a project's `.env` across your team, encrypted end-to-end. Secrets are
encrypted **on your machine** to your teammates' public keys; the backend only
ever stores ciphertext.

### Your identity

The first time you use secrets, Grove creates a personal key pair at
`~/.grove/identity` (the private half never leaves your machine). Share your
**public** key so teammates can grant you access:

```bash
grove secret whoami
# age1q9…                ← your public key
```

### Setting and pulling secrets

```bash
# Set a secret (encrypted + pushed):
grove secret set myapp DB_PASSWORD=super-secret

# Fetch + decrypt (print, or write a .env):
grove secret pull myapp
grove secret pull myapp --write     # writes ./.env
```

### Inviting teammates

A teammate runs `grove secret whoami` and sends you their public key. You grant
access — Grove re-encrypts the secrets to include them:

```bash
grove secret share myapp age1teammatekey…
grove secret members myapp            # who has access
grove secret revoke myapp age1teammatekey…   # remove + re-encrypt
```

### A typical team workflow

```bash
# You (project owner):
grove secret set myapp APP_KEY=base64:…
grove secret set myapp DB_PASSWORD=…

# New teammate:
grove secret whoami                   # copy your public key, send it to the owner

# You:
grove secret share myapp <their-key>

# Teammate, after cloning the repo:
grove secret pull myapp --write       # .env is ready — app runs
```

---

## 4. Security model

- **End-to-end encryption.** Secrets are encrypted client-side with `age`
  (X25519) to the current members' public keys. Only someone holding a member
  private key can decrypt — the server cannot.
- **Zero-knowledge backend.** The hosted service stores only ciphertext and
  public keys. Removing a member re-encrypts without their key, so they lose
  access on the next change.
- **Offline license verification.** Licenses are Ed25519-signed by the store and
  verified against a baked-in public key — no phone-home for daily use.
- **Server-side enforcement.** The backend independently verifies the license
  signature, checks it is an active Teams license, and enforces the seat count —
  so the open-source client can be inspected freely without weakening security.

---

## 5. Self-hosting / custom backend

The client talks to `https://teams.elyracode.com` by default. Point it elsewhere
with an environment variable:

```bash
export GROVE_TEAMS_SERVER=https://teams.example.com
```

---

## 6. Troubleshooting

| Symptom | Fix |
| --- | --- |
| `no license found` | Run `grove license activate <key>` first. |
| `this is a Grove Teams feature` | Your license is Pro (solo); Teams is required for secret sync. |
| `not a member of "…"` | You haven't been granted access — ask an owner to `grove secret share` your `grove secret whoami` key. |
| Backend `401` | Your license is invalid or expired — check `grove license status`. |
| Backend `402` on share | You've hit your seat limit — add seats from the portal. |

---

## FAQ

**Does buying Pro change anything about the free version?** No. The free,
open-source core is exactly the same, forever. Pro is purely additive.

**What happens when my license expires?** Pro features stop unlocking; the free
core keeps working. Renew from the portal to restore them.

**Where do my secrets live?** Encrypted on the backend (ciphertext only) and, when
you `pull --write`, in your project's `.env`. Never commit `.env` to git.

See also: [Commands](COMMANDS.md) · [Architecture](ARCHITECTURE.md).
