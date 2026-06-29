// Typed wrappers over the Tauri commands defined in grove-gui's Rust backend.
// Every call ultimately becomes a grove-ipc request to the daemon.

import { invoke } from "@tauri-apps/api/core";
import type {
  CapturedEmail,
  DaemonStatus,
  DiagnosticEntry,
  EmailSummary,
  PhpBuild,
  ResolvedSite,
  ServiceStatus,
  SettingsPatch,
  SettingsView,
} from "./types";

export const api = {
  daemonRunning: (): Promise<boolean> => invoke("daemon_running"),
  startDaemon: (): Promise<void> => invoke("start_daemon"),
  stopDaemon: (): Promise<void> => invoke("stop_daemon"),

  status: (): Promise<DaemonStatus> => invoke("get_status"),
  listSites: (): Promise<ResolvedSite[]> => invoke("list_sites"),

  secure: (name: string, enable: boolean): Promise<string> =>
    invoke("secure_site", { name, enable }),
  isolate: (name: string, version: string | null): Promise<string> =>
    invoke("isolate_site", { name, version }),
  park: (path: string): Promise<string> => invoke("park_dir", { path }),
  unpark: (path: string): Promise<string> => invoke("unpark_dir", { path }),
  link: (path: string, name: string | null): Promise<string> =>
    invoke("link_dir", { path, name }),
  unlink: (name: string): Promise<string> => invoke("unlink_site", { name }),
  proxy: (name: string, url: string): Promise<string> =>
    invoke("proxy_site", { name, url }),

  doctor: (): Promise<DiagnosticEntry[]> => invoke("doctor"),
  phpList: (): Promise<PhpBuild[]> => invoke("php_list"),

  mailList: (): Promise<EmailSummary[]> => invoke("mail_list"),
  mailGet: (id: number): Promise<CapturedEmail | null> => invoke("mail_get", { id }),
  mailClear: (): Promise<string> => invoke("mail_clear"),

  getSettings: (): Promise<SettingsView> => invoke("get_settings"),
  updateSettings: (patch: SettingsPatch): Promise<string> =>
    invoke("update_settings", { patch }),

  serviceList: (): Promise<ServiceStatus[]> => invoke("service_list"),
  serviceInstall: (key: string): Promise<string> => invoke("service_install", { key }),
  serviceStart: (key: string): Promise<string> => invoke("service_start", { key }),
  serviceStop: (key: string): Promise<string> => invoke("service_stop", { key }),
  serviceRestart: (key: string): Promise<string> => invoke("service_restart", { key }),

  openUrl: (url: string): Promise<void> => invoke("open_url", { url }),
  openPath: (path: string): Promise<void> => invoke("open_path", { path }),
};
