<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "../lib/api";
  import type { ResolvedSite, TunnelStatus, TunnelRequestEntry } from "../lib/types";

  let { sites, notify }: { sites: ResolvedSite[]; notify: (m: string) => void } =
    $props();

  let tunnels = $state<TunnelStatus[]>([]);
  let requests = $state<TunnelRequestEntry[]>([]);
  let busy = $state(false);

  // Start form.
  let site = $state("");
  let subdomain = $state("");
  let basicAuth = $state("");

  let timer: ReturnType<typeof setInterval> | undefined;

  async function refresh() {
    try {
      tunnels = await api.tunnelList();
      requests = await api.tunnelRequests(null);
    } catch (e) {
      // daemon down — leave as-is
    }
  }

  onMount(() => {
    if (sites.length && !site) site = sites[0].name;
    refresh();
    timer = setInterval(refresh, 1500);
  });
  onDestroy(() => timer && clearInterval(timer));

  const sharing = (name: string) => tunnels.some((t) => t.site === `${name}.test` || t.site === name);

  async function start() {
    if (!site) return;
    busy = true;
    try {
      const res = await api.tunnelStart(
        site,
        subdomain.trim() || null,
        basicAuth.trim() || null,
      );
      const url = res[0]?.public_url ?? "";
      notify(`Sharing ${site} → ${url}`);
      subdomain = "";
      basicAuth = "";
      await refresh();
    } catch (e) {
      notify(String(e));
    }
    busy = false;
  }

  async function stop(s: string) {
    try {
      notify(await api.tunnelStop(s));
      await refresh();
    } catch (e) {
      notify(String(e));
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      notify("Public URL copied");
    } catch {
      notify("copy failed");
    }
  }

  function uptime(ms: number): string {
    const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
    if (s < 60) return `${s}s`;
    if (s < 3600) return `${Math.floor(s / 60)}m ${s % 60}s`;
    return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  }

  function clock(ms: number): string {
    const d = new Date(ms);
    return d.toLocaleTimeString([], { hour12: false });
  }

  function statusClass(code: number): string {
    if (code === 0) return "err";
    if (code >= 500) return "err";
    if (code >= 400) return "warn";
    if (code >= 300) return "redir";
    return "ok";
  }
</script>

<div class="tunnels">
  <!-- Start form -->
  <div class="starter">
    <select bind:value={site} class="inp">
      {#each sites as s (s.name)}
        <option value={s.name}>{s.hostname}</option>
      {/each}
    </select>
    <input class="inp" placeholder="subdomain (optional)" bind:value={subdomain} />
    <input class="inp" placeholder="basic auth user:pass (optional)" bind:value={basicAuth} />
    <button class="btn primary" disabled={busy || !site} onclick={start}>
      {busy ? "Connecting…" : "Share"}
    </button>
  </div>

  <!-- Active tunnels -->
  {#if tunnels.length === 0}
    <p class="empty">No active tunnels. Share a site to get a public URL for demos or webhooks.</p>
  {:else}
    <div class="active">
      {#each tunnels as t (t.site)}
        <div class="tcard">
          <div class="tinfo">
            <div class="thost">{t.site}</div>
            <button class="turl" title="Copy" onclick={() => copy(t.public_url)}>
              {t.public_url}
            </button>
          </div>
          <div class="tmeta mono">
            <span>{t.request_count} req</span>
            <span>·</span>
            <span>up {uptime(t.started_at_ms)}</span>
          </div>
          <div class="tactions">
            <button class="btn icon" title="Open" onclick={() => api.openUrl(t.public_url)}>↗</button>
            <button class="btn" onclick={() => stop(t.site)}>Stop</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Request inspector -->
  <div class="inspector">
    <div class="ihead">
      <h3>Request inspector</h3>
      <span class="muted">live · last {requests.length}</span>
    </div>
    {#if requests.length === 0}
      <p class="empty small">Incoming requests appear here in real time — great for debugging webhooks.</p>
    {:else}
      <table class="reqs mono">
        <thead>
          <tr><th>Time</th><th>Site</th><th>Method</th><th>Path</th><th>Status</th><th>ms</th></tr>
        </thead>
        <tbody>
          {#each requests as r (r.at_unix_ms + r.path + r.method + r.status)}
            <tr>
              <td class="dim">{clock(r.at_unix_ms)}</td>
              <td class="dim">{r.site}</td>
              <td><span class="method {r.method.toLowerCase()}">{r.method}</span></td>
              <td class="path" title={r.path}>{r.path}</td>
              <td><span class="code {statusClass(r.status)}">{r.status === 0 ? "ERR" : r.status}</span></td>
              <td class="dim">{r.duration_ms}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
</div>

<style>
  .tunnels {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .starter {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
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
  .starter .inp:nth-child(2),
  .starter .inp:nth-child(3) {
    flex: 1;
    min-width: 140px;
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
  .btn.icon {
    padding: 8px 10px;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .empty {
    color: var(--text-dim);
    font-size: 13px;
    padding: 8px 0;
  }
  .empty.small {
    padding: 4px 0;
  }
  .active {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .tcard {
    display: flex;
    align-items: center;
    gap: 16px;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 14px;
  }
  .tinfo {
    flex: 1;
    min-width: 0;
  }
  .thost {
    font-weight: 600;
    font-size: 14px;
  }
  .turl {
    background: none;
    border: none;
    color: var(--accent);
    font-family: var(--font-mono);
    font-size: 12px;
    cursor: pointer;
    padding: 2px 0;
  }
  .turl:hover {
    text-decoration: underline;
  }
  .tmeta {
    display: flex;
    gap: 6px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .tactions {
    display: flex;
    gap: 6px;
  }
  .inspector {
    border-top: 1px solid var(--border);
    padding-top: 14px;
  }
  .ihead {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .ihead h3 {
    margin: 0;
    font-size: 14px;
  }
  .muted {
    color: var(--text-dim);
    font-size: 12px;
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
  .reqs td.path {
    max-width: 360px;
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
