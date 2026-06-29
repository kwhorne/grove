<script lang="ts">
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

  const toggleSecure = (s: ResolvedSite) => run(api.secure(s.name, !s.secure));
  const setPhp = (s: ResolvedSite, v: string) => run(api.isolate(s.name, v));
  const setNode = (s: ResolvedSite, v: string) => run(api.setNode(s.name, v === "" ? null : v));
  const open = (s: ResolvedSite) => api.openUrl(url(s));
  const reveal = (s: ResolvedSite) => api.openPath(s.path);
  const unlink = (s: ResolvedSite) => run(api.unlink(s.name));

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
            <div class="mono">{s.path}</div>
          </td>
          <td><span class="badge {s.driver}">{s.driver}</span></td>
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
          </td>
          <td>
            <button
              class="toggle {s.secure ? 'on' : ''}"
              aria-label="toggle https"
              onclick={() => toggleSecure(s)}
            >
              <span class="knob"></span>
            </button>
          </td>
          <td>
            <div class="btn-row">
              <button class="btn icon" title="Open in browser" onclick={() => open(s)}>↗</button>
              <button class="btn icon" title="Reveal folder" onclick={() => reveal(s)}>📁</button>
              {#if s.kind === "linked"}
                <button class="btn icon" title="Unlink" onclick={() => unlink(s)}>✕</button>
              {/if}
            </div>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}
