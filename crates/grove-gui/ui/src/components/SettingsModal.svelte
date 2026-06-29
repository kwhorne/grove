<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { api } from "../lib/api";
  import type { SettingsPatch, SettingsView } from "../lib/types";
  import { applyTheme, getTheme, type Theme } from "../lib/theme";

  let { open: isOpen = false, onclose, notify }: {
    open: boolean;
    onclose: () => void;
    notify: (m: string) => void;
  } = $props();

  type Section = "general" | "mail";
  let section = $state<Section>("general");
  let s = $state<SettingsView | null>(null);
  let parked = $state<string[]>([]);
  let originalParked = $state<string[]>([]);
  let theme = $state<Theme>(getTheme());
  let saving = $state(false);

  $effect(() => {
    if (isOpen && !s) void load();
  });

  async function load() {
    try {
      s = await api.getSettings();
      parked = [...s.parked];
      originalParked = [...s.parked];
    } catch (e) {
      notify(String(e));
    }
  }

  async function addPath() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a directory to park" });
    if (typeof picked === "string" && !parked.includes(picked)) {
      parked = [...parked, picked];
    }
  }

  function removePath(p: string) {
    parked = parked.filter((x) => x !== p);
  }

  function setTheme(t: Theme) {
    theme = t;
    applyTheme(t);
  }

  async function save() {
    if (!s) return;
    saving = true;
    try {
      const patch: SettingsPatch = {
        tld: s.tld,
        default_php: s.default_php,
        auto_start: s.auto_start,
        mail_enabled: s.mail_enabled,
        mail_port: s.mail_port,
      };
      await api.updateSettings(patch);

      // Reconcile parked directories.
      for (const p of parked) {
        if (!originalParked.includes(p)) await api.park(p);
      }
      for (const p of originalParked) {
        if (!parked.includes(p)) await api.unpark(p);
      }
      originalParked = [...parked];
      notify("settings saved");
      onclose();
    } catch (e) {
      notify(String(e));
    }
    saving = false;
  }

  function shortPath(p: string): string {
    const home = "/Users/";
    return p.startsWith(home) ? "~" + p.slice(p.indexOf("/", home.length)) : p;
  }
</script>

