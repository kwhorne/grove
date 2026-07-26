# Deploy contract v1 (draft)

> **Status: draft.** Nothing implements this yet. It exists so `grove deploy` and
> the Conductor cockpit can be built against one agreed shape, the way the
> [queue/cache/pub-sub contracts](https://github.com/kwhorne/sql-anywhere/tree/main/docs/contracts)
> were agreed before Askr implemented them.

## Why this exists

Askr's [DEPLOYMENT.md](https://github.com/kwhorne/askr/blob/main/docs/DEPLOYMENT.md)
describes a zero-downtime deploy in two steps:

> 1. Put the new code in place (`rsync`, `git pull`, atomic symlink swap, …).
> 2. Reload: `systemctl reload askr`.

Step 2 is excellent and already shipped: workers roll one at a time, each draining
in flight, and `--canary` rolls a single worker first and **aborts the reload** if
it looks unhealthy — the rest keep serving the old code.

Since Askr **1.2.0** that gate is sharper, and this contract depends on the
difference:

- The canary is judged **against the rest of the fleet** from per-worker counters
  in shared memory (`canary_max_error_rate` is points *above* the fleet), not an
  absolute 5xx count. Before 1.2.0 a site with any error baseline aborted every
  reload while a canary serving no traffic passed.
- A failed canary is **drained and its slot quarantined**, so it does not keep
  serving a broken build from 1/N of the fleet.
- The outcome is readable from `/api/status` as `rollout`:
  `rolling` | `ok` | `aborted` | `inconclusive`.
- `inconclusive` means the canary saw fewer than `canary_min_requests`. Askr rolls
  on with a warning — **so `grove deploy` must not treat it as success**. See below.

Step 1 is a parenthesis. It is the only place in the ecosystem where the answer is
"do it yourself, somehow". This contract makes step 1 a product.

**Askr owns the last ten metres. `grove deploy` owns everything before the reload,
and the decision to roll back after it.**

## Scope

| | v0.1 |
| --- | --- |
| Target | One Linux box, reachable over SSH, **with Askr already installed and running** |
| App | A PHP/Laravel app served by Askr |
| Out of scope | Provisioning a fresh box, multiple servers, database provisioning, TLS issuance (Askr's ACME already does it), supervising queues/scheduler (Askr already does it) |

Provisioning is deliberately v0.2. It is the larger and rarer job; releasing is the
daily one, and it can ship first without constraining how provisioning later works.

## Release layout

```
/srv/<app>/
├── releases/
│   ├── 20260726T081500Z-a1b2c3d/     ← immutable; never written to after the swap
│   └── 20260726T093000Z-e4f5a6b/
├── shared/
│   ├── .env                          ← the only secret on the box
│   ├── storage/                      ← Laravel storage, symlinked into each release
│   └── deploys/                      ← one JSON record per deploy (see Cockpit)
└── current -> releases/20260726T093000Z-e4f5a6b
```

Normative:

- A release directory is named `<UTC timestamp>-<git sha>` and is **immutable**
  once the swap happens. Anything that must survive a release lives in `shared/`.
- `current` is a symlink, swapped with `rename(2)` so no request ever observes a
  half-written state.
- The document root Askr serves is `current/public`. Askr must resolve the symlink
  per request, or be reloaded on swap — the reload happens anyway, so either works.

Changing this layout later breaks every existing box, so it is fixed by this
contract rather than by the first implementation.

## The steps

| # | Step | Fails how |
| --- | --- | --- |
| 1 | **Preflight** — SSH reachable, Askr healthy, disk space, `shared/.env` present, git sha resolvable | Abort before touching the server |
| 2 | **Upload** — new release directory (rsync from a local build, or fetch on the box) | Abort; remove the partial directory |
| 3 | **Build** — `composer install --no-dev`, asset build if not built locally | Abort; remove the release directory |
| 4 | **Link shared** — symlink `shared/.env` and `shared/storage` into the release | Abort; remove the release directory |
| 5 | **Migrate** — `php artisan migrate --force` | **Abort. See below.** |
| 6 | **Swap** — atomic `rename` of `current` | Abort; previous `current` is untouched |
| 7 | **Reload** — Askr canary reload, then read `rollout` from `/api/status` | `aborted` → **auto-rollback** (step 8). `inconclusive` → continue to verify, and say so in the record |
| 8 | **Verify** — health endpoint returns 200 through the real listener | Failure → auto-rollback |
| 9 | **Prune** — keep the last N releases (default 5) | Never fails the deploy |

### The migration decision

Migrations run at step 5, **before** the swap, while the previous release is still
serving.

This is the choice that defines the product, so it is stated plainly:

> **A rollback restores code. It never restores data.**

Therefore a migration must be backward-compatible with the release currently
running — the expand/contract discipline: add a nullable column now, backfill,
switch the code, drop the old column in a *later* deploy. `grove deploy` cannot
verify this, and must not pretend to.

The alternative — migrating after the swap — buys nothing: a failure then leaves a
new schema serving old code, which is strictly worse. The third option, migrating
inside a transaction and rolling back, is a lie on MySQL, where DDL commits
implicitly.

What `grove deploy` **can** do is refuse to be surprised: `--dry-run` reports the
pending migrations before anything is uploaded, and `--no-migrate` exists for the
deploys that must not touch the schema.

### Rollback

```bash
grove deploy rollback [--to <release>]
```

Repoints `current` at the previous release and reloads. It is the same mechanism
the canary abort triggers automatically, so the automatic and manual paths cannot
drift apart.

Rollback is **code only**. If a deploy migrated, the operator is told so
explicitly in the rollback output rather than left to discover it.

### What a canary abort actually protects, per mode

Askr documents this honestly and so must we:

| Askr mode | On abort |
| --- | --- |
| **Worker mode** | Surviving workers hold the previous app *in memory*, so old code really does keep serving. Repointing `current` makes that permanent. |
| **Per-request mode** | Every worker reads current files from disk. The gate detects and drains the canary, but cannot roll back code that is no longer on disk — **only the symlink swap can**. |

So in per-request mode the symlink is the rollback, and `grove deploy` owning it is
not a convenience: it is the mechanism. This is the strongest argument for the
immutable-release layout above.

## Secrets

The `.env` lives at `shared/.env` and is never part of a release, never printed,
and never held in the deploy log.

```bash
grove deploy env pull        # fetch to a local, gitignored, encrypted file
grove deploy env push        # replace shared/.env, keeping a timestamped backup
grove deploy env diff        # key-level difference; values are never shown
```

`env push` is a separate verb from `deploy`, because pushing configuration and
releasing code fail differently and should be recoverable independently.

Non-goal for v0.1: a secret store. The point here is only that a secret has one
home and is not scattered by `scp`.

## The cockpit's read model

Conductor must not need SSH plumbing knowledge to show a deploy. Each deploy
writes one JSON record to `shared/deploys/<id>.json`, appended-to as it runs:

```json
{
  "id": "20260726T093000Z-e4f5a6b",
  "app": "elyracode",
  "sha": "e4f5a6b",
  "actor": "kh",
  "started_at": "2026-07-26T09:30:00Z",
  "finished_at": "2026-07-26T09:30:47Z",
  "outcome": "succeeded",
  "rollout": "ok",
  "rolled_back_from": null,
  "migrations": ["2026_07_20_120000_add_domains_to_licenses"],
  "steps": [
    { "name": "preflight", "status": "ok", "ms": 310 },
    { "name": "upload",    "status": "ok", "ms": 4120 },
    { "name": "migrate",   "status": "ok", "ms": 890 },
    { "name": "swap",      "status": "ok", "ms": 12 },
    { "name": "reload",    "status": "ok", "ms": 6300, "note": "canary healthy" }
  ]
}
```

Normative: `outcome` is one of `succeeded`, `failed`, `rolled_back`. `rollout`
mirrors Askr's own value (`ok`, `aborted`, `inconclusive`) so the cockpit can show
"deployed, but the canary saw too little traffic to judge" — which is neither a
success worth trusting nor a failure worth waking up for. A record is written even
when preflight fails, so a failed deploy is visible rather than absent. Records are
pruned with their releases.

This is a file, not a database, so a deploy never depends on the app's database
being reachable — which is exactly the situation you are in when you most need to
read the log.

## Explicit non-goals for v0.1

Listed so the first version can actually ship:

- Provisioning a bare server (v0.2)
- More than one target host per app
- Blue/green or traffic-splitting beyond Askr's canary
- Scheduled or webhook-triggered deploys
- Rolling back data
- A hosted control plane. This deploys **from your machine to your box**; there is
  no third party in the path, which is the point.

## Open questions

1. Does the release build happen locally and ship as an artifact, or on the box?
   Local is reproducible and keeps build tools off production; on-box is simpler
   and survives a slow uplink. v0.1 leans local, with `--build=remote` as an
   escape hatch.
2. Should `grove deploy` know about Askr's `askr.toml`, or leave server config
   entirely to the box? Leaning: leave it, and revisit with provisioning.
3. Where does the app's identity live — a `grove.deploy.toml` in the repo, or
   Grove's own config? Leaning: in the repo, so a deploy is reproducible by anyone
   who clones it.
