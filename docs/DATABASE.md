# Database client

Grove has a **built-in database client** — a Database panel in the desktop app
that browses and edits your databases without any connection setup. Because Grove
already knows your projects, it reads each site's connection details straight
from its `.env`, so there's nothing to configure.

> Not to be confused with `grove db` on the CLI, which takes **snapshots** of
> Grove's bundled databases. The Database *panel* is a full client for browsing
> and editing data. See [Commands](COMMANDS.md) for snapshots.

## Opening it

Launch the Grove app and choose **Database** in the sidebar.

## Zero-config auto-discovery

For every site Grove serves, it looks for a database configuration in the
project's `.env` (`DB_CONNECTION`, `DB_HOST`, `DB_DATABASE`, …). Each project with
a database shows up as a connection at the top of the panel — no host, port, or
password to type. Supported engines: **MySQL**, **PostgreSQL**, and **SQLite**.

## Browsing and querying (free)

- Click a connection to list its tables.
- Click a table to load its rows into the data grid (`select * from … limit 500`).
- Write your own query in the editor and press **⌘⏎** to run it.

Browsing tables and running read-only queries (`SELECT`, `SHOW`, `EXPLAIN`,
`PRAGMA`, …) is **free**.

## Free vs Pro

| | Free | **Grove Pro** |
| --- | :---: | :---: |
| Browse tables | ✅ | ✅ |
| Run `SELECT` queries | ✅ | ✅ |
| Inline row editing | | ✅ |
| Schema inspector (columns, indexes, foreign keys) | | ✅ |
| Production-safety guard | | ✅ |

Activate Pro with `grove license activate <key>` or under **Settings → License**.
See [Pro & Teams](PRO.md).

## Schema inspector (Pro)

With a table open, switch to the **Schema** tab to see its columns (name, type,
nullability, key), indexes (name, uniqueness, columns), and foreign keys
(column → referenced table.column).

## Inline row editing (Pro)

**Double-click a cell** to edit it, then press Enter to save (Escape cancels).
Grove builds a safe `UPDATE` using the table's **primary key**, so a table needs
a primary key for editing to be available.

## Production safety

Grove inspects each connection and flags any that look like **production** (for
example, a non-local host or an `APP_ENV`/database name that reads as prod). Such
connections are marked with a ⚠ badge, and **editing is disabled** on them — a
guard against fat-fingering live data. Read-only browsing still works.

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| A project doesn't appear | It has no `DB_CONNECTION` in its `.env`, or the database isn't running (start it under **Services**). |
| "unknown connection" | Reopen the Database panel to refresh the discovered connections. |
| Can't edit a cell | Editing is Pro-only, requires the table to have a primary key, and is disabled on production-looking connections. |
| Editing / schema asks to upgrade | These are Grove Pro features — activate a license. |

See also: [Pro & Teams](PRO.md) · [Configuration](CONFIGURATION.md).
