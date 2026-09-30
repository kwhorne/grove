<script lang="ts">
  import { tick, untrack } from "svelte";
  import { api } from "../lib/api";
  import type { ResolvedSite } from "../lib/types";
  import {
    ago,
    loadPicked,
    recentSites,
    siteScore,
    type RecentReason,
  } from "../lib/sitesearch";

  let {
    open,
    sites,
    onclose,
    onfocus,
    onopen,
  }: {
    open: boolean;
    sites: ResolvedSite[];
    onclose: () => void;
    /** Enter: show just this site in Sites. */
    onfocus: (s: ResolvedSite) => void;
    /** ⌘Enter: open it in the browser. */
    onopen: (s: ResolvedSite) => void;
  } = $props();

  interface Row {
    site: ResolvedSite;
    note: string;
  }

  let query = $state("");
  let selected = $state(0);
  let input = $state<HTMLInputElement | null>(null);
  let list = $state<HTMLDivElement | null>(null);
  let git = $state<Record<string, number>>({});
  let requests = $state<Record<string, number>>({});

  const reasonLabel: Record<RecentReason, string> = {
    git: "git",
    request: "request",
    picked: "opened",
  };

  // Each time it opens: an empty query, the cursor in the box, and fresh
  // signals for what was worked on last. Only `open` is tracked — the site
  // list is refreshed every few seconds, and that must not wipe what is typed.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      query = "";
      selected = 0;
      tick().then(() => input?.focus());
      loadSignals();
    });
  });

  async function loadSignals() {
    try {
      git = await api.siteActivity(sites.filter((s) => !s.docker).map((s) => s.path));
    } catch {
      git = {};
    }
    try {
      const next: Record<string, number> = {};
      for (const r of await api.requestLog(null, 500)) {
        next[r.site] = Math.max(next[r.site] ?? 0, r.epoch_ms);
      }
      requests = next;
    } catch {
      requests = {};
    }
  }

  const rows = $derived.by((): Row[] => {
    const q = query.trim();
    if (!q) {
      return recentSites(sites, git, requests, loadPicked(), 10).map((r) => ({
        site: r.site,
        note: `${reasonLabel[r.reason]} ${ago(r.at)}`,
      }));
    }
    return sites
      .map((site) => ({ site, score: siteScore(site, q) }))
      .filter((r): r is { site: ResolvedSite; score: number } => r.score !== null)
      .sort((a, b) => b.score - a.score)
      .slice(0, 50)
      .map((r) => ({ site: r.site, note: r.site.driver }));
  });

  // A new query starts at the top.
  $effect(() => {
    void query;
    selected = 0;
  });

  function move(by: number) {
    if (rows.length === 0) return;
    selected = (selected + by + rows.length) % rows.length;
    tick().then(() =>
      list?.querySelector(`[data-i="${selected}"]`)?.scrollIntoView({ block: "nearest" }),
    );
  }

  function choose(i: number, inBrowser: boolean) {
    const row = rows[i];
    if (!row) return;
    if (inBrowser) onopen(row.site);
    else onfocus(row.site);
    onclose();
  }

  function onkey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(selected, e.metaKey || e.ctrlKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onclose();
    }
  }
</script>

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => e.target === e.currentTarget && onclose()}
  >
    <div class="palette" role="dialog" aria-modal="true" aria-label="Go to site">
      <input
        bind:this={input}
        bind:value={query}
        onkeydown={onkey}
        class="search"
        placeholder="Go to site…"
        spellcheck="false"
        autocomplete="off"
      />
      <div class="section">{query.trim() ? `${rows.length} match${rows.length === 1 ? "" : "es"}` : "Recently worked on"}</div>
      <div class="rows" bind:this={list}>
        {#each rows as row, i (row.site.name)}
          <button
            class="row {i === selected ? 'sel' : ''}"
            data-i={i}
            onmousemove={() => (selected = i)}
            onclick={(e) => choose(i, e.metaKey || e.ctrlKey)}
          >
            <span class="host">{row.site.hostname}</span>
            <span class="path mono">{row.site.docker ? "🐳 docker" : row.site.path}</span>
            <span class="note">{row.note}</span>
          </button>
        {:else}
          <div class="none">
            {query.trim() ? "No site matches." : "Nothing worked on yet — type to search every site."}
          </div>
        {/each}
      </div>
      <div class="hints">
        <span><kbd>↑</kbd><kbd>↓</kbd> choose</span>
        <span><kbd>↵</kbd> focus in Sites</span>
        <span><kbd>⌘</kbd><kbd>↵</kbd> open in browser</span>
        <span><kbd>esc</kbd> close</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 1000;
  }
  .palette {
    width: 620px;
    max-width: 92vw;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 20px 56px rgba(0, 0, 0, 0.55);
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .search {
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    font-size: 16px;
    padding: 16px 18px;
    outline: none;
  }
  .section {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    padding: 10px 18px 4px;
  }
  .rows {
    max-height: 50vh;
    overflow-y: auto;
    padding: 4px 6px 6px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-areas: "host note" "path note";
    column-gap: 12px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-radius: 8px;
    padding: 8px 12px;
    color: var(--text);
    cursor: pointer;
  }
  .row.sel {
    background: var(--accent-2);
  }
  .host {
    grid-area: host;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .path {
    grid-area: path;
    font-size: 11px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .note {
    grid-area: note;
    align-self: center;
    font-size: 11px;
    color: var(--text-dim);
  }
  .none {
    color: var(--text-dim);
    padding: 14px 12px;
    font-size: 13px;
  }
  .hints {
    display: flex;
    gap: 16px;
    border-top: 1px solid var(--border);
    padding: 8px 18px;
    font-size: 11px;
    color: var(--text-dim);
  }
  kbd {
    font-family: inherit;
    font-size: 10px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 4px;
    padding: 0 4px;
    margin-right: 2px;
    background: var(--bg-3);
  }
</style>
