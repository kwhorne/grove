<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { DbConnInfo, DbQueryResult, LicenseClaims } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  let connections = $state<DbConnInfo[]>([]);
  let active = $state<DbConnInfo | null>(null);
  let tables = $state<string[]>([]);
  let sql = $state("");
  let result = $state<DbQueryResult | null>(null);
  let error = $state("");
  let busy = $state(false);
  let license = $state<LicenseClaims | null>(null);

  let isPro = $derived(license?.plan === "pro" || license?.plan === "teams");

  onMount(async () => {
    try {
      license = await api.licenseStatus();
      connections = await api.dbConnections();
      if (connections.length) void select(connections[0]);
    } catch (e) {
      error = String(e);
    }
  });

  async function select(conn: DbConnInfo) {
    active = conn;
    result = null;
    error = "";
    tables = [];
    try {
      tables = await api.dbTables(conn.key);
    } catch (e) {
      error = String(e);
    }
  }

  async function openTable(t: string) {
    sql = `select * from ${t} limit 500`;
    await run();
  }

  async function run() {
    if (!active || !sql.trim()) return;
    busy = true;
    error = "";
    try {
      result = await api.dbQuery(active.key, sql.trim());
    } catch (e) {
      error = String(e);
      result = null;
    }
    busy = false;
  }

  function onKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      void run();
    }
  }
</script>

<div class="db">
  {#if connections.length === 0}
    <p class="empty">
      No databases found. Grove reads each site's <span class="mono">.env</span> —
      link a project with a database, or start one under <b>Services</b>.
    </p>
  {:else}
    <div class="bar">
      <div class="conns">
        {#each connections as c (c.key)}
          <button class="conn {active?.key === c.key ? 'active' : ''}" onclick={() => select(c)}>
            <span class="eng {c.engine}">{c.engine}</span>
            <span class="cname">{c.label}</span>
            {#if c.is_prod}<span class="prod" title="Looks like production">⚠ prod</span>{/if}
          </button>
        {/each}
      </div>
      {#if !isPro}
        <span class="ro">Read-only · <a href="https://elyracode.com/grove" target="_blank" rel="noreferrer">Grove Pro</a> to edit</span>
      {/if}
    </div>

    {#if active?.is_prod}
      <div class="warn">⚠ This connection looks like <b>production</b>. Be careful — changes here affect live data.</div>
    {/if}

    <div class="work">
      <aside class="tables">
        <div class="thead">Tables ({tables.length})</div>
        <div class="tlist">
          {#each tables as t (t)}
            <button class="titem" onclick={() => openTable(t)}>{t}</button>
          {/each}
        </div>
      </aside>

      <div class="main">
        <div class="editor">
          <textarea
            class="sql"
            bind:value={sql}
            placeholder="select * from users limit 100   —   ⌘⏎ to run"
            onkeydown={onKeydown}
            spellcheck="false"
          ></textarea>
          <button class="btn primary" onclick={run} disabled={busy || !sql.trim()}>
            {busy ? "Running…" : "Run ⌘⏎"}
          </button>
        </div>

        {#if error}
          <div class="err">{error}</div>
        {:else if result}
          <div class="meta">
            {#if result.is_select}
              {result.rows.length} row{result.rows.length === 1 ? "" : "s"}
            {:else}
              {result.rows_affected ?? 0} affected
            {/if}
            · {result.elapsed_ms}ms{result.truncated ? " · truncated" : ""}
          </div>
          <div class="grid">
            <table>
              <thead>
                <tr>{#each result.columns as col}<th>{col}</th>{/each}</tr>
              </thead>
              <tbody>
                {#each result.rows as row}
                  <tr>
                    {#each row as cell}
                      <td class:null={cell === null} title={cell ?? "NULL"}>{cell ?? "NULL"}</td>
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else}
          <p class="hint">Pick a table on the left, or write a query and press ⌘⏎.</p>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .db { display: flex; flex-direction: column; gap: 12px; height: calc(100vh - 190px); }
  .empty, .hint { color: var(--text-dim); font-size: 13px; }
  .bar { display: flex; align-items: center; gap: 12px; }
  .conns { display: flex; gap: 6px; flex-wrap: wrap; }
  .conn {
    display: flex; align-items: center; gap: 7px;
    background: var(--panel); border: 1px solid var(--border); color: var(--text);
    border-radius: 8px; padding: 6px 10px; font: inherit; font-size: 12px; cursor: pointer;
  }
  .conn:hover { border-color: var(--accent); }
  .conn.active { border-color: var(--brand); }
  .eng { font-family: var(--font-mono); font-size: 10px; text-transform: uppercase; color: var(--text-dim); }
  .eng.mysql { color: #f29111; } .eng.postgres { color: #3b82f6; } .eng.sqlite { color: #10b981; }
  .cname { font-weight: 500; }
  .prod { color: var(--red); font-size: 11px; font-weight: 600; }
  .ro { margin-left: auto; font-size: 12px; color: var(--text-dim); }
  .ro a { color: var(--brand); }
  .warn { background: color-mix(in srgb, var(--red) 12%, transparent); border: 1px solid var(--red); color: var(--text); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .work { display: flex; gap: 12px; flex: 1; min-height: 0; }
  .tables { width: 190px; border: 1px solid var(--border); border-radius: 10px; display: flex; flex-direction: column; overflow: hidden; }
  .thead { padding: 8px 10px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.14em; color: var(--text-dim); border-bottom: 1px solid var(--border); }
  .tlist { overflow-y: auto; }
  .titem { display: block; width: 100%; text-align: left; background: none; border: 0; color: var(--text); padding: 6px 10px; font: inherit; font-size: 13px; cursor: pointer; font-family: var(--font-mono); }
  .titem:hover { background: var(--panel); color: var(--brand); }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 10px; }
  .editor { display: flex; gap: 8px; align-items: stretch; }
  .sql { flex: 1; resize: vertical; min-height: 60px; background: var(--bg); border: 1px solid var(--border); border-radius: 8px; color: var(--text); font-family: var(--font-mono); font-size: 13px; padding: 10px; }
  .btn { background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 8px 14px; font: inherit; font-size: 13px; cursor: pointer; white-space: nowrap; }
  .btn.primary { background: var(--brand); border-color: var(--brand); color: #1a1015; font-weight: 600; }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .meta { font-size: 12px; color: var(--text-dim); }
  .err { background: color-mix(in srgb, var(--red) 12%, transparent); border: 1px solid var(--red); color: var(--text); border-radius: 8px; padding: 10px 12px; font-size: 13px; font-family: var(--font-mono); }
  .grid { flex: 1; min-height: 0; overflow: auto; border: 1px solid var(--border); border-radius: 10px; }
  .grid table { border-collapse: collapse; font-size: 12px; width: max-content; min-width: 100%; }
  .grid th { position: sticky; top: 0; background: var(--panel); text-align: left; padding: 7px 10px; border-bottom: 1px solid var(--border); color: var(--text-dim); font-weight: 500; white-space: nowrap; }
  .grid td { padding: 6px 10px; border-bottom: 1px solid var(--border); white-space: nowrap; max-width: 320px; overflow: hidden; text-overflow: ellipsis; }
  .grid td.null { color: var(--text-dim); font-style: italic; }
</style>
