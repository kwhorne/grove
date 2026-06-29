<script lang="ts">
  import { api } from "../lib/api";
  import type { LogEntry, LogSource } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  let sources = $state<LogSource[]>([]);
  let selectedPath = $state<string | null>(null);
  let entries = $state<LogEntry[]>([]);
  let selectedEntry = $state<LogEntry | null>(null);
  let search = $state("");

  async function loadSources() {
    try {
      sources = await api.logSources();
      if (!selectedPath && sources.length) select(sources[0]);
    } catch (e) {
      notify(String(e));
    }
  }

  async function select(src: LogSource) {
    selectedPath = src.path;
    selectedEntry = null;
    await loadEntries();
  }

  async function loadEntries() {
    if (!selectedPath) return;
    try {
      entries = await api.logEntries(selectedPath, 200);
    } catch (e) {
      notify(String(e));
    }
  }

  const filtered = $derived(
    search.trim()
      ? entries.filter((e) =>
          (e.message + e.level + e.datetime).toLowerCase().includes(search.toLowerCase()),
        )
      : entries,
  );

  $effect(() => {
    loadSources();
    const t = setInterval(loadEntries, 4000);
    return () => clearInterval(t);
  });
</script>

<div class="logs">
  <aside class="sources">
    {#each sources as s (s.path)}
      <button class="source {selectedPath === s.path ? 'active' : ''}" onclick={() => select(s)}>
        <span class="kind {s.kind}">{s.kind === "laravel" ? "L" : "G"}</span>
        <span class="sname">{s.name}</span>
      </button>
    {/each}
    {#if sources.length === 0}
      <div class="empty-src">No log files found.</div>
    {/if}
  </aside>

  <div class="viewer">
    <div class="toolbar">
      <input class="search" placeholder="Search log…" bind:value={search} />
      <button class="btn" onclick={loadEntries} title="Refresh">⟳</button>
    </div>

    <div class="table">
      <div class="thead">
        <span class="c-level">Level</span><span class="c-date">Date</span><span class="c-msg">Message</span>
      </div>
      <div class="tbody">
        {#each filtered as e, i (i)}
          <button
            class="row {selectedEntry === e ? 'sel' : ''}"
            onclick={() => (selectedEntry = e)}
          >
            <span class="c-level"><span class="lvl {e.level.toLowerCase()}">{e.level}</span></span>
            <span class="c-date mono">{e.datetime || "—"}</span>
            <span class="c-msg">{e.message}</span>
          </button>
        {/each}
        {#if filtered.length === 0}
          <div class="empty-src">No entries.</div>
        {/if}
      </div>
    </div>

    {#if selectedEntry}
      <div class="detail">
        <div class="dmsg">{selectedEntry.message}</div>
        {#if selectedEntry.context}
          <pre class="mono ctx">{selectedEntry.context}</pre>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .logs {
    display: grid;
    grid-template-columns: 220px 1fr;
    gap: 12px;
    height: calc(100vh - 150px);
  }
  .sources {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    overflow-y: auto;
    padding: 6px;
  }
  .source {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-radius: 6px;
    padding: 7px 8px;
    color: var(--text);
    font-size: 12px;
  }
  .source:hover {
    background: var(--bg-3);
  }
  .source.active {
    background: var(--accent-2);
  }
  .kind {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    background: var(--bg-3);
    color: var(--text-dim);
  }
  .kind.laravel {
    color: var(--red);
  }
  .kind.service {
    color: var(--accent);
  }
  .sname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .viewer {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .toolbar {
    display: flex;
    gap: 8px;
    margin-bottom: 8px;
  }
  .search {
    flex: 1;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 5px 10px;
    color: var(--text);
    font-size: 12px;
  }
  .search:focus {
    border-color: var(--accent);
    outline: none;
  }
  .table {
    flex: 1;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .thead,
  .row {
    display: grid;
    grid-template-columns: 90px 160px 1fr;
    gap: 10px;
    align-items: center;
  }
  .thead {
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
  }
  .tbody {
    overflow-y: auto;
    flex: 1;
  }
  .row {
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-bottom: 1px solid var(--border);
    padding: 8px 12px;
    color: var(--text);
    font-size: 12px;
  }
  .row:hover {
    background: var(--bg-3);
  }
  .row.sel {
    background: var(--accent-2);
  }
  .c-msg {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .lvl {
    font-size: 10px;
    font-weight: 700;
    font-family: var(--font-mono);
  }
  .lvl.error {
    color: var(--red);
  }
  .lvl.warning {
    color: var(--amber);
  }
  .lvl.info,
  .lvl.notice {
    color: var(--accent);
  }
  .detail {
    margin-top: 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    padding: 12px 14px;
    max-height: 38%;
    overflow: auto;
  }
  .dmsg {
    font-size: 13px;
    margin-bottom: 8px;
  }
  .ctx {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: 11px;
    color: var(--text-dim);
    border-top: 1px solid var(--border);
    padding-top: 8px;
  }
  .empty-src {
    padding: 20px 12px;
    color: var(--text-dim);
    font-size: 12px;
    text-align: center;
  }
</style>
