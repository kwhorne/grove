<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "../lib/api";
  import type { ResolvedSite, RequestEntry, RequestDetail } from "../lib/types";

  let { sites, notify }: { sites: ResolvedSite[]; notify: (m: string) => void } = $props();

  let hooks = $state<RequestEntry[]>([]);
  let live = $state(true);
  let timer: ReturnType<typeof setInterval> | undefined;
  let selected = $state<RequestDetail | null>(null);
  let detailFor = $state<number | null>(null);
  let target = $state("");
  let replaying = $state(false);
  let msg = $state("");

  let sampleHost = $derived(sites[0]?.hostname ?? "myapp.test");
  let captureUrl = $derived(`https://${sampleHost}/__grove/hooks/stripe`);

  async function refresh() {
    if (!live) return;
    try {
      hooks = await api.hookList(200);
    } catch {
      /* daemon down */
    }
  }

  onMount(() => {
    target = `https://${sampleHost}/`;
    refresh();
    timer = setInterval(refresh, 1000);
  });
  onDestroy(() => timer && clearInterval(timer));

  async function toggle(r: RequestEntry) {
    if (detailFor === r.id) {
      detailFor = null;
      selected = null;
      return;
    }
    detailFor = r.id;
    selected = null;
    try {
      selected = await api.hookDetail(r.id);
    } catch (e) {
      msg = String(e);
    }
  }

  function prettyBody(body: string): string {
    try {
      return JSON.stringify(JSON.parse(body), null, 2);
    } catch {
      return body;
    }
  }

  async function replay(id: number) {
    if (!target.trim()) {
      msg = "Enter a target URL first";
      return;
    }
    replaying = true;
    try {
      const [status, ms] = await api.hookReplayTo(id, target.trim());
      msg = `Delivered → ${status} in ${ms}ms`;
      notify(`Webhook re-delivered → ${status}`);
    } catch (e) {
      msg = String(e);
    }
    replaying = false;
  }

  async function copyAs(id: number, format: string) {
    try {
      const code = await api.hookToTest(id, format);
      await navigator.clipboard.writeText(code);
      msg = `Copied as ${format}`;
    } catch (e) {
      msg = String(e);
    }
  }

  async function clearAll() {
    try {
      await api.hookClear();
      hooks = [];
      detailFor = null;
      selected = null;
      msg = "Cleared";
    } catch (e) {
      msg = String(e);
    }
  }

  function localTime(ms: number): string {
    const d = new Date(ms);
    const p = (n: number, w = 2) => String(n).padStart(w, "0");
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }
</script>

