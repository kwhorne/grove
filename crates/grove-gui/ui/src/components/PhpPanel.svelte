<script lang="ts">
  import { api } from "../lib/api";
  import type { NodeVersion, PhpBuild } from "../lib/types";

  let { php, notify }: { php: PhpBuild[]; notify: (m: string) => void } = $props();

  let versions = $state<NodeVersion[]>([]);
  let busy = $state<Record<string, boolean>>({});

  async function load() {
    try {
      versions = await api.phpVersions();
    } catch (e) {
      notify(String(e));
    }
  }

  async function install(major: string) {
    busy = { ...busy, [major]: true };
    try {
      notify(await api.phpInstall(major));
    } catch (e) {
      notify(String(e));
    }
    busy = { ...busy, [major]: false };
    await load();
  }

  $effect(() => {
    load();
  });

  // Custom (bring-your-own) builds that aren't part of the offered majors.
  const custom = $derived(php.filter((b) => b.user_registered));
</script>

<table>
  <thead>
    <tr>
      <th>Version</th>
      <th>Installed</th>
      <th style="width:1%"></th>
    </tr>
  </thead>
  <tbody>
    {#each versions as v (v.major)}
      <tr>
        <td class="host">PHP {v.major}</td>
        <td class="mono">{v.installed ? "yes" : "—"}</td>
        <td>
          <button
            class="btn {v.installed ? '' : 'primary'}"
            disabled={busy[v.major]}
            onclick={() => install(v.major)}
          >
            {busy[v.major] ? "Working…" : v.installed ? "Update" : "Install"}
          </button>
        </td>
      </tr>
    {/each}
  </tbody>
</table>

{#if custom.length}
  <h3 style="font-size:13px;color:var(--text-dim);margin:20px 0 8px;text-transform:uppercase;letter-spacing:0.5px">
    Custom builds
  </h3>
  <table>
    <tbody>
      {#each custom as b (b.version)}
        <tr>
          <td class="host">php@{b.version}</td>
          <td class="mono">{b.fpm_binary}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<p class="mono" style="margin-top:14px; color: var(--text-dim);">
  Self-contained static PHP-FPM builds are downloaded into Grove — no Homebrew or Herd needed.
</p>