{#if isOpen && s}
  <div class="overlay" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
    <div class="settings" role="dialog" aria-modal="true">
      <aside class="snav">
        <div class="stitle">Settings</div>
        <button class="snav-item {section === 'general' ? 'active' : ''}" onclick={() => (section = "general")}>
          <span class="ico">⚙</span> General
        </button>
        <button class="snav-item {section === 'mail' ? 'active' : ''}" onclick={() => (section = "mail")}>
          <span class="ico">✉</span> Mail
        </button>
      </aside>

      <div class="scontent">
        {#if section === "general"}
          <h3>General</h3>

          <div class="group">
            <div class="glabel">Parked paths</div>
            <p class="ghelp">All sub-folders in these directories are served as <span class="mono">&lt;name&gt;.{s.tld}</span>.</p>
            <div class="pathlist">
              {#each parked as p (p)}
                <div class="pathrow">
                  <span class="mono">{shortPath(p)}</span>
                  <button class="btn icon" title="Remove" onclick={() => removePath(p)}>✕</button>
                </div>
              {/each}
              {#if parked.length === 0}
                <div class="pathempty">No parked directories yet.</div>
              {/if}
            </div>
            <button class="btn" onclick={addPath}>+ Add path</button>
          </div>

          <div class="field">
            <div>
              <div class="flabel">Top-level domain</div>
              <div class="fhelp">Sites are served on this TLD. Restart required.</div>
            </div>
            <input class="inp" bind:value={s.tld} style="width:120px" />
          </div>

          <div class="field">
            <div>
              <div class="flabel">Default PHP version</div>
              <div class="fhelp">Used by sites without an explicit isolate.</div>
            </div>
            <select class="inp" bind:value={s.default_php}>
              {#if !s.php_versions.includes(s.default_php)}
                <option value={s.default_php}>{s.default_php}</option>
              {/if}
              {#each s.php_versions as v}
                <option value={v}>{v}</option>
              {/each}
            </select>
          </div>

          <div class="field">
            <div>
              <div class="flabel">Launch at login</div>
              <div class="fhelp">Start the Grove daemon automatically.</div>
            </div>
            <button aria-label="Toggle launch at login" class="toggle {s.auto_start ? 'on' : ''}" onclick={() => (s!.auto_start = !s!.auto_start)}>
              <span class="knob"></span>
            </button>
          </div>

          <div class="field">
            <div>
              <div class="flabel">Theme</div>
              <div class="fhelp">Choose auto, light or dark.</div>
            </div>
            <select class="inp" value={theme} onchange={(e) => setTheme((e.currentTarget as HTMLSelectElement).value as Theme)}>
              <option value="auto">Auto</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
            </select>
          </div>
        {:else if section === "mail"}
          <h3>Mail</h3>
          <p class="ghelp">
            Configure your apps to send mail via Grove's SMTP catcher and inspect every
            outgoing message in the Mail tab.
          </p>

          <div class="field">
            <div class="flabel">Mail server status</div>
            <span class="dot {s.mail_enabled ? 'on' : ''}"></span>
          </div>

          <div class="field">
            <div>
              <div class="flabel">Enable mail-catcher</div>
              <div class="fhelp">Run the built-in SMTP server. Restart required.</div>
            </div>
            <button aria-label="Toggle mail-catcher" class="toggle {s.mail_enabled ? 'on' : ''}" onclick={() => (s!.mail_enabled = !s!.mail_enabled)}>
              <span class="knob"></span>
            </button>
          </div>

          <div class="field">
            <div>
              <div class="flabel">Mail server port</div>
              <div class="fhelp">The SMTP port apps connect to. Restart required.</div>
            </div>
            <input class="inp" type="number" bind:value={s.mail_port} style="width:90px" />
          </div>
        {/if}
      </div>

      <footer class="sfoot">
        <span class="note">Port &amp; TLD changes apply after <span class="mono">grove restart</span>.</span>
        <div class="spacer"></div>
        <button class="btn" onclick={onclose}>Cancel</button>
        <button class="btn primary" onclick={save} disabled={saving}>{saving ? "Saving…" : "Save"}</button>
      </footer>
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
  .settings {
    width: 760px;
    max-width: 94vw;
    height: 560px;
    max-height: 90vh;
    background: var(--bg-2);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: 0 20px 56px rgba(0, 0, 0, 0.55);
    display: grid;
    grid-template-columns: 190px 1fr;
    grid-template-rows: 1fr auto;
    overflow: hidden;
  }
  .snav {
    grid-row: 1 / 3;
    background: var(--bg);
    border-right: 1px solid var(--border);
    padding: 14px 8px;
  }
  .stitle {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.6px;
    color: var(--text-dim);
    padding: 4px 10px 10px;
  }
  .snav-item {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    border-radius: 7px;
    margin-bottom: 2px;
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .snav-item:hover { background: var(--bg-3); }
  .snav-item.active { background: var(--accent-2); }
  .snav-item .ico { width: 16px; text-align: center; color: var(--text-dim); }
  .snav-item.active .ico { color: var(--accent); }

  .scontent { padding: 20px 22px; overflow-y: auto; }
  h3 { margin: 0 0 16px; font-size: 16px; }

  .group { margin-bottom: 22px; }
  .glabel { font-size: 13px; font-weight: 600; margin-bottom: 2px; }
  .ghelp { color: var(--text-dim); font-size: 12px; margin: 0 0 10px; line-height: 1.5; }

  .pathlist {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--panel);
    margin-bottom: 8px;
    overflow: hidden;
  }
  .pathrow {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 7px 12px;
    border-bottom: 1px solid var(--border);
  }
  .pathrow:last-child { border-bottom: 0; }
  .pathempty { padding: 12px; color: var(--text-dim); font-size: 12px; }

  .field {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    border-top: 1px solid var(--border);
  }
  .flabel { font-size: 13px; }
  .fhelp { font-size: 11px; color: var(--text-dim); margin-top: 2px; }

  .inp {
    background: var(--bg-3);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 12px;
    font-family: var(--font-mono);
  }
  .inp:focus { border-color: var(--accent); outline: none; }

  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--text-dim); }
  .dot.on { background: var(--green); box-shadow: 0 0 8px var(--green); }

  .sfoot {
    grid-column: 2;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid var(--border);
    background: var(--bg-2);
  }
  .note { font-size: 11px; color: var(--text-dim); }
</style>
