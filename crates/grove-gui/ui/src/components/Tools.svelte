<script lang="ts">
  import { api } from "../lib/api";

  let { notify }: { notify: (m: string) => void } = $props();

  // Herd's MySQL defaults: 127.0.0.1:3306, root, no password.
  let host = $state("127.0.0.1");
  let port = $state(3306);
  let user = $state("root");
  let password = $state("");

  let busy = $state(false);
  let result = $state<string | null>(null);
  let error = $state<string | null>(null);

  let restarting = $state(false);
  async function restartDaemon() {
    restarting = true;
    try {
      notify(await api.restartDaemon());
    } catch (e) {
      notify(String(e));
    }
    restarting = false;
  }

  async function migrate() {
    busy = true;
    result = null;
    error = null;
    try {
      result = await api.mysqlMigrate(host, port, user, password);
      notify("MySQL migration complete");
    } catch (e) {
      error = String(e);
    }
    busy = false;
  }
</script>

<div class="tools">
  <div class="card">
    <div class="card-head">
      <div>
        <h3>Migrate MySQL from Herd</h3>
        <p class="muted">
          Copies all databases from another MySQL server (e.g. Laravel Herd)
          into Grove's MySQL, using a safe logical dump &amp; restore. The source
          databases are left untouched.
        </p>
      </div>
      <span class="badge">🛢 MySQL</span>
    </div>

    <div class="form">
      <label>
        <span>Source host</span>
        <input class="inp" bind:value={host} placeholder="127.0.0.1" />
      </label>
      <label>
        <span>Port</span>
        <input class="inp" type="number" bind:value={port} />
      </label>
      <label>
        <span>User</span>
        <input class="inp" bind:value={user} placeholder="root" />
      </label>
      <label>
        <span>Password</span>
        <input class="inp" type="password" bind:value={password} placeholder="(empty for Herd)" />
      </label>
    </div>

    <div class="actions">
      <button class="btn primary" disabled={busy} onclick={migrate}>
        {busy ? "Migrating…" : "Migrate databases"}
      </button>
    </div>

    <p class="hint muted">
      Grove's MySQL must be installed and running on a <em>different</em> port than
      the source. If both use 3306, change Grove's MySQL port under
      <strong>Services</strong> (e.g. 3307), start it, then migrate.
    </p>

    {#if result}
      <div class="banner ok">{result}</div>
    {/if}
    {#if error}
      <div class="banner err">{error}</div>
    {/if}
  </div>

  <div class="card">
    <div class="card-head">
      <div>
        <h3>Restart daemon</h3>
        <p class="muted">
          Restarts Grove's background service. Use this after updating the app so
          the running daemon picks up the new version. No password needed.
        </p>
      </div>
      <span class="badge">↻ Service</span>
    </div>
    <div class="actions">
      <button class="btn" disabled={restarting} onclick={restartDaemon}>
        {restarting ? "Restarting…" : "Restart daemon"}
      </button>
    </div>
  </div>
</div>

<style>
  .tools {
    max-width: 720px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .card {
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 18px;
  }
  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 16px;
  }
  .card-head h3 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .muted {
    color: var(--text-dim);
    font-size: 13px;
    margin: 0;
  }
  .badge {
    flex: none;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .form {
    display: grid;
    grid-template-columns: 2fr 1fr;
    gap: 12px;
    margin: 18px 0 8px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .inp {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    padding: 8px 10px;
    font: inherit;
    font-size: 13px;
  }
  .actions {
    margin-top: 10px;
  }
  .btn {
    background: var(--panel);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 9px 16px;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }
  .btn.primary {
    background: var(--brand);
    border-color: var(--brand);
    color: #1a1015;
    font-weight: 600;
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .hint {
    margin-top: 12px;
    font-size: 12px;
  }
  .banner {
    margin-top: 14px;
    padding: 10px 12px;
    border-radius: 8px;
    font-size: 13px;
    white-space: pre-wrap;
  }
  .banner.ok {
    border: 1px solid var(--green);
    background: color-mix(in srgb, var(--green) 14%, transparent);
    color: var(--green);
  }
  .banner.err {
    border: 1px solid var(--red);
    background: color-mix(in srgb, var(--red) 14%, transparent);
    color: var(--red);
  }
</style>
