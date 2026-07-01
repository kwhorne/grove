---
title: "Grove grows a toolbox — 0.2.7 & 0.2.8"
date: 2026-07-01
author: Knut W. Horne
tags: [grove, release, tools, databases]
---

# Grove grows a toolbox — 0.2.7 & 0.2.8

There's a particular kind of joy in software that just gets out of your way. You
open it, it does the thing, and you go back to what you were actually trying to
build. That's the feeling I keep chasing with **Elyra Grove**, and these two
releases are all about smoothing over the little papercuts that stood between
you and that feeling.

Two small features, both living in a brand-new **Tools** panel: a one-click
**Restart daemon** button, and a full **database converter** that moves whole
databases between MySQL, PostgreSQL and SQLite. Let's take a walk through both.

---

## 0.2.7 — "Restart daemon", because nobody enjoys typing that command

Grove runs as a tiny background service so it can hold on to ports 53, 80 and
443 and quietly serve your `*.test` sites. That's great — until you update the
app. The freshly downloaded binary is sitting on disk, but the *running* daemon
is still the old one, happily unaware that its replacement has arrived.

Until now, the fix was a trip to the terminal:

```console
$ sudo launchctl kickstart -k system/com.elyra.grove
Password:
```

It works, but it asks for your password, it's easy to fat-finger, and honestly
you shouldn't have to remember an incantation like that just to pick up an
update.

So Grove learned to restart itself. There's now a **Restart daemon** button in
**Tools**:

```text
🛠  Tools
┌─────────────────────────────────────────────────────────────┐
│  Restart daemon                                    ↻ Service  │
│  Restarts Grove's background service. Use this after          │
│  updating the app so the running daemon picks up the new      │
│  version. No password needed.                                 │
│                                                               │
│                                          [ Restart daemon ]   │
└─────────────────────────────────────────────────────────────┘
```

Click it and… that's it. No password prompt, no terminal. The trick is a happy
little detail of how Grove is installed: the daemon already runs as root (that's
how it binds the privileged ports), so it can re-exec *itself* through
`launchctl` without asking you for anything. You click a button, the service
blinks, and a second later you're on the new version:

```console
$ grove status | head -1
Grove 0.2.7
```

The whole update loop is now: **update the app → Tools → Restart daemon → done.**

---

## 0.2.8 — Convert a database between MySQL, PostgreSQL and SQLite

This is the one I'm most excited about.

We've all been there. You've got a MySQL database full of lovingly seeded dev
data, and you want a **SQLite** copy of it — maybe to run a test suite quickly,
maybe to hand a self-contained file to a colleague, maybe just to poke at it on
a plane. Or the other direction: you've been prototyping against a SQLite file
and now you want it living in MySQL like a grown-up.

Historically that meant hunting for a dump script, wrestling with dialect
differences, and praying your dates survived the trip. Grove now does it for you.
Open **Tools → Convert database**, pick a source and a target, and press
**Convert**:

```text
🛠  Tools → Convert database

  Source                          →        Target
  ┌───────────────────────┐                ┌───────────────────────┐
  │ Type   [ MySQL     ▾] │                │ Type   [ SQLite    ▾] │
  │ Host   127.0.0.1      │                │ File   ~/shop.sqlite  │
  │ Port   3306           │                │                       │
  │ User   root           │                │                       │
  │ DB     shop           │                │                       │
  └───────────────────────┘                └───────────────────────┘

                         [ Convert ]

  ✅ Converted 14 table(s) and 3,481 row(s) into the sqlite database.
```

And there's your file, ready to go:

```console
$ sqlite3 ~/shop.sqlite '.tables'
cache            jobs             products         sessions
categories       migrations       sessions_index   users
failed_jobs      orders           order_items      ...

$ sqlite3 ~/shop.sqlite 'SELECT name, price FROM products LIMIT 3;'
Widget|9.99
Gadget|19.50
Sprocket|4.25
```

It works in every direction that matters — **MySQL → SQLite**, **SQLite →
MySQL**, and the same for PostgreSQL. It recreates your tables, columns and
primary keys, then copies every row.

### The interesting bit: making values survive the trip

The hard part of any cross-database conversion isn't the tables — it's the
*values*. Every dialect has its own opinions about dates, decimals, booleans and
binary data. Grove sidesteps most of that by transferring values as **text**
(and blobs as raw bytes), letting each database cast them back on the way in. So
a MySQL `DATETIME` lands in SQLite as `2024-01-02 03:04:05`, a `DECIMAL` keeps
its digits, and JSON and UUID columns come through intact.

Getting there meant fixing a few genuinely gnarly things under the hood — like
the fact that MySQL and PostgreSQL hand back their `information_schema` type
names as *binary* strings, and that a `LONGTEXT` column can come back as bytes
rather than a tidy `String`. Grove now reads all of that robustly, with a
bytes-to-UTF-8 fallback, so the schema it rebuilds actually matches reality.
(There's an end-to-end test that spins up a real MySQL, round-trips a table with
dates, decimals and NULLs through SQLite and back, and checks every value. It's
green. 🟢)

A note on scope: this v1 copies **tables, columns, primary keys and data**.
Views, stored routines, triggers and foreign keys aren't recreated yet — for the
vast majority of "I just want my data over there" moments, it's exactly enough.

---

## Getting the update

Grove auto-updates, so you'll get a little banner. After it relaunches:

- On **0.2.7+**: just hit **Tools → Restart daemon**.
- Coming from an older build one last time:

  ```console
  $ sudo launchctl kickstart -k system/com.elyra.grove
  ```

Then check you're current:

```console
$ grove status | head -1
Grove 0.2.8
```

---

Two small buttons, but they each remove a real bit of friction — one from
updating, one from wrangling databases. That's the whole idea: a local dev
environment that quietly handles the plumbing so you can get back to building.

As always, Grove is open source and entirely self-contained — no Homebrew, no
Composer, no separate database to install. Grab it, park a folder, and go.

Happy building. 🌳
