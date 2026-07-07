<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "../lib/api";
  import type { ResolvedSite, RequestEntry } from "../lib/types";

  let { sites }: { sites: ResolvedSite[] } = $props();

  let requests = $state<RequestEntry[]>([]);
  let filter = $state("");
  let live = $state(true);
  let timer: ReturnType<typeof setInterval> | undefined;

  async function refresh() {
    if (!live) return;
    try {
      requests = await api.requestLog(filter || null, 200);
    } catch (e) {
      // daemon down — leave as-is
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 1000);
  });
  onDestroy(() => timer && clearInterval(timer));

  $effect(() => {
    filter;
    refresh();
  });

  function statusClass(s: number): string {
    if (s === 0 || s >= 500) return "err";
    if (s >= 400) return "warn";
    if (s >= 300) return "redir";
    return "ok";
  }

  function localTime(ms: number): string {
    const d = new Date(ms);
    const p = (n: number, w = 2) => String(n).padStart(w, "0");
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
  }

  // Little summary of what's on screen.
  let count = $derived(requests.length);
  let avgMs = $derived(
    requests.length
      ? Math.round(requests.reduce((a, r) => a + r.duration_ms, 0) / requests.length)
      : 0,
  );
  let errRate = $derived(
    requests.length
      ? Math.round((requests.filter((r) => r.status === 0 || r.status >= 400).length / requests.length) * 100)
      : 0,
  );
</script>

<div class="reqpanel">
  <div class="toolbar">
    <select class="inp" bind:value={filter}>
      <option value="">All sites</option>
      {#each sites as s (s.name)}
        <option value={s.name}>{s.name}</option>
      {/each}
    </select>

    <div class="stats">
      <span><b>{count}</b> shown</span>
      <span><b>{avgMs}</b>ms avg</span>
      <span class:bad={errRate > 0}><b>{errRate}%</b> errors</span>
    </div>

    <button class="btn" class:primary={live} onclick={() => (live = !live)}>
      {live ? "⏸ Pause" : "▶ Live"}
    </button>
  </div>

  {#if requests.length === 0}
    <p class="empty">No requests yet — open a <span class="mono">.test</span> site and reload. Every request Grove proxies shows up here, live.</p>
  {:else}
    <table class="reqs">
      <thead>
        <tr><th>Time</th><th>Method</th><th>Status</th><th>ms</th><th>Site</th><th>Path</th></tr>
      </thead>
      <tbody>
        {#each requests as r (r.epoch_ms + r.path + r.method + r.status)}
          <tr>
            <td class="dim mono">{localTime(r.epoch_ms)}</td>
            <td><span class="method {r.method.toLowerCase()}">{r.method}</span></td>
            <td><span class="code {statusClass(r.status)}">{r.status === 0 ? "ERR" : r.status}</span></td>
            <td class="dim" class:slow={r.duration_ms >= 500}>{r.duration_ms}</td>
            <td class="dim">{r.site}</td>
            <td class="path mono" title={r.path}>{r.path}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .reqpanel {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .inp {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    padding: 8px 10px;
    font: inherit;
    font-size: 13px;
  }
  .stats {
    display: flex;
    gap: 16px;
    color: var(--text-dim);
    font-size: 12px;
    margin-left: auto;
  }
  .stats b {
    color: var(--text);
  }
  .stats .bad b {
    color: var(--red);
  }
  .btn {
    background: var(--panel);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 8px 14px;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .btn:hover {
    border-color: var(--accent);
  }
  .btn.primary {
    background: var(--brand);
    border-color: var(--brand);
    color: #1a1015;
    font-weight: 600;
  }
  .empty {
    color: var(--text-dim);
    font-size: 13px;
  }
  .mono {
    font-family: var(--font-mono);
  }
  table.reqs {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
  }
  .reqs th {
    text-align: left;
    color: var(--text-dim);
    font-weight: 500;
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .reqs td {
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  .reqs td.dim {
    color: var(--text-dim);
  }
  .reqs td.slow {
    color: var(--amber);
    font-weight: 600;
  }
  .reqs td.path {
    max-width: 460px;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text);
  }
  .method {
    font-weight: 600;
  }
  .method.get { color: var(--green); }
  .method.post { color: var(--accent); }
  .method.put, .method.patch { color: var(--amber); }
  .method.delete { color: var(--red); }
  .code {
    font-weight: 600;
  }
  .code.ok { color: var(--green); }
  .code.redir { color: var(--accent); }
  .code.warn { color: var(--amber); }
  .code.err { color: var(--red); }
</style>
