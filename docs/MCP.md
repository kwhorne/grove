# AI tools (MCP server)

Grove ships a built-in [Model Context Protocol](https://modelcontextprotocol.io)
server, so AI tools like **Claude** and **Cursor** can see your local
environment and answer questions about it — live. Because Grove already knows
your sites, proxies every request, and can read each project's database, it can
hand an AI assistant a rich, accurate picture of what's actually running on your
machine, with **zero per-project setup**.

It's read-only and runs entirely on your machine — nothing is sent anywhere
except to the AI client you connect.

## Start it

```bash
grove mcp
```

That runs an MCP server over stdio (newline-delimited JSON-RPC). You normally
don't run it by hand — your AI client launches it for you.

## Connect your client

**Claude Desktop** — edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "grove": { "command": "grove", "args": ["mcp"] }
  }
}
```

**Cursor** — add an MCP server in settings, or `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "grove": { "command": "grove", "args": ["mcp"] }
  }
}
```

If `grove` isn't on your `PATH`, use the full path to the binary (for the desktop
app that's `/Applications/Grove.app/Contents/MacOS/grove`, or install the shims
with `grove path install` so `grove` resolves everywhere). Restart the client
after editing its config.

## What the AI can do

| Tool | What it answers |
| --- | --- |
| `grove_sites` | Every site Grove serves (host, driver, PHP/Node, HTTPS, path). |
| `grove_requests` | Recent requests across sites — method, path, status, duration. |
| `grove_request` | Full headers + body of one captured request. |
| `grove_request_chain` | The causal chain for one request — the SQL it issued (with `grove sql-capture on`) and mail it sent within its time window, plus derived metrics (duration, query count). |
| `grove_explain` | A curated debugging bundle for one request — the request (headers + body), its causal chain, and matching error-log entries with stacktraces. Everything needed to explain a failing request. |
| `grove_webhooks` | Recently captured inbound webhooks. |
| `grove_logs` | List log sources, or read recent Laravel / service log entries. |
| `grove_db_schema` | Tables and columns for a site's database (read from its `.env`). |
| `grove_db_query` | Run a **read-only** SQL query and return rows. |

So you can ask your assistant things like:

- "What routes does `myapp` have a controller for, and what did the last 500
  error there look like?"
- "Show me the schema for the `orders` table in `shop`."
- "Show me the full chain for request #42 — how many queries did it run and did it send any email?" (turn on `grove sql-capture on` first for SQL)
- "Explain request #42 — why did it 500?" (uses `grove_explain` to pull the request, its queries, and the stacktrace together)
- "How many users signed up today?" (it runs a `SELECT` for you)
- "What webhook did Stripe just send, and did my handler 200?"

## Agent-safe write tools (opt-in)

By default the server is **read-only**. Start it with `--allow-write` to expose
tools that can change state — each one wrapped in a safety net so an agent can
never leave your database in a broken state:

```bash
grove mcp --allow-write
```

| Tool | What it does |
| --- | --- |
| `grove_migrate_sandboxed` | Runs `php artisan <command>` (default `migrate --force`) inside an **automatic snapshot sandbox**. |
| `grove_sql_sandboxed` | Runs a **write** SQL statement (INSERT/UPDATE/DELETE/DDL) inside the same snapshot sandbox. Read-only statements are refused — use `grove_db_query`. |

Both support **MySQL**, **PostgreSQL** and **SQLite**.

How `grove_migrate_sandboxed` works:

1. Grove takes a point-in-time **snapshot** of the site's database first.
2. It runs the migration and captures the **schema diff** (added/removed tables
   and columns).
3. If the command **fails**, Grove **automatically rolls back** to the snapshot —
   your data is untouched.
4. Pass `roll_back: true` for a pure **dry run**: it runs, reports what the
   migration *would* change, then rolls back even on success.
5. On success it keeps the change and returns the `snapshot_id` so you can roll
   back manually at any time.

Works with **MySQL**, **PostgreSQL** (snapshotted via Grove's daemon) and
**SQLite** (snapshotted by copying the `.sqlite` file). Every write operation is
appended to an audit log at `$GROVE_HOME/logs/mcp-writes.log` (what ran, when,
the snapshot id, and the outcome).

`grove_sql_sandboxed` follows the exact same snapshot → run → diff → rollback
flow for a single write statement, returning `rows_affected` alongside the schema
diff.

So you can safely ask: *"Add the migration for the `invoices` table and apply it"*,
*"Do a dry run of `migrate:fresh` and show me what tables it would create,"* or
*"Backfill `users.status` to 'active' where it's null — but roll back so I can
review first."*

## Safety

- **Read-only by default.** Write tools appear only with `grove mcp
  --allow-write`, and even then the server refuses them if the flag is off.
- **Sandboxed writes.** Every write goes through a snapshot with automatic
  rollback on failure, plus an audit log.
- `grove_db_query` refuses anything that isn't a `SELECT` /
  `SHOW` / `EXPLAIN` / `PRAGMA`.
- **Local only.** The server talks to Grove's local daemon and your local
  databases; it makes no outbound network calls.
- Requires the Grove daemon to be running (`grove start`).

See also: [Commands](COMMANDS.md) · [Request timeline](COMMANDS.md#request-timeline).
