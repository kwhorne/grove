<script lang="ts">
  import type { PhpBuild } from "../lib/types";

  let { php }: { php: PhpBuild[] } = $props();
</script>

{#if php.length === 0}
  <div class="empty">
    No PHP builds. Install one with <span class="mono">grove php install 8.4</span>
  </div>
{:else}
  <table>
    <thead>
      <tr>
        <th>Version</th>
        <th>Source</th>
        <th>Binary</th>
      </tr>
    </thead>
    <tbody>
      {#each php as b (b.version)}
        <tr>
          <td class="host">php@{b.version}</td>
          <td>
            <span class="badge">{b.user_registered ? "custom" : "managed"}</span>
          </td>
          <td class="mono">{b.fpm_binary}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}
