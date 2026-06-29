<script lang="ts">
  import { api } from "../lib/api";
  import type { NodeVersion } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  let nodes = $state<NodeVersion[]>([]);
  let busy = $state<Record<string, boolean>>({});

  async function load() {
    try {
      nodes = await api.nodeList();
    } catch (e) {
      notify(String(e));
    }
  }

  async function install(major: string) {
    busy = { ...busy, [major]: true };
    try {
      notify(await api.nodeInstall(major));
    } catch (e) {
      notify(String(e));
    }
    busy = { ...busy, [major]: false };
    await load();
  }

  $effect(() => {
    load();
  });
</script>

<table>
  <thead>
    <tr>
      <th>Major version</th>
      <th>Installed version</th>
      <th style="width:1%"></th>
    </tr>
  </thead>
  <tbody>
    {#each nodes as n (n.major)}
      <tr>
        <td class="host">Node {n.major}</td>
        <td class="mono">{n.installed ? `v${n.version}` : "—"}</td>
        <td>
          <button
            class="btn {n.installed ? '' : 'primary'}"
            disabled={busy[n.major]}
            onclick={() => install(n.major)}
          >
            {busy[n.major] ? "Working…" : n.installed ? "Update" : "Install"}
          </button>
        </td>
      </tr>
    {/each}
  </tbody>
</table>

<p class="mono" style="margin-top:14px; color: var(--text-dim);">
  Node binaries (node · npm · npx) are downloaded into Grove — no nvm or Homebrew needed.
</p>
