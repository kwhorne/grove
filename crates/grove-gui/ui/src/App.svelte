<script lang="ts">
  import { api } from "./lib/api";
  import type {
    DaemonStatus,
    DiagnosticEntry,
    PhpBuild,
    ResolvedSite,
  } from "./lib/types";
  import SiteTable from "./components/SiteTable.svelte";
  import Services from "./components/Services.svelte";
  import PhpPanel from "./components/PhpPanel.svelte";
  import Doctor from "./components/Doctor.svelte";

  type Tab = "sites" | "services" | "php" | "doctor";

  let tab = $state<Tab>("sites");
  let running = $state(false);
  let status = $state<DaemonStatus | null>(null);
  let sites = $state<ResolvedSite[]>([]);
  let php = $state<PhpBuild[]>([]);
  let diagnostics = $state<DiagnosticEntry[]>([]);
  let toast = $state<string | null>(null);
  let loading = $state(true);

  function notify(msg: string) {
    toast = msg;
    setTimeout(() => (toast = null), 2600);
  }

  async function refresh() {
    running = await api.daemonRunning();
    if (!running) {
      status = null;
      sites = [];
      loading = false;
      return;
    }
    try {
      [status, sites, php] = await Promise.all([
        api.status(),
        api.listSites(),
        api.phpList(),
      ]);
      if (tab === "doctor") diagnostics = await api.doctor();
    } catch (e) {
      notify(String(e));
    }
    loading = false;
  }

  async function toggleDaemon() {
    loading = true;
    try {
      if (running) {
        await api.stopDaemon();
        notify("daemon stopped");
      } else {
        await api.startDaemon();
        notify("daemon started");
      }
    } catch (e) {
      notify(String(e));
    }
    await refresh();
  }

  // Initial load + poll while the window is open.
  $effect(() => {
    refresh();
    const id = setInterval(refresh, 4000);
    return () => clearInterval(id);
  });

  const phpVersions = $derived(php.map((b) => b.version));
</script>

<div class="app">
  <header class="topbar">
    <div class="brand"><span class="leaf">🌳</span> Grove</div>
    {#if status}
      <div class="meta">
        <span>TLD <b>.{status.tld}</b></span>
        <span>HTTP <b>:{status.http_port}</b></span>
        <span>HTTPS <b>:{status.https_port}</b></span>
        <span>Sites <b>{status.site_count}</b></span>
      </div>
    {/if}
    <div class="spacer"></div>
    <span class="status-pill {running ? 'up' : 'down'}">
      <span class="dot"></span>
      {running ? `groved ${status?.version ?? ""}` : "stopped"}
    </span>
    <button class="btn" onclick={toggleDaemon}>
      {running ? "Stop" : "Start"}
    </button>
  </header>

  <div class="body">
    <nav class="sidebar">
      <button class="nav-item {tab === 'sites' ? 'active' : ''}" onclick={() => (tab = "sites")}>
        ◰ Sites
      </button>
      <button class="nav-item {tab === 'services' ? 'active' : ''}" onclick={() => (tab = "services")}>
        ⚙ Services
      </button>
      <button class="nav-item {tab === 'php' ? 'active' : ''}" onclick={() => (tab = "php")}>
        🐘 PHP
      </button>
      <button
        class="nav-item {tab === 'doctor' ? 'active' : ''}"
        onclick={async () => {
          tab = "doctor";
          if (running) diagnostics = await api.doctor();
        }}
      >
        ✚ Doctor
      </button>
    </nav>

    <main class="content">
      {#if !running}
        <div class="empty">
          <p>The Grove daemon is not running.</p>
          <button class="btn primary" onclick={toggleDaemon}>Start daemon</button>
        </div>
      {:else if loading}
        <div class="empty">Loading…</div>
      {:else if tab === "sites"}
        <h2>Sites</h2>
        <p class="subtitle">Everything Grove is serving on .{status?.tld ?? "test"}</p>
        <SiteTable {sites} {phpVersions} {notify} onchange={refresh} />
      {:else if tab === "services"}
        <h2>Services</h2>
        <p class="subtitle">Local services managed by Grove</p>
        <Services services={status?.services ?? []} />
      {:else if tab === "php"}
        <h2>PHP runtimes</h2>
        <p class="subtitle">Installed builds and their extensions</p>
        <PhpPanel {php} />
      {:else if tab === "doctor"}
        <h2>Doctor</h2>
        <p class="subtitle">Environment diagnostics</p>
        <Doctor {diagnostics} />
      {/if}
    </main>
  </div>

  {#if toast}
    <div class="toast">{toast}</div>
  {/if}
</div>
