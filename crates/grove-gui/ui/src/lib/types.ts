// Mirrors the serde types in grove-ipc / grove-core so the Svelte UI is typed
// against the exact daemon contract.

export type Driver = "laravel" | "wordpress" | "php" | "static" | "proxy";
export type SiteKind = "parked" | "linked";

export interface ResolvedSite {
  name: string;
  hostname: string;
  path: string;
  document_root: string;
  driver: Driver;
  php: string;
  secure: boolean;
  kind: SiteKind;
  proxy_to?: string;
  front_controller?: string;
}

export interface ServiceState {
  name: string;
  running: boolean;
  port?: number;
}

export interface DaemonStatus {
  version: string;
  tld: string;
  http_port: number;
  https_port: number;
  dns_port: number;
  site_count: number;
  services: ServiceState[];
}

export type DiagnosticStatus = "pass" | "warn" | "fail";

export interface DiagnosticEntry {
  check: string;
  status: DiagnosticStatus;
  detail: string;
}

export interface PhpBuild {
  version: string;
  fpm_binary: string;
  user_registered: boolean;
}

export interface EmailSummary {
  id: number;
  from: string;
  to: string[];
  subject: string;
  received_at: string;
  size: number;
}

export interface CapturedEmail extends EmailSummary {
  raw: string;
  text?: string;
  html?: string;
}
