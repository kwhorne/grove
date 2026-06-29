<script lang="ts">
  import { api } from "../lib/api";
  import type { ServiceState, ServiceStatus } from "../lib/types";

  let { services, notify }: { services: ServiceState[]; notify: (m: string) => void } = $props();

  let bundled = $state<ServiceStatus[]>([]);
  let busy = $state<Record<string, boolean>>({});

  async function load() {
    try {
      bundled = await api.serviceList();
    } catch (e) {
      notify(String(e));
    }
  }

  async function act(key: string, fn: () => Promise<string>) {
    busy = { ...busy, [key]: true };
    try {
      notify(await fn());
    } catch (e) {
      notify(String(e));
    }
    busy = { ...busy, [key]: false };
    await load();
  }

  const install = (k: string) => act(k, () => api.serviceInstall(k));
  const start = (k: string) => act(k, () => api.serviceStart(k));
  const stop = (k: string) => act(k, () => api.serviceStop(k));
  const restart = (k: string) => act(k, () => api.serviceRestart(k));

  let expanded = $state<string | null>(null);
  let portEdit = $state<Record<string, number>>({});

  function toggle(s: ServiceStatus) {
    if (!s.installed) return;
    expanded = expanded === s.key ? null : s.key;
    if (expanded === s.key) portEdit = { ...portEdit, [s.key]: s.port };
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      notify("copied to clipboard");
    } catch {
      notify("copy failed");
    }
  }

  async function savePort(key: string) {
    const port = portEdit[key];
    if (!port || port < 1 || port > 65535) {
      notify("invalid port");
      return;
    }
    await act(key, () => api.serviceSetPort(key, port));
  }

  // Group bundled services by category for a Herd-style layout.
  const groups = $derived(
    bundled.reduce<Record<string, ServiceStatus[]>>((acc, s) => {
      (acc[s.category] ??= []).push(s);
      return acc;
    }, {}),
  );

  $effect(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  });
</script>

<!-- Built-in daemon services (DNS, mail) -->
<div class="builtins">
  {#each services as svc (svc.name)}
    <span class="chip">
      <span class="dot {svc.running ? 'on' : ''}"></span>
      {svc.name}{#if svc.port}<span class="mono">:{svc.port}</span>{/if}
    </span>
  {/each}
</div>

{#each Object.entries(groups) as [category, items] (category)}
  <div class="cat-label">{category}</div>
  <div class="svc-list">
    {#each items as s (s.key)}
      <div class="svc-wrap">
        <div class="svc">
          <button
            class="svc-head"
            class:clickable={s.installed}
            disabled={!s.installed}
            onclick={() => toggle(s)}
          >
            <span class="dot {s.running ? 'on' : s.installed ? 'idle' : ''}"></span>
            <div class="info">
              <div class="name">
                {s.name}
                {#if s.installed}<span class="chev">{expanded === s.key ? "▾" : "▸"}</span>{/if}
              </div>
              <div class="meta mono">
                {s.version}{#if s.installed}&nbsp;·&nbsp;Port {s.port}{/if}
              </div>
            </div>
          </button>
          <div class="actions">
            {#if !s.installed}
              <button class="btn primary" disabled={busy[s.key]} onclick={() => install(s.key)}>
                {busy[s.key] ? "Installing…" : "Install"}
              </button>
            {:else if s.running}
              <button class="btn" disabled={busy[s.key]} onclick={() => restart(s.key)}>Restart</button>
              <button class="btn" disabled={busy[s.key]} onclick={() => stop(s.key)}>Stop</button>
            {:else}
              <button class="btn primary" disabled={busy[s.key]} onclick={() => start(s.key)}>Start</button>
            {/if}
          </div>
        </div>

        {#if expanded === s.key && s.installed}
          <div class="details">
            <div class="drow">
              <span class="dk">Connection URI</span>
              <code class="dv">{s.uri}</code>
              <button class="btn icon" title="Copy" onclick={() => copy(s.uri)}>⧉</button>
            </div>
            <div class="drow">
              <span class="dk">Host</span>
              <code class="dv">{s.host}</code>
              <button class="btn icon" title="Copy" onclick={() => copy(s.host)}>⧉</button>
            </div>
            {#if s.username}
              <div class="drow">
                <span class="dk">Username</span>
                <code class="dv">{s.username}</code>
                <button class="btn icon" title="Copy" onclick={() => copy(s.username!)}>⧉</button>
              </div>
            {/if}
            {#if s.socket}
              <div class="drow">
                <span class="dk">Socket</span>
                <code class="dv">{s.socket}</code>
                <button class="btn icon" title="Copy" onclick={() => copy(s.socket!)}>⧉</button>
              </div>
            {/if}
            <div class="drow">
              <span class="dk">Port</span>
              <input class="port-inp" type="number" bind:value={portEdit[s.key]} />
              <button class="btn" disabled={busy[s.key]} onclick={() => savePort(s.key)}>Save</button>
            </div>
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/each}

<p class="note">
  Grove downloads and supervises these itself — no Homebrew, MySQL or Redis to install separately.
</p>

<style>
  .builtins {
    display: flex;
    gap: 8px;
    margin-bottom: 18px;
    flex-wrap: wrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 4px 10px;
    border-radius: 6px;
    font-size: 12px;
    background: var(--bg-3);
    border: 1px solid var(--border);
  }
  .chip .mono {
    color: var(--text-dim);
    margin-left: 2px;
  }
  .cat-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    margin: 16px 0 8px;
  }
  .svc-list {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    overflow: hidden;
  }
  .svc-wrap {
    border-bottom: 1px solid var(--border);
  }
  .svc-wrap:last-child {
    border-bottom: 0;
  }
  .svc {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 14px 0 0;
  }
  .svc-head {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    background: transparent;
    border: 0;
    color: var(--text);
    text-align: left;
  }
  .svc-head.clickable {
    cursor: pointer;
  }
  .svc-head.clickable:hover {
    background: var(--bg-3);
  }
  .svc-head:disabled {
    cursor: default;
  }
  .chev {
    color: var(--text-dim);
    font-size: 10px;
    margin-left: 4px;
  }
  .details {
    padding: 4px 14px 12px 35px;
    background: var(--bg-2);
  }
  .drow {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 0;
  }
  .dk {
    width: 110px;
    font-size: 11px;
    color: var(--text-dim);
    flex: none;
  }
  .dv {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--accent);
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 3px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .port-inp {
    width: 90px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text);
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 3px 8px;
  }
  .port-inp:focus {
    border-color: var(--accent);
    outline: none;
  }
  .info {
    flex: 1;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
  }
  .meta {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 1px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--text-dim);
    flex: none;
  }
  .dot.on {
    background: var(--green);
    box-shadow: 0 0 8px var(--green);
  }
  .dot.idle {
    background: var(--amber);
  }
  .note {
    margin-top: 18px;
    font-size: 12px;
    color: var(--text-dim);
  }
</style>
