<script lang="ts">
  import type { DiagnosticEntry } from "../lib/types";

  let { diagnostics }: { diagnostics: DiagnosticEntry[] } = $props();

  const mark = (s: string) => (s === "pass" ? "✓" : s === "warn" ? "!" : "✗");
</script>

<div style="background: var(--bg-elev); border: 1px solid var(--border); border-radius: 10px; overflow: hidden;">
  {#each diagnostics as d (d.check)}
    <div class="diag {d.status}">
      <span class="mark">{mark(d.status)}</span>
      <div>
        <div>{d.check}</div>
        <div class="detail">{d.detail}</div>
      </div>
    </div>
  {/each}
  {#if diagnostics.length === 0}
    <div class="empty">No diagnostics yet.</div>
  {/if}
</div>
