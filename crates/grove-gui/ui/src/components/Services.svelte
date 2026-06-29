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
      <div class="svc">
        <span class="dot {s.running ? 'on' : s.installed ? 'idle' : ''}"></span>
        <div class="info">
          <div class="name">{s.name}</div>
          <div class="meta mono">
            {s.version}{#if s.installed}&nbsp;·&nbsp;Port {s.port}{/if}
          </div>
        </div>
        <div class="actions">
          {#if !s.installed}
            <button class="btn primary" disabled={busy[s.key]} onclick={() => install(s.key)}>
              {busy[s.key] ? "Installing…" : "Install"}
            </button>
          {:else if s.running}
            <button class="btn" disabled={busy[s.key]} onclick={() => stop(s.key)}>Stop</button>
          {:else}
            <button class="btn" disabled={busy[s.key]} onclick={() => start(s.key)}>Start</button>
          {/if}
        </div>
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
  .svc {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .svc:last-child {
    border-bottom: 0;
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
