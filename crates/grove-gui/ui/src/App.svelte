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
  import AboutModal from "./components/AboutModal.svelte";

  type Tab = "sites" | "services" | "php" | "doctor";

  let tab = $state<Tab>("sites");
  let running = $state(false);
  let status = $state<DaemonStatus | null>(null);
  let sites = $state<ResolvedSite[]>([]);
  let php = $state<PhpBuild[]>([]);
  let diagnostics = $state<DiagnosticEntry[]>([]);
  let toast = $state<string | null>(null);
  let loading = $state(true);
  let aboutOpen = $state(false);

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

  $effect(() => {
    refresh();
    const id = setInterval(refresh, 4000);
    return () => clearInterval(id);
  });

  const phpVersions = $derived(php.map((b) => b.version));

  const navItems: { id: Tab; icon: string; label: string }[] = [
    { id: "sites", icon: "◰", label: "Sites" },
    { id: "services", icon: "⚙", label: "Services" },
    { id: "php", icon: "🐘", label: "PHP" },
    { id: "doctor", icon: "✚", label: "Doctor" },
  ];

  async function selectTab(t: Tab) {
    tab = t;
    if (t === "doctor" && running) diagnostics = await api.doctor();
  }
</script>

<div class="app">
  <header class="toolbar">
    <div class="brand">
      <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" width="22" height="22">
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
      Grove
    </div>
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
    <button class="btn" onclick={toggleDaemon}>{running ? "Stop" : "Start"}</button>
  </header>

  <div class="body">
    <nav class="sidebar">
      <div class="section-label">Manage</div>
      {#each navItems as item (item.id)}
        <button
          class="nav-item {tab === item.id ? 'active' : ''}"
          onclick={() => selectTab(item.id)}
        >
          <span class="ico">{item.icon}</span>
          {item.label}
        </button>
      {/each}
      <div class="foot">
        <button class="nav-item" onclick={() => (aboutOpen = true)}>
          <span class="ico">ⓘ</span>
          About
        </button>
      </div>
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

  <AboutModal open={aboutOpen} onclose={() => (aboutOpen = false)} />
</div>
