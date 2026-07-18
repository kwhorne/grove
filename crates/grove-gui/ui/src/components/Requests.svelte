<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "../lib/api";
  import type {
    ResolvedSite,
    RequestEntry,
    RequestDetail,
    RequestChain,
    SqlCaptureState,
  } from "../lib/types";

  let { sites }: { sites: ResolvedSite[] } = $props();

  let requests = $state<RequestEntry[]>([]);
  let filter = $state("");
  let live = $state(true);
  let timer: ReturnType<typeof setInterval> | undefined;

  let selected = $state<RequestDetail | null>(null);
  let chain = $state<RequestChain | null>(null);
  let sqlCapture = $state<SqlCaptureState | null>(null);
  let detailFor = $state<number | null>(null);
  let replaying = $state(false);
  let msg = $state("");

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
    api
      .sqlCaptureStatus()
      .then((s) => (sqlCapture = s))
      .catch(() => {});
    timer = setInterval(refresh, 1000);
  });

  async function toggleSql() {
    try {
      sqlCapture = await api.sqlCaptureSet(!sqlCapture?.enabled);
      msg = sqlCapture.note;
    } catch (e) {
      msg = String(e);
    }
  }
  onDestroy(() => timer && clearInterval(timer));

  $effect(() => {
    filter;
    refresh();
  });

  async function toggle(r: RequestEntry) {
    if (detailFor === r.id) {
      detailFor = null;
      selected = null;
      chain = null;
      return;
    }
    detailFor = r.id;
    selected = null;
    chain = null;
    try {
      selected = await api.requestDetail(r.id);
      chain = await api.requestChain(r.id);
    } catch (e) {
      msg = String(e);
    }
  }

  async function replay(id: number) {
    replaying = true;
    try {
      const [status, ms] = await api.replayRequest(id);
      msg = `Replayed → ${status} in ${ms}ms`;
      await refresh();
    } catch (e) {
      msg = String(e);
    }
    replaying = false;
  }

  async function copyAs(id: number, format: string) {
    try {
      const code = await api.requestToTest(id, format);
      await navigator.clipboard.writeText(code);
      msg = `Copied as ${format}`;
    } catch (e) {
      msg = String(e);
    }
  }

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

    {#if msg}<span class="msg">{msg}</span>{/if}

    <button
      class="btn"
      class:primary={sqlCapture?.enabled}
      onclick={toggleSql}
      title={sqlCapture?.note ?? "Correlate SQL queries with requests (MySQL)"}
    >
      {sqlCapture?.enabled ? "◉ SQL" : "◎ SQL"}
    </button>

    <button class="btn" class:primary={live} onclick={() => (live = !live)}>
      {live ? "⏸ Pause" : "▶ Live"}
    </button>
  </div>

  {#if requests.length === 0}
    <p class="empty">No requests yet — open a <span class="mono">.test</span> site and reload. Every request Grove proxies shows up here, live.</p>
  {:else}
    <table class="reqs">
      <thead>
        <tr><th>#</th><th>Time</th><th>Method</th><th>Status</th><th>ms</th><th>Site</th><th>Path</th></tr>
      </thead>
      <tbody>
        {#each requests as r (r.id)}
          <tr class="rrow" class:sel={detailFor === r.id} onclick={() => toggle(r)}>
            <td class="dim mono">#{r.id}</td>
            <td class="dim mono">{localTime(r.epoch_ms)}</td>
            <td><span class="method {r.method.toLowerCase()}">{r.method}</span></td>
            <td><span class="code {statusClass(r.status)}">{r.status === 0 ? "ERR" : r.status}</span></td>
            <td class="dim" class:slow={r.duration_ms >= 500}>{r.duration_ms}</td>
            <td class="dim">{r.site}</td>
            <td class="path mono" title={r.path}>{r.path}</td>
          </tr>
          {#if detailFor === r.id && selected}
            <tr class="detail">
              <td colspan="7">
                <div class="dhead">
                  <span class="durl mono">{selected.method} {selected.https ? "https://" : "http://"}{selected.host}{selected.path}</span>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); replay(r.id); }} disabled={replaying}>↻ Replay</button>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); copyAs(r.id, "curl"); }} title="Copy as curl">curl</button>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); copyAs(r.id, "http"); }} title="Copy as .http">.http</button>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); copyAs(r.id, "pest"); }} title="Copy as Pest test">Pest</button>
                </div>
                {#if selected.headers.length}
                  <div class="dsec">Request headers</div>
                  <table class="hdrs">
                    <tbody>
                      {#each selected.headers as [k, v]}
                        <tr><td class="hk mono">{k}</td><td class="hv mono">{v}</td></tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
                {#if selected.body}
                  <div class="dsec">Body{selected.body_truncated ? " (truncated)" : ""}</div>
                  <pre class="body mono">{selected.body}</pre>
                {/if}
                {#if chain}
                  <div class="dsec">
                    Causal chain
                    <span class="chainmeta"
                      >{chain.metrics.duration_ms}ms · {chain.metrics.query_count}
                      queries · {chain.metrics.email_count} emails</span
                    >
                  </div>
                  {#if chain.queries.length}
                    <table class="hdrs">
                      <tbody>
                        {#each chain.queries as q}
                          <tr
                            ><td class="hk mono">{localTime(q.epoch_ms)}</td><td
                              class="hv mono">{q.sql}</td
                            ></tr
                          >
                        {/each}
                      </tbody>
                    </table>
                  {/if}
                  {#each chain.emails as m}
                    <div class="chainmail mono">✉ {m.subject} → {m.to.join(", ")}</div>
                  {/each}
                  {#if !chain.queries.length && !chain.emails.length}
                    <div class="chainempty">
                      No side effects captured in this request's window.{sqlCapture?.enabled
                        ? ""
                        : " Turn on SQL capture to include queries."}
                    </div>
                  {/if}
                {/if}
              </td>
            </tr>
          {/if}
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

  .rrow { cursor: pointer; }
  .rrow:hover { background: var(--panel); }
  .rrow.sel { background: var(--panel); }
  .msg { color: var(--accent); font-size: 12px; margin-left: auto; }
  .btn.sm { padding: 4px 10px; font-size: 12px; }
  .detail > td { background: var(--bg); padding: 12px 14px; white-space: normal; }
  .dhead { display: flex; align-items: center; gap: 12px; margin-bottom: 10px; }
  .durl { font-size: 12px; color: var(--text); word-break: break-all; flex: 1; }
  .dsec { font-size: 11px; text-transform: uppercase; letter-spacing: 0.12em; color: var(--text-dim); margin: 8px 0 4px; }
  .chainmeta { margin-left: 8px; text-transform: none; letter-spacing: 0; color: var(--text); font-size: 12px; }
  .chainmail { font-size: 12px; color: var(--text); padding: 3px 0; }
  .chainempty { font-size: 12px; color: var(--text-dim); padding: 2px 0; }
  .hdrs { border-collapse: collapse; font-size: 12px; }
  .hdrs td { padding: 2px 8px 2px 0; vertical-align: top; }
  .hk { color: var(--text-dim); white-space: nowrap; }
  .hv { color: var(--text); word-break: break-all; }
  .body { margin: 0; padding: 8px 10px; background: var(--panel); border: 1px solid var(--border); border-radius: 6px; font-size: 12px; color: var(--text); max-height: 220px; overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
