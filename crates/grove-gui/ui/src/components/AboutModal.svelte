<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { getVersion } from "@tauri-apps/api/app";

  let { open = false, onclose }: { open: boolean; onclose: () => void } = $props();

  let version = $state("");
  $effect(() => {
    if (open && !version) {
      getVersion()
        .then((v) => (version = v))
        .catch(() => {});
    }
  });

  function openUrl(url: string) {
    invoke("open_url", { url }).catch(() => {});
  }
</script>

{#if open}
  <div
    class="overlay"
    role="presentation"
    onclick={(e) => e.target === e.currentTarget && onclose()}
  >
    <div class="modal" role="dialog" aria-modal="true" tabindex="-1">
      <div class="logo">
        <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" width="56" height="56">
          <rect width="24" height="24" rx="5" fill="#0a0a0a" />
          <g stroke="#fb923c" stroke-width="1" stroke-linecap="round" opacity="0.45">
            <line x1="12" y1="3.5" x2="7" y2="10.5" />
            <line x1="12" y1="3.5" x2="16" y2="9.5" />
            <line x1="7" y1="10.5" x2="16" y2="9.5" />
            <line x1="7" y1="10.5" x2="9" y2="19" />
            <line x1="16" y1="9.5" x2="17.5" y2="18" />
            <line x1="9" y1="19" x2="17.5" y2="18" />
          </g>
          <circle cx="12" cy="3.5" r="1.9" fill="#fb923c" />
          <circle cx="7" cy="10.5" r="1.15" fill="#fb923c" />
          <circle cx="16" cy="9.5" r="1.15" fill="#fb923c" />
          <circle cx="9" cy="19" r="1.15" fill="#fb923c" />
          <circle cx="17.5" cy="18" r="1.15" fill="#fb923c" />
        </svg>
      </div>
      <div class="name">Elyra Grove</div>
      <div class="ver">Version {version || "…"}</div>
      <p class="tag">
        A native local development environment in Rust — automatic .test routing,
        local HTTPS, multi-version PHP and zero external dependencies.
      </p>

      <div class="links">
        <button class="link" onclick={() => openUrl("https://elyracode.com/grove")}>
          <span class="lbl">Website</span><span class="url">elyracode.com/grove</span>
        </button>
        <button class="link" onclick={() => openUrl("https://github.com/kwhorne/grove")}>
          <span class="lbl">GitHub</span><span class="url">github.com/kwhorne/grove</span>
        </button>
        <button class="link" onclick={() => openUrl("https://kwhorne.com/")}>
          <span class="lbl">Developed by</span><span class="url">Knut W. Horne · kwhorne.com</span>
        </button>
      </div>

      <button class="close" onclick={onclose}>Close</button>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }
  .modal {
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 14px;
    padding: 26px 28px;
    width: 380px;
    max-width: 92vw;
    text-align: center;
    box-shadow: 0 20px 56px rgba(0, 0, 0, 0.55);
  }
  .logo {
    display: flex;
    justify-content: center;
    margin-bottom: 10px;
  }
  .name {
    font-size: 18px;
    font-weight: 700;
  }
  .ver {
    font-size: 12px;
    color: var(--text-dim);
    margin-top: 2px;
    font-family: var(--font-mono);
  }
  .tag {
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.5;
    margin: 14px 0 18px;
  }
  .links {
    display: flex;
    flex-direction: column;
    gap: 6px;
    text-align: left;
  }
  .link {
    display: flex;
    flex-direction: column;
    gap: 1px;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 8px 12px;
    cursor: pointer;
  }
  .link:hover {
    border-color: var(--accent);
  }
  .lbl {
    font-size: 10px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .url {
    font-size: 13px;
    color: var(--accent);
    font-family: var(--font-mono);
  }
  .close {
    margin-top: 18px;
    background: var(--bg-3);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 7px 18px;
    font-size: 12px;
    cursor: pointer;
  }
  .close:hover {
    border-color: var(--accent);
  }
</style>
