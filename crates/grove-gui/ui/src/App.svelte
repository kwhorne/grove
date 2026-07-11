<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { check as checkUpdate, type Update } from "@tauri-apps/plugin-updater";
  import { relaunch } from "@tauri-apps/plugin-process";
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
  import NodePanel from "./components/NodePanel.svelte";
  import Doctor from "./components/Doctor.svelte";
  import Mail from "./components/Mail.svelte";
  import Tunnels from "./components/Tunnels.svelte";
  import Tools from "./components/Tools.svelte";
  import Requests from "./components/Requests.svelte";
  import Webhooks from "./components/Webhooks.svelte";
  import Database from "./components/Database.svelte";
  import Logs from "./components/Logs.svelte";
  import AboutModal from "./components/AboutModal.svelte";
  import SettingsModal from "./components/SettingsModal.svelte";
  import NewSiteModal from "./components/NewSiteModal.svelte";

  type Tab = "sites" | "services" | "mail" | "php" | "node" | "tunnels" | "requests" | "webhooks" | "database" | "tools" | "logs" | "doctor";

  let tab = $state<Tab>("sites");
  let running = $state(false);
  let status = $state<DaemonStatus | null>(null);
  let sites = $state<ResolvedSite[]>([]);
  let php = $state<PhpBuild[]>([]);
  let nodeVersions = $state<string[]>([]);
  let diagnostics = $state<DiagnosticEntry[]>([]);
  let toast = $state<string | null>(null);
  let loading = $state(true);
  let aboutOpen = $state(false);
  let settingsOpen = $state(false);
  let newSiteOpen = $state(false);

  // ---- auto-update ----
  let update = $state<Update | null>(null);
  let updateStatus = $state<"" | "downloading" | "ready" | "error">("");
  let updateProgress = $state(0);
  let updateError = $state("");
  let updateDismissed = $state(false);

  async function checkForUpdate(manual = false) {
    try {
      const u = await checkUpdate();
      if (u) {
        update = u;
        updateDismissed = false;
      } else if (manual) {
        notify("You're on the latest version.");
      }
    } catch (e) {
      if (manual) notify(`Update check failed: ${e}`);
      else console.warn("update check failed", e);
    }
  }

  async function installUpdate() {
    if (!update) return;
    updateStatus = "downloading";
    let total = 0;
    let got = 0;
    try {
      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") total = ev.data?.contentLength ?? 0;
        else if (ev.event === "Progress") {
          got += ev.data.chunkLength;
          updateProgress = total ? Math.round((got / total) * 100) : 0;
        } else if (ev.event === "Finished") updateProgress = 100;
      });
      updateStatus = "ready";
      await relaunch();
    } catch (e) {
      updateStatus = "error";
      updateError = String(e);
    }
  }

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
      nodeVersions = (await api.nodeList())
        .filter((n) => n.installed)
        .map((n) => n.major);
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
    checkForUpdate(false); // silent check on startup
    const id = setInterval(refresh, 4000);
    return () => clearInterval(id);
  });

  // Cmd/Ctrl+, opens Settings, matching the macOS convention.
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        settingsOpen = true;
      }
      if (e.key === "Escape") {
        settingsOpen = false;
        aboutOpen = false;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const phpVersions = $derived(php.map((b) => b.version));

  const navItems: { id: Tab; icon: string; label: string }[] = [
    { id: "sites", icon: "◰", label: "Sites" },
    { id: "services", icon: "⚙", label: "Services" },
    { id: "mail", icon: "✉", label: "Mail" },
    { id: "php", icon: "🐘", label: "PHP" },
    { id: "node", icon: "⬢", label: "Node" },
    { id: "tunnels", icon: "🌍", label: "Tunnels" },
    { id: "requests", icon: "⇄", label: "Requests" },
    { id: "webhooks", icon: "🪝", label: "Webhooks" },
    { id: "database", icon: "🗄", label: "Database" },
    { id: "tools", icon: "🛠", label: "Tools" },
    { id: "logs", icon: "≡", label: "Logs" },
    { id: "doctor", icon: "✚", label: "Doctor" },
  ];

  // Import existing projects: pick a directory and park it (every subfolder
  // becomes a <name>.test site).
  async function parkFolder() {
    const picked = await open({ directory: true, multiple: false, title: "Choose a directory to park" });
    if (typeof picked === "string") {
      try {
        notify(await api.park(picked));
        await refresh();
      } catch (e) {
        notify(String(e));
      }
    }
  }

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
      {running ? "Running" : "Stopped"}
    </span>
    <button class="btn" onclick={toggleDaemon}>{running ? "Stop" : "Start"}</button>
    <button class="btn icon" title="Settings (⌘,)" onclick={() => (settingsOpen = true)}>⚙</button>
  </header>

  {#if update && !updateDismissed}
    <div class="update-bar">
      {#if updateStatus === "error"}
        <span>⚠ Update failed: {updateError}</span>
        <button class="btn" onclick={() => (updateStatus = "")}>Dismiss</button>
      {:else if updateStatus === "downloading"}
        <span>Downloading update… {updateProgress}%</span>
      {:else}
        <span>⬆ Update available: <strong>v{update.version}</strong></span>
        <div class="spacer"></div>
        <button class="btn" onclick={() => (updateDismissed = true)}>Later</button>
        <button class="btn primary" onclick={installUpdate}>Install &amp; restart</button>
      {/if}
    </div>
  {/if}

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
        <div class="page-head">
          <div>
            <h2>Sites</h2>
            <p class="subtitle">Everything Grove is serving on .{status?.tld ?? "test"}</p>
          </div>
          <div class="head-actions">
            <button class="btn" onclick={parkFolder}>Park folder…</button>
            <button class="btn primary" onclick={() => (newSiteOpen = true)}>+ New site</button>
          </div>
        </div>
        <SiteTable {sites} {phpVersions} {nodeVersions} {notify} onchange={refresh} />
      {:else if tab === "services"}
        <h2>Services</h2>
        <p class="subtitle">Local services managed by Grove</p>
        <Services services={status?.services ?? []} {notify} />
      {:else if tab === "mail"}
        <h2>Mail</h2>
        <p class="subtitle">Outgoing email captured by the built-in mail-catcher</p>
        <Mail {notify} />
      {:else if tab === "php"}
        <h2>PHP runtimes</h2>
        <p class="subtitle">Install and manage PHP versions</p>
        <PhpPanel {php} {notify} />
      {:else if tab === "node"}
        <h2>Node.js</h2>
        <p class="subtitle">Install and manage Node.js versions</p>
        <NodePanel {notify} />
      {:else if tab === "tunnels"}
        <h2>Tunnels</h2>
        <p class="subtitle">Share local sites publicly and inspect incoming requests</p>
        <Tunnels {sites} {notify} />
      {:else if tab === "requests"}
        <h2>Requests</h2>
        <p class="subtitle">A live timeline of every request Grove proxied — any site, any framework</p>
        <Requests {sites} />
      {:else if tab === "webhooks"}
        <h2>Webhooks</h2>
        <p class="subtitle">Capture incoming webhooks locally, inspect them, and re-deliver to your app while you fix the handler</p>
        <Webhooks {sites} {notify} />
      {:else if tab === "database"}
        <h2>Database</h2>
        <p class="subtitle">Browse and query your sites' databases — auto-connected from each project's .env</p>
        <Database {notify} />
      {:else if tab === "tools"}
        <h2>Tools</h2>
        <p class="subtitle">Migrations and one-off utilities</p>
        <Tools {notify} />
      {:else if tab === "logs"}
        <h2>Logs</h2>
        <p class="subtitle">Application and service logs</p>
        <Logs {notify} />
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
  <SettingsModal open={settingsOpen} onclose={() => (settingsOpen = false)} {notify} />
  <NewSiteModal
    open={newSiteOpen}
    onclose={() => {
      newSiteOpen = false;
      refresh();
    }}
    {notify}
  />
</div>
