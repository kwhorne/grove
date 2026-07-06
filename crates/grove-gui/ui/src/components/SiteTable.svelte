<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "../lib/api";
  import type { ResolvedSite } from "../lib/types";

  let {
    sites,
    phpVersions,
    nodeVersions,
    notify,
    onchange,
  }: {
    sites: ResolvedSite[];
    phpVersions: string[];
    nodeVersions: string[];
    notify: (m: string) => void;
    onchange: () => void;
  } = $props();

  async function run(p: Promise<string>) {
    try {
      notify(await p);
      onchange();
    } catch (e) {
      notify(String(e));
    }
  }

  // Map of host → public URL for sites currently being shared.
  let shared = $state<Record<string, string>>({});
  let shareBusy = $state<Record<string, boolean>>({});
  let timer: ReturnType<typeof setInterval> | undefined;

  async function refreshTunnels() {
    try {
      const list = await api.tunnelList();
      const next: Record<string, string> = {};
      for (const t of list) next[t.site] = t.public_url;
      shared = next;
    } catch {
      /* daemon down */
    }
  }
  onMount(() => {
    refreshTunnels();
    timer = setInterval(refreshTunnels, 2500);
  });
  onDestroy(() => timer && clearInterval(timer));

  async function toggleShare(s: ResolvedSite) {
    shareBusy = { ...shareBusy, [s.hostname]: true };
    try {
      if (shared[s.hostname]) {
        notify(await api.tunnelStop(s.hostname));
      } else {
        const res = await api.tunnelStart(s.name, null, null);
        const u = res[0]?.public_url ?? "";
        try {
          await navigator.clipboard.writeText(u);
        } catch {
          /* ignore */
        }
        notify(`Sharing ${s.hostname} → ${u} (copied)`);
      }
      await refreshTunnels();
    } catch (e) {
      notify(String(e));
    }
    shareBusy = { ...shareBusy, [s.hostname]: false };
  }

  const toggleSecure = (s: ResolvedSite) => run(api.secure(s.name, !s.secure));
  const setPhp = (s: ResolvedSite, v: string) => run(api.isolate(s.name, v));
  const setNode = (s: ResolvedSite, v: string) => run(api.setNode(s.name, v === "" ? null : v));
  const open = (s: ResolvedSite) => api.openUrl(url(s));
  const reveal = (s: ResolvedSite) => api.openPath(s.path);
  async function copyShareUrl(host: string) {
    const u = shared[host];
    if (!u) return;
    try {
      await navigator.clipboard.writeText(u);
      notify("Public URL copied");
    } catch {
      notify(u);
    }
  }

  let dockerBusy = $state<Record<string, boolean>>({});
  async function dockerAction(s: ResolvedSite, action: string) {
    if (!s.docker_id) return;
    dockerBusy = { ...dockerBusy, [s.name]: true };
    try {
      notify(await api.dockerControl(s.docker_id, action));
      onchange();
    } catch (e) {
      notify(String(e));
    }
    dockerBusy = { ...dockerBusy, [s.name]: false };
  }

  async function forget(s: ResolvedSite) {
    const ok = confirm(
      `Remove ${s.hostname} from the list?\n\nThe project files in ${s.path} are kept — this only hides it from Grove.`,
    );
    if (!ok) return;
    await run(api.forgetSite(s.name));
  }

  function url(s: ResolvedSite): string {
    return `${s.secure ? "https" : "http"}://${s.hostname}`;
  }
</script>

{#if sites.length === 0}
  <div class="empty">
    No sites yet. Park a directory from the CLI: <span class="mono">grove park ~/Code</span>
  </div>
{:else}
  <table>
    <thead>
      <tr>
        <th>Host</th>
        <th>Driver</th>
        <th>PHP</th>
        <th>Node</th>
        <th>HTTPS</th>
        <th style="width:1%">Actions</th>
      </tr>
    </thead>
    <tbody>
      {#each sites as s (s.name)}
        <tr>
          <td class="host">
            <a href={url(s)} onclick={(e) => { e.preventDefault(); open(s); }}>
              {s.hostname}
            </a>
            {#if s.docker}
              <div class="mono dim">
                🐳 {s.docker_running ? (s.proxy_to ?? "running") : "stopped"}
              </div>
            {:else}
              <div class="mono">{s.path}</div>
            {/if}
            {#if shared[s.hostname]}
              <button
                class="share-url mono"
                title="Public tunnel — click to copy"
                onclick={() => copyShareUrl(s.hostname)}>
                🌍 {shared[s.hostname]}
              </button>
            {/if}
          </td>
          <td><span class="badge {s.driver}">{s.docker ? "🐳 docker" : s.driver}</span></td>
          <td>
            {#if s.driver === "proxy"}
              <span class="mono">—</span>
            {:else}
              <select class="php" value={s.php} onchange={(e) => setPhp(s, (e.currentTarget as HTMLSelectElement).value)}>
                {#if !phpVersions.includes(s.php)}
                  <option value={s.php}>{s.php}</option>
                {/if}
                {#each phpVersions as v}
                  <option value={v}>{v}</option>
                {/each}
              </select>
            {/if}
          </td>
          <td>
            {#if s.driver === "proxy"}
              <span class="mono">—</span>
            {:else}
              <select
                class="php"
                value={s.node ?? ""}
                onchange={(e) => setNode(s, (e.currentTarget as HTMLSelectElement).value)}
              >
                <option value="">—</option>
                {#if s.node && !nodeVersions.includes(s.node)}
                  <option value={s.node}>{s.node}</option>
                {/if}
                {#each nodeVersions as v}
                  <option value={v}>{v}</option>
                {/each}
              </select>
            {/if}
          </td>
          <td>
            {#if s.docker}
              <span class="mono" title="Served over HTTPS by Grove">🔒</span>
            {:else}
              <button
                class="toggle {s.secure ? 'on' : ''}"
                aria-label="toggle https"
                onclick={() => toggleSecure(s)}
              >
                <span class="knob"></span>
              </button>
            {/if}
          </td>
          <td>
            <div class="btn-row">
              <button class="btn icon" title="Open in browser" onclick={() => open(s)}>↗</button>
              <button
                class="btn icon {shared[s.hostname] ? 'sharing' : ''}"
                title={shared[s.hostname] ? `Public: ${shared[s.hostname]} — click to stop` : "Share publicly"}
                disabled={shareBusy[s.hostname]}
                onclick={() => toggleShare(s)}>🌍</button>
              {#if s.docker}
                {#if s.docker_running}
                  <button class="btn icon" title="Restart container" disabled={dockerBusy[s.name]} onclick={() => dockerAction(s, "restart")}>↻</button>
                  <button class="btn icon danger" title="Stop container" disabled={dockerBusy[s.name]} onclick={() => dockerAction(s, "stop")}>⏹</button>
                {:else}
                  <button class="btn icon" title="Start container" disabled={dockerBusy[s.name]} onclick={() => dockerAction(s, "start")}>▶</button>
                {/if}
              {:else}
                <button class="btn icon" title="Reveal folder" onclick={() => reveal(s)}>📁</button>
                <button class="btn icon danger" title="Remove from list (keeps files)" onclick={() => forget(s)}>🗑</button>
              {/if}
            </div>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}
