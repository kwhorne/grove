<script lang="ts">
  import { api } from "../lib/api";
  import type { CapturedEmail, EmailSummary } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  let mails = $state<EmailSummary[]>([]);
  let selected = $state<CapturedEmail | null>(null);
  let view = $state<"text" | "html" | "raw">("text");

  async function load() {
    try {
      mails = await api.mailList();
    } catch (e) {
      notify(String(e));
    }
  }

  async function openMail(id: number) {
    selected = await api.mailGet(id);
    view = selected?.text ? "text" : selected?.html ? "html" : "raw";
  }

  async function clearAll() {
    notify(await api.mailClear());
    selected = null;
    await load();
  }

  $effect(() => {
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  });

  function fmtSize(n: number): string {
    return n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`;
  }
</script>

<div class="mailbar">
  <span class="count">{mails.length} message{mails.length === 1 ? "" : "s"}</span>
  <div class="spacer"></div>
  <button class="btn" onclick={clearAll} disabled={mails.length === 0}>Clear all</button>
</div>

{#if mails.length === 0}
  <div class="empty">
    No captured emails. Point your app's SMTP at <span class="mono">127.0.0.1:1025</span>.
  </div>
{:else}
  <div class="mail-grid">
    <div class="mail-list">
      {#each mails as m (m.id)}
        <button
          class="mail-item {selected?.id === m.id ? 'active' : ''}"
          onclick={() => openMail(m.id)}
        >
          <div class="row1">
            <span class="from">{m.from}</span>
            <span class="size mono">{fmtSize(m.size)}</span>
          </div>
          <div class="subj">{m.subject || "(no subject)"}</div>
          <div class="to mono">to {m.to.join(", ")}</div>
        </button>
      {/each}
    </div>

    <div class="mail-view">
      {#if !selected}
        <div class="empty">Select a message</div>
      {:else}
        <div class="headers">
          <div><b>From</b> {selected.from}</div>
          <div><b>To</b> {selected.to.join(", ")}</div>
          <div><b>Subject</b> {selected.subject || "(no subject)"}</div>
          <div class="mono date">{selected.received_at}</div>
        </div>
        <div class="view-tabs">
          {#if selected.text}
            <button class="vt {view === 'text' ? 'on' : ''}" onclick={() => (view = "text")}>Text</button>
          {/if}
          {#if selected.html}
            <button class="vt {view === 'html' ? 'on' : ''}" onclick={() => (view = "html")}>HTML</button>
          {/if}
          <button class="vt {view === 'raw' ? 'on' : ''}" onclick={() => (view = "raw")}>Raw</button>
        </div>
        <div class="body">
          {#if view === "html" && selected.html}
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            <iframe title="email html" srcdoc={selected.html}></iframe>
          {:else if view === "text" && selected.text}
            <pre>{selected.text}</pre>
          {:else}
            <pre class="mono">{selected.raw}</pre>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .mailbar {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
  }
  .count {
    font-size: 12px;
    color: var(--text-dim);
  }
  .mail-grid {
    display: grid;
    grid-template-columns: 320px 1fr;
    gap: 12px;
    height: calc(100vh - 180px);
  }
  .mail-list {
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
  }
  .mail-item {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 0;
    border-bottom: 1px solid var(--border);
    padding: 10px 12px;
    color: var(--text);
  }
  .mail-item:hover {
    background: var(--bg-3);
  }
  .mail-item.active {
    background: var(--accent-2);
  }
  .row1 {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .from {
    font-weight: 600;
    font-size: 12px;
  }
  .size {
    font-size: 10px;
    color: var(--text-dim);
  }
  .subj {
    font-size: 13px;
    margin: 2px 0;
  }
  .to {
    font-size: 10px;
  }
  .mail-view {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .headers {
    padding: 14px 16px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    line-height: 1.7;
  }
  .headers b {
    display: inline-block;
    width: 60px;
    color: var(--text-dim);
    font-weight: 500;
  }
  .headers .date {
    color: var(--text-dim);
    font-size: 11px;
    margin-top: 4px;
  }
  .view-tabs {
    display: flex;
    gap: 4px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }
  .vt {
    background: var(--bg-3);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 6px;
    padding: 3px 10px;
    font-size: 11px;
  }
  .vt.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 14px 16px;
  }
  .body pre {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text);
  }
  .body iframe {
    width: 100%;
    height: 100%;
    border: 0;
    background: #fff;
    border-radius: 6px;
  }
</style>
