<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { DbConnSpec, XdebugStatus } from "../lib/types";

  let { notify }: { notify: (m: string) => void } = $props();

  // ---- Xdebug ----
  let xdebug = $state<XdebugStatus | null>(null);
  let xdBusy = $state(false);

  async function loadXdebug() {
    try {
      xdebug = await api.debugStatus();
    } catch {
      /* daemon down */
    }
  }
  onMount(loadXdebug);

  async function toggleXdebug() {
    if (!xdebug) return;
    xdBusy = true;
    try {
      xdebug = await api.debugSet(!xdebug.enabled);
      notify(xdebug.enabled ? "Xdebug enabled" : "Xdebug disabled");
    } catch (e) {
      notify(String(e));
    }
    xdBusy = false;
  }



  // ---- Convert database (MySQL / PostgreSQL / SQLite) ----
  function blank(kind: string): DbConnSpec {
    return {
      kind,
      host: "127.0.0.1",
      port: kind === "postgres" ? 5432 : 3306,
      user: kind === "postgres" ? "grove" : "root",
      password: "",
      database: "",
      path: "",
    };
  }
  let src = $state<DbConnSpec>(blank("mysql"));
  let dst = $state<DbConnSpec>(blank("sqlite"));
  let converting = $state(false);
  let convResult = $state<string | null>(null);
  let convError = $state<string | null>(null);

  function retune(spec: DbConnSpec) {
    const d = blank(spec.kind);
    spec.host = d.host;
    spec.port = d.port;
    spec.user = d.user;
  }

  async function convert() {
    converting = true;
    convResult = null;
    convError = null;
    try {
      convResult = await api.dbConvert($state.snapshot(src), $state.snapshot(dst));
      notify("Database conversion complete");
    } catch (e) {
      convError = String(e);
    }
    converting = false;
  }

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
        <h3>Xdebug step-debugging</h3>
        <p class="muted">
          Load Xdebug into PHP-FPM on demand. A request opts in with the
          <code>XDEBUG_TRIGGER</code> cookie/param; your editor listens on DBGp
          port {xdebug?.port ?? 9003}. Idle requests pay almost no overhead.
        </p>
      </div>
      <button
        class="toggle {xdebug?.enabled ? 'on' : ''}"
        disabled={xdBusy || !xdebug}
        onclick={toggleXdebug}
        title="Toggle Xdebug"
        aria-label="Toggle Xdebug">
        <span class="knob"></span>
      </button>
    </div>

    {#if xdebug && xdebug.builds.length > 0}
      <div class="builds">
        {#each xdebug.builds as b (b.version)}
          <div class="brow">
            <span class="mono ver">php@{b.version}</span>
            <span class="avail {b.ready ? 'ok' : 'no'}">{b.availability}</span>
          </div>
        {/each}
      </div>
    {:else if xdebug}
      <p class="hint muted">No PHP builds installed yet.</p>
    {/if}

    <p class="hint muted">
      Step-debugging needs a PHP that has Xdebug — register one with
      <code>grove php register</code> (Grove's fully-static builds can't load it).
      For CLI (artisan, tests): <code>eval "$(grove debug env)"</code>, and point
      your editor's DBGp/DAP listener at port {xdebug?.port ?? 9003}.
    </p>
  </div>

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
        <h3>Convert database</h3>
        <p class="muted">
          Copy a whole database between MySQL, PostgreSQL and SQLite (tables,
          columns, primary keys and data). Great for turning a MySQL database
          into a portable SQLite file, and back.
        </p>
      </div>
      <span class="badge">⇄ Convert</span>
    </div>

    <div class="convert-grid">
      <div class="endpoint">
        <div class="ep-title">Source</div>
        <label>
          <span>Type</span>
          <select class="inp" bind:value={src.kind} onchange={() => retune(src)}>
            <option value="mysql">MySQL</option>
            <option value="postgres">PostgreSQL</option>
            <option value="sqlite">SQLite</option>
          </select>
        </label>
        {#if src.kind === "sqlite"}
          <label><span>File path</span>
            <input class="inp" bind:value={src.path} placeholder="/Users/you/database.sqlite" /></label>
        {:else}
          <div class="row2">
            <label><span>Host</span><input class="inp" bind:value={src.host} /></label>
            <label><span>Port</span><input class="inp" type="number" bind:value={src.port} /></label>
          </div>
          <div class="row2">
            <label><span>User</span><input class="inp" bind:value={src.user} /></label>
            <label><span>Password</span><input class="inp" type="password" bind:value={src.password} /></label>
          </div>
          <label><span>Database</span><input class="inp" bind:value={src.database} placeholder="my_app" /></label>
        {/if}
      </div>

      <div class="arrow">→</div>

      <div class="endpoint">
        <div class="ep-title">Target</div>
        <label>
          <span>Type</span>
          <select class="inp" bind:value={dst.kind} onchange={() => retune(dst)}>
            <option value="sqlite">SQLite</option>
            <option value="mysql">MySQL</option>
            <option value="postgres">PostgreSQL</option>
          </select>
        </label>
        {#if dst.kind === "sqlite"}
          <label><span>File path (created if missing)</span>
            <input class="inp" bind:value={dst.path} placeholder="/Users/you/database.sqlite" /></label>
        {:else}
          <div class="row2">
            <label><span>Host</span><input class="inp" bind:value={dst.host} /></label>
            <label><span>Port</span><input class="inp" type="number" bind:value={dst.port} /></label>
          </div>
          <div class="row2">
            <label><span>User</span><input class="inp" bind:value={dst.user} /></label>
            <label><span>Password</span><input class="inp" type="password" bind:value={dst.password} /></label>
          </div>
          <label><span>Database (must exist)</span><input class="inp" bind:value={dst.database} placeholder="my_app" /></label>
        {/if}
      </div>
    </div>

    <div class="actions">
      <button class="btn primary" disabled={converting} onclick={convert}>
        {converting ? "Converting…" : "Convert"}
      </button>
    </div>
    <p class="hint muted">
      Recreates tables, columns, primary keys and copies all rows. Views, stored
      routines, triggers and foreign keys are not copied. The target database (for
      MySQL/PostgreSQL) must already exist.
    </p>
    {#if convResult}<div class="banner ok">{convResult}</div>{/if}
    {#if convError}<div class="banner err">{convError}</div>{/if}
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
  .convert-grid {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 14px;
    margin: 16px 0 4px;
  }
  .endpoint {
    display: flex;
    flex-direction: column;
    gap: 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px;
  }
  .ep-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .row2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .arrow {
    font-size: 22px;
    color: var(--brand);
  }
  .toggle {
    flex: none;
    width: 46px;
    height: 26px;
    border-radius: 13px;
    border: 1px solid var(--border);
    background: var(--bg);
    position: relative;
    cursor: pointer;
    transition: background 0.15s;
  }
  .toggle.on {
    background: var(--green);
    border-color: var(--green);
  }
  .toggle .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: #fff;
    transition: left 0.15s;
  }
  .toggle.on .knob {
    left: 22px;
  }
  .toggle:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .builds {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 16px 0 4px;
  }
  .brow {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .ver {
    font-weight: 600;
    min-width: 70px;
  }
  .avail {
    flex: 1;
    font-size: 12px;
  }
  .avail.ok {
    color: var(--green);
  }
  .avail.no {
    color: var(--text-dim);
  }
  code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--bg);
    padding: 1px 5px;
    border-radius: 4px;
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
