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
| `grove_webhooks` | Recently captured inbound webhooks. |
| `grove_logs` | List log sources, or read recent Laravel / service log entries. |
| `grove_db_schema` | Tables and columns for a site's database (read from its `.env`). |
| `grove_db_query` | Run a **read-only** SQL query and return rows. |

So you can ask your assistant things like:

- "What routes does `myapp` have a controller for, and what did the last 500
  error there look like?"
- "Show me the schema for the `orders` table in `shop`."
- "How many users signed up today?" (it runs a `SELECT` for you)
- "What webhook did Stripe just send, and did my handler 200?"

## Safety

- **Read-only.** `grove_db_query` refuses anything that isn't a `SELECT` /
  `SHOW` / `EXPLAIN` / `PRAGMA`. The server never modifies your data or files.
- **Local only.** The server talks to Grove's local daemon and your local
  databases; it makes no outbound network calls.
- Requires the Grove daemon to be running (`grove start`).

See also: [Commands](COMMANDS.md) · [Request timeline](COMMANDS.md#request-timeline).