<div class="wh">
  <div class="banner">
    <div>
      <div class="blabel">Point your provider at</div>
      <code class="url">{captureUrl}</code>
    </div>
    <div class="bhint">
      Any path under <span class="mono">/__grove/hooks/&lt;bucket&gt;</span> is captured and
      answered <span class="mono">200</span>. Run <span class="mono">grove share {sites[0]?.name ?? "&lt;site&gt;"}</span>
      for a public URL to give Stripe, GitHub, etc.
    </div>
  </div>

  <div class="toolbar">
    <div class="tgt">
      <label for="whtgt">Re-deliver to</label>
      <input id="whtgt" class="inp" bind:value={target} placeholder="https://myapp.test/stripe/webhook" spellcheck="false" />
    </div>
    {#if msg}<span class="msg">{msg}</span>{/if}
    <button class="btn" class:primary={live} onclick={() => (live = !live)}>{live ? "⏸ Pause" : "▶ Live"}</button>
    <button class="btn" onclick={clearAll} disabled={hooks.length === 0}>Clear</button>
  </div>

  {#if hooks.length === 0}
    <p class="empty">No webhooks captured yet. Send one to <span class="mono">{captureUrl}</span> and it shows up here, live.</p>
  {:else}
    <table class="reqs">
      <thead>
        <tr><th>#</th><th>Time</th><th>Method</th><th>Bucket</th><th>Path</th></tr>
      </thead>
      <tbody>
        {#each hooks as r (r.id)}
          <tr class="rrow" class:sel={detailFor === r.id} onclick={() => toggle(r)}>
            <td class="dim mono">#{r.id}</td>
            <td class="dim mono">{localTime(r.epoch_ms)}</td>
            <td><span class="method {r.method.toLowerCase()}">{r.method}</span></td>
            <td class="mono">{r.site}</td>
            <td class="path mono" title={r.path}>{r.path}</td>
          </tr>
          {#if detailFor === r.id && selected}
            <tr class="detail">
              <td colspan="5">
                <div class="dhead">
                  <span class="durl mono">{selected.method} {selected.path}</span>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); replay(r.id); }} disabled={replaying}>↻ Re-deliver</button>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); copyAs(r.id, "curl"); }}>curl</button>
                  <button class="btn sm" onclick={(e) => { e.stopPropagation(); copyAs(r.id, "pest"); }}>Pest</button>
                </div>
                {#if selected.headers.length}
                  <div class="dsec">Headers</div>
                  <table class="hdrs">
                    <tbody>
                      {#each selected.headers as [k, v]}
                        <tr><td class="hk mono">{k}</td><td class="hv mono">{v}</td></tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
                {#if selected.body}
                  <div class="dsec">Payload{selected.body_truncated ? " (truncated)" : ""}</div>
                  <pre class="body mono">{prettyBody(selected.body)}</pre>
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
  .wh { display: flex; flex-direction: column; gap: 14px; }
  .banner { display: flex; justify-content: space-between; gap: 20px; align-items: flex-start; background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 12px 14px; }
  .blabel { font-size: 11px; text-transform: uppercase; letter-spacing: 0.12em; color: var(--text-dim); margin-bottom: 4px; }
  .url { font-family: var(--font-mono); color: var(--brand); font-size: 13px; }
  .bhint { font-size: 12px; color: var(--text-dim); max-width: 48%; }
  .toolbar { display: flex; align-items: center; gap: 12px; }
  .tgt { display: flex; align-items: center; gap: 8px; flex: 1; }
  .tgt label { font-size: 12px; color: var(--text-dim); white-space: nowrap; }
  .inp { flex: 1; background: var(--bg); border: 1px solid var(--border); border-radius: 8px; color: var(--text); padding: 8px 10px; font: inherit; font-family: var(--font-mono); font-size: 12px; }
  .msg { color: var(--accent); font-size: 12px; }
  .btn { background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 8px 12px; font: inherit; font-size: 13px; cursor: pointer; white-space: nowrap; }
  .btn.primary { border-color: var(--brand); }
  .btn.sm { padding: 4px 10px; font-size: 12px; }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .empty { color: var(--text-dim); font-size: 13px; }
  .reqs { border-collapse: collapse; width: 100%; font-size: 13px; }
  .reqs th { text-align: left; color: var(--text-dim); font-weight: 500; padding: 6px 8px; border-bottom: 1px solid var(--border); }
  .reqs td { padding: 5px 8px; border-bottom: 1px solid var(--border); white-space: nowrap; }
  .reqs td.dim { color: var(--text-dim); }
  .reqs td.path { max-width: 420px; overflow: hidden; text-overflow: ellipsis; }
  .rrow { cursor: pointer; }
  .rrow:hover, .rrow.sel { background: var(--panel); }
  .method { font-weight: 600; }
  .method.get { color: var(--green); }
  .method.post { color: var(--accent); }
  .method.put, .method.patch { color: var(--amber); }
  .method.delete { color: var(--red); }
  .detail > td { background: var(--bg); padding: 12px 14px; white-space: normal; }
  .dhead { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; flex-wrap: wrap; }
  .durl { font-size: 12px; color: var(--text); flex: 1; word-break: break-all; }
  .dsec { font-size: 11px; text-transform: uppercase; letter-spacing: 0.12em; color: var(--text-dim); margin: 8px 0 4px; }
  .hdrs { border-collapse: collapse; font-size: 12px; }
  .hdrs td { padding: 2px 8px 2px 0; vertical-align: top; }
  .hk { color: var(--text-dim); white-space: nowrap; }
  .hv { color: var(--text); word-break: break-all; }
  .mono { font-family: var(--font-mono); }
  .body { margin: 0; padding: 8px 10px; background: var(--panel); border: 1px solid var(--border); border-radius: 6px; font-size: 12px; color: var(--text); max-height: 260px; overflow: auto; white-space: pre-wrap; word-break: break-word; }
</style>
