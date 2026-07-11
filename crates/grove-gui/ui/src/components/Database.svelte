<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { highlightSql } from "../lib/sqlHighlight";
  import type {
    DbConnInfo,
    DbQueryResult,
    LicenseClaims,
    ColumnInfo,
    IndexRow,
    FkRow,
    PkPair,
  } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  let connections = $state<DbConnInfo[]>([]);
  let active = $state<DbConnInfo | null>(null);
  let tables = $state<string[]>([]);
  let currentTable = $state<string | null>(null);
  let view = $state<"data" | "schema">("data");
  let sql = $state("");
  let result = $state<DbQueryResult | null>(null);
  let columns = $state<ColumnInfo[]>([]);
  let indexes = $state<IndexRow[]>([]);
  let fks = $state<FkRow[]>([]);
  let error = $state("");
  let busy = $state(false);
  let license = $state<LicenseClaims | null>(null);

  // Inline cell editing state.
  let editing = $state<{ r: number; c: number } | null>(null);
  let editVal = $state("");

  // SQL editor highlight overlay.
  let hl = $derived(highlightSql(sql));
  let sqlEl = $state<HTMLTextAreaElement | null>(null);
  let preEl = $state<HTMLPreElement | null>(null);
  function syncScroll() {
    if (sqlEl && preEl) {
      preEl.scrollTop = sqlEl.scrollTop;
      preEl.scrollLeft = sqlEl.scrollLeft;
    }
  }

  let isPro = $derived(license?.plan === "pro" || license?.plan === "teams");
  let pkCols = $derived(
    columns.filter((c) => /pri|pk/i.test(c.key)).map((c) => c.name),
  );
  let canEdit = $derived(
    isPro && !!currentTable && pkCols.length > 0 && !active?.is_prod,
  );

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
    currentTable = null;
    columns = [];
    try {
      tables = await api.dbTables(conn.key);
    } catch (e) {
      error = String(e);
    }
  }

  async function openTable(t: string) {
    currentTable = t;
    view = "data";
    sql = `select * from ${t} limit 500`;
    columns = [];
    if (isPro && active) {
      try {
        columns = await api.dbColumns(active.key, t);
      } catch {
        /* schema is Pro; ignore for free */
      }
    }
    await run();
  }

  async function run() {
    if (!active || !sql.trim()) return;
    busy = true;
    error = "";
    editing = null;
    try {
      result = await api.dbQuery(active.key, sql.trim());
    } catch (e) {
      error = String(e);
      result = null;
    }
    busy = false;
  }

  async function loadSchema() {
    if (!active || !currentTable) return;
    view = "schema";
    error = "";
    try {
      columns = await api.dbColumns(active.key, currentTable);
      indexes = await api.dbIndexes(active.key, currentTable);
      fks = (await api.dbForeignKeys(active.key)).filter((f) => f.table === currentTable);
    } catch (e) {
      error = String(e);
    }
  }

  function startEdit(r: number, c: number) {
    if (!canEdit || !result) return;
    editing = { r, c };
    editVal = result.rows[r][c] ?? "";
  }

  async function saveEdit() {
    if (!editing || !result || !active || !currentTable) return;
    const { r, c } = editing;
    const column = result.columns[c];
    const pk: PkPair[] = pkCols.map((name) => {
      const idx = result!.columns.indexOf(name);
      return [name, idx >= 0 ? result!.rows[r][idx] : null];
    });
    try {
      await api.dbUpdateCell(active.key, currentTable, column, editVal, pk);
      result.rows[r][c] = editVal;
      notify(`Updated ${column}`);
    } catch (e) {
      notify(String(e));
    }
    editing = null;
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
        <span class="ro">Read-only · <a href="https://elyracode.com/grove" target="_blank" rel="noreferrer">Grove Pro</a> to edit &amp; inspect</span>
      {/if}
    </div>

    {#if active?.is_prod}
      <div class="warn">⚠ This connection looks like <b>production</b>. Editing is disabled here to protect live data.</div>
    {/if}

    <div class="work">
      <aside class="tables">
        <div class="thead">Tables ({tables.length})</div>
        <div class="tlist">
          {#each tables as t (t)}
            <button class="titem {currentTable === t ? 'sel' : ''}" onclick={() => openTable(t)}>{t}</button>
          {/each}
        </div>
      </aside>

      <div class="main">
        <div class="editor">
          <div class="sqlwrap">
            <pre class="hl" bind:this={preEl} aria-hidden="true">{@html hl}</pre>
            <textarea
              class="sql"
              bind:this={sqlEl}
              bind:value={sql}
              onscroll={syncScroll}
              placeholder="select * from users limit 100   —   ⌘⏎ to run"
              onkeydown={onKeydown}
              spellcheck="false"
            ></textarea>
          </div>
          <div class="ebtns">
            <button class="btn primary" onclick={run} disabled={busy || !sql.trim()}>
              {busy ? "Running…" : "Run ⌘⏎"}
            </button>
            {#if currentTable}
              <button class="btn" class:on={view === "data"} onclick={() => (view = "data")}>Data</button>
              <button class="btn" class:on={view === "schema"} onclick={loadSchema}>Schema</button>
            {/if}
          </div>
        </div>

        {#if error}
          <div class="err">{error}</div>
        {:else if view === "schema" && currentTable}
          {#if !isPro}
            <p class="hint">🔒 Schema inspection is a <a href="https://elyracode.com/grove" target="_blank" rel="noreferrer">Grove Pro</a> feature.</p>
          {:else}
            <div class="schema">
              <div class="sblock">
                <h4>Columns</h4>
                <table>
                  <thead><tr><th>Column</th><th>Type</th><th>Null</th><th>Key</th></tr></thead>
                  <tbody>
                    {#each columns as col}
                      <tr>
                        <td class="mono">{col.name}</td>
                        <td class="dim">{col.data_type}</td>
                        <td class="dim">{col.nullable ? "yes" : "no"}</td>
                        <td>{#if col.key}<span class="key">{col.key}</span>{/if}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
              {#if indexes.length}
                <div class="sblock">
                  <h4>Indexes</h4>
                  <table>
                    <thead><tr><th>Name</th><th>Unique</th><th>Columns</th></tr></thead>
                    <tbody>
                      {#each indexes as ix}
                        <tr><td class="mono">{ix.name}</td><td class="dim">{ix.unique ? "yes" : "no"}</td><td class="mono">{ix.columns.join(", ")}</td></tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {/if}
              {#if fks.length}
                <div class="sblock">
                  <h4>Foreign keys</h4>
                  <table>
                    <thead><tr><th>Column</th><th>References</th></tr></thead>
                    <tbody>
                      {#each fks as fk}
                        <tr><td class="mono">{fk.column}</td><td class="mono dim">{fk.ref_table}.{fk.ref_column}</td></tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {/if}
            </div>
          {/if}
        {:else if result}
          <div class="meta">
            {#if result.is_select}
              {result.rows.length} row{result.rows.length === 1 ? "" : "s"}
            {:else}
              {result.rows_affected ?? 0} affected
            {/if}
            · {result.elapsed_ms}ms{result.truncated ? " · truncated" : ""}
            {#if canEdit}<span class="edithint"> · double-click a cell to edit</span>{/if}
          </div>
          <div class="grid">
            <table>
              <thead>
                <tr>{#each result.columns as col}<th>{col}</th>{/each}</tr>
              </thead>
              <tbody>
                {#each result.rows as row, r}
                  <tr>
                    {#each row as cell, c}
                      {#if editing && editing.r === r && editing.c === c}
                        <td class="edit">
                          <!-- svelte-ignore a11y_autofocus -->
                          <input
                            bind:value={editVal}
                            autofocus
                            onblur={saveEdit}
                            onkeydown={(e) => { if (e.key === "Enter") saveEdit(); if (e.key === "Escape") editing = null; }}
                          />
                        </td>
                      {:else}
                        <td class:null={cell === null} class:editable={canEdit}
                            title={cell ?? "NULL"}
                            ondblclick={() => startEdit(r, c)}>{cell ?? "NULL"}</td>
                      {/if}
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
  .hint a, .ro a { color: var(--brand); }
  .bar { display: flex; align-items: center; gap: 12px; }
  .conns { display: flex; gap: 6px; flex-wrap: wrap; }
  .conn { display: flex; align-items: center; gap: 7px; background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 6px 10px; font: inherit; font-size: 12px; cursor: pointer; }
  .conn:hover { border-color: var(--accent); }
  .conn.active { border-color: var(--brand); }
  .eng { font-family: var(--font-mono); font-size: 10px; text-transform: uppercase; color: var(--text-dim); }
  .eng.mysql { color: #f29111; } .eng.postgres { color: #3b82f6; } .eng.sqlite { color: #10b981; }
  .cname { font-weight: 500; }
  .prod { color: var(--red); font-size: 11px; font-weight: 600; }
  .ro { margin-left: auto; font-size: 12px; color: var(--text-dim); }
  .warn { background: color-mix(in srgb, var(--red) 12%, transparent); border: 1px solid var(--red); color: var(--text); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .work { display: flex; gap: 12px; flex: 1; min-height: 0; }
  .tables { width: 190px; border: 1px solid var(--border); border-radius: 10px; display: flex; flex-direction: column; overflow: hidden; }
  .thead { padding: 8px 10px; font-size: 11px; text-transform: uppercase; letter-spacing: 0.14em; color: var(--text-dim); border-bottom: 1px solid var(--border); }
  .tlist { overflow-y: auto; }
  .titem { display: block; width: 100%; text-align: left; background: none; border: 0; color: var(--text); padding: 6px 10px; font: inherit; font-size: 13px; cursor: pointer; font-family: var(--font-mono); }
  .titem:hover, .titem.sel { background: var(--panel); color: var(--brand); }
  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 10px; }
  .editor { display: flex; gap: 8px; align-items: stretch; }
  .sqlwrap { position: relative; flex: 1; min-height: 60px; }
  .hl, .sql {
    margin: 0;
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1.5;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    white-space: pre-wrap;
    word-break: break-word;
    overflow-wrap: break-word;
    box-sizing: border-box;
  }
  .hl {
    position: absolute;
    inset: 0;
    overflow: auto;
    pointer-events: none;
    border-color: transparent;
    background: var(--bg);
    color: var(--text);
  }
  .sql {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 60px;
    resize: vertical;
    background: transparent;
    color: transparent;
    caret-color: var(--text);
  }
  .sql::placeholder { color: var(--text-dim); }
  .hl :global(.tok-keyword) { color: var(--brand); font-weight: 600; }
  .hl :global(.tok-string) { color: var(--green); }
  .hl :global(.tok-number) { color: var(--accent); }
  .hl :global(.tok-comment) { color: var(--text-dim); font-style: italic; }
  .hl :global(.tok-punct) { color: var(--text-dim); }
  .ebtns { display: flex; flex-direction: column; gap: 6px; }
  .btn { background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 8px 14px; font: inherit; font-size: 13px; cursor: pointer; white-space: nowrap; }
  .btn.primary { background: var(--brand); border-color: var(--brand); color: #1a1015; font-weight: 600; }
  .btn.on { border-color: var(--brand); }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .meta { font-size: 12px; color: var(--text-dim); }
  .edithint { color: var(--accent); }
  .err { background: color-mix(in srgb, var(--red) 12%, transparent); border: 1px solid var(--red); color: var(--text); border-radius: 8px; padding: 10px 12px; font-size: 13px; font-family: var(--font-mono); }
  .grid { flex: 1; min-height: 0; overflow: auto; border: 1px solid var(--border); border-radius: 10px; }
  .grid table { border-collapse: collapse; font-size: 12px; width: max-content; min-width: 100%; }
  .grid th { position: sticky; top: 0; background: var(--panel); text-align: left; padding: 7px 10px; border-bottom: 1px solid var(--border); color: var(--text-dim); font-weight: 500; white-space: nowrap; }
  .grid td { padding: 6px 10px; border-bottom: 1px solid var(--border); white-space: nowrap; max-width: 320px; overflow: hidden; text-overflow: ellipsis; }
  .grid td.null { color: var(--text-dim); font-style: italic; }
  .grid td.editable { cursor: text; }
  .grid td.edit { padding: 0; }
  .grid td.edit input { width: 100%; box-sizing: border-box; border: 2px solid var(--brand); background: var(--bg); color: var(--text); font: inherit; font-size: 12px; padding: 5px 8px; }
  .schema { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 18px; }
  .sblock h4 { margin: 0 0 8px; font-size: 13px; }
  .schema table { border-collapse: collapse; font-size: 12px; width: 100%; }
  .schema th { text-align: left; color: var(--text-dim); font-weight: 500; padding: 6px 10px; border-bottom: 1px solid var(--border); }
  .schema td { padding: 5px 10px; border-bottom: 1px solid var(--border); }
  .schema td.dim { color: var(--text-dim); }
  .key { font-family: var(--font-mono); font-size: 10px; background: var(--panel); border: 1px solid var(--border); border-radius: 4px; padding: 1px 5px; color: var(--accent); }
  .mono { font-family: var(--font-mono); }
</style>
