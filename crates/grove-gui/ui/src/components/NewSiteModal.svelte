<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api } from "../lib/api";

  let { open: isOpen = false, onclose, notify }: {
    open: boolean;
    onclose: () => void;
    notify: (m: string) => void;
  } = $props();

  type Kind = "laravel" | "static" | "link";
  let kind = $state<Kind>("laravel");
  let name = $state("");
  let parent = $state("~/Code");
  let php = $state("");
  let initGit = $state(false);
  let phpVersions = $state<string[]>([]);
  let busy = $state(false);

  $effect(() => {
    if (isOpen) {
      api.phpList().then((b) => {
        phpVersions = b.map((x) => x.version);
        if (!php && phpVersions.length) php = phpVersions[0];
      });
    }
  });

  async function browseParent() {
    const picked = await open({ directory: true, multiple: false, title: "Choose the parent directory" });
    if (typeof picked === "string") parent = picked;
  }

  async function browseExisting() {
    const picked = await open({ directory: true, multiple: false, title: "Choose an existing project" });
    if (typeof picked === "string") {
      busy = true;
      try {
        notify(await api.link(picked, null));
        reset();
        onclose();
      } catch (e) {
        notify(String(e));
      }
      busy = false;
    }
  }

  async function create() {
    if (!name.trim()) {
      notify("Enter a project name");
      return;
    }
    busy = true;
    try {
      notify(await api.createSite(name.trim(), parent, kind, kind === "laravel" ? php || null : null, initGit));
      reset();
      onclose();
    } catch (e) {
      notify(String(e));
    }
    busy = false;
  }

  function reset() {
    name = "";
    initGit = false;
  }

  const kinds: { id: Kind; label: string; desc: string }[] = [
    { id: "laravel", label: "Laravel", desc: "Fresh Laravel app via Composer" },
    { id: "static", label: "Static", desc: "Plain HTML site" },
    { id: "link", label: "Link existing", desc: "Use a folder you already have" },
  ];
</script>

{#if isOpen}
  <div class="overlay" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="modal" role="dialog" aria-modal="true">
      <h3>Create a new site</h3>

      <div class="kinds">
        {#each kinds as k (k.id)}
          <button class="kind {kind === k.id ? 'on' : ''}" onclick={() => (kind = k.id)}>
            <div class="klabel">{k.label}</div>
            <div class="kdesc">{k.desc}</div>
          </button>
        {/each}
      </div>

      {#if kind === "link"}
        <p class="hint">Pick an existing project folder; Grove will serve it as <span class="mono">&lt;folder&gt;.test</span>.</p>
        <div class="actions">
          <button class="btn" onclick={onclose}>Cancel</button>
          <button class="btn primary" disabled={busy} onclick={browseExisting}>
            {busy ? "Linking…" : "Choose folder…"}
          </button>
        </div>
      {:else}
        <div class="field">
          <label for="ns-name">Project name</label>
          <input id="ns-name" class="inp" placeholder="my-project" bind:value={name} />
        </div>
        <div class="field">
          <label for="ns-parent">Parent directory</label>
          <div class="path-row">
            <input id="ns-parent" class="inp mono" bind:value={parent} />
            <button class="btn" onclick={browseParent}>Browse…</button>
          </div>
        </div>
        {#if kind === "laravel"}
          <div class="field">
            <label for="ns-php">PHP version</label>
            <select id="ns-php" class="inp" bind:value={php}>
              {#if phpVersions.length === 0}
                <option value="">default</option>
              {/if}
              {#each phpVersions as v}
                <option value={v}>{v}</option>
              {/each}
            </select>
          </div>
          <div class="field row">
            <label for="ns-git">Initialize a git repository</label>
            <button id="ns-git" aria-label="toggle git" class="toggle {initGit ? 'on' : ''}" onclick={() => (initGit = !initGit)}>
              <span class="knob"></span>
            </button>
          </div>
        {/if}
        <div class="actions">
          <button class="btn" onclick={onclose}>Cancel</button>
          <button class="btn primary" disabled={busy} onclick={create}>
            {busy ? "Creating… (this can take a moment)" : "Create site"}
          </button>
        </div>
      {/if}
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
    padding: 22px 24px;
    width: 480px;
    max-width: 92vw;
    box-shadow: 0 20px 56px rgba(0, 0, 0, 0.55);
  }
  h3 {
    margin: 0 0 16px;
    font-size: 16px;
  }
  .kinds {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin-bottom: 18px;
  }
  .kind {
    text-align: left;
    background: var(--bg-3);
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 10px 12px;
    color: var(--text);
  }
  .kind.on {
    border-color: var(--accent);
  }
  .klabel {
    font-weight: 600;
    font-size: 13px;
  }
  .kdesc {
    font-size: 10px;
    color: var(--text-dim);
    margin-top: 2px;
  }
  .field {
    margin-bottom: 12px;
  }
  .field.row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  label {
    display: block;
    font-size: 12px;
    color: var(--text-dim);
    margin-bottom: 5px;
  }
  .field.row label {
    margin-bottom: 0;
  }
  .inp {
    width: 100%;
    background: var(--bg-3);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px 10px;
    font-size: 13px;
  }
  .inp:focus {
    border-color: var(--accent);
    outline: none;
  }
  .path-row {
    display: flex;
    gap: 8px;
  }
  .path-row .inp {
    flex: 1;
  }
  .path-row .btn {
    flex: none;
  }
  .hint {
    font-size: 12px;
    color: var(--text-dim);
    line-height: 1.5;
    margin: 0 0 16px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 18px;
  }
</style>
