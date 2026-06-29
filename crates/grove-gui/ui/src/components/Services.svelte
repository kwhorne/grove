<script lang="ts">
  import type { ServiceState } from "../lib/types";

  let { services }: { services: ServiceState[] } = $props();
</script>

<div class="cards">
  {#each services as svc (svc.name)}
    <div class="card">
      <h3>{svc.name}</h3>
      <div class="big" style="color: {svc.running ? 'var(--accent)' : 'var(--text-dim)'}">
        {svc.running ? "running" : "stopped"}
      </div>
      {#if svc.port}
        <div class="mono">port {svc.port}</div>
      {/if}
    </div>
  {/each}
</div>

{#if services.length === 0}
  <div class="empty">No managed services. A mail-catcher and DB supervisor land in a later phase.</div>
{/if}
