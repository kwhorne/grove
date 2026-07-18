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
  node?: string;
  secure: boolean;
  kind: SiteKind;
  proxy_to?: string;
  front_controller?: string;
  docker?: boolean;
  docker_id?: string;
  docker_running?: boolean;
}

export interface ServiceState {
  name: string;
  running: boolean;
  port?: number;
}

export interface ServiceStatus {
  key: string;
  name: string;
  category: string;
  installed: boolean;
  running: boolean;
  port: number;
  version: string;
  host: string;
  username?: string;
  socket?: string;
  uri: string;
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

export interface TunnelStatus {
  site: string;
  public_url: string;
  public_host: string;
  started_at_ms: number;
  request_count: number;
}

export interface TunnelRequestEntry {
  site: string;
  at_unix_ms: number;
  method: string;
  path: string;
  status: number;
  duration_ms: number;
}

export interface DbConnInfo {
  key: string;
  label: string;
  engine: string;
  database: string;
  environment: string;
  is_prod: boolean;
}

export interface ColumnInfo {
  name: string;
  data_type: string;
  nullable: boolean;
  key: string;
}

export interface IndexRow {
  name: string;
  unique: boolean;
  columns: string[];
}

export interface FkRow {
  table: string;
  column: string;
  ref_table: string;
  ref_column: string;
}

export type PkPair = [string, string | null];

export interface DbQueryResult {
  columns: string[];
  rows: (string | null)[][];
  rows_affected: number | null;
  elapsed_ms: number;
  is_select: boolean;
  truncated: boolean;
}

export interface LicenseClaims {
  v: number;
  id: string;
  plan: string;
  seats: number;
  email: string;
  iat: number;
  exp: number;
}

export interface RequestEntry {
  id: number;
  time: string;
  epoch_ms: number;
  site: string;
  method: string;
  path: string;
  status: number;
  duration_ms: number;
  https: boolean;
}

export interface RequestDetail {
  id: number;
  method: string;
  host: string;
  path: string;
  https: boolean;
  status: number;
  headers: [string, string][];
  body: string;
  body_truncated: boolean;
}

export interface XdebugBuild {
  version: string;
  availability: string;
  ready: boolean;
}

export interface XdebugStatus {
  enabled: boolean;
  port: number;
  builds: XdebugBuild[];
}

export interface DbConnSpec {
  kind: string; // "mysql" | "postgres" | "sqlite"
  host: string;
  port: number;
  user: string;
  password: string;
  database: string;
  path: string;
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
  received_ms?: number;
  size: number;
}

export interface QueryEvent {
  epoch_ms: number;
  engine: string;
  sql: string;
}

export interface ChainMetrics {
  duration_ms: number;
  email_count: number;
  query_count: number;
}

export interface RequestChain {
  request: RequestEntry;
  window_start_ms: number;
  window_end_ms: number;
  emails: EmailSummary[];
  queries: QueryEvent[];
  metrics: ChainMetrics;
}

export interface SqlCaptureState {
  enabled: boolean;
  note: string;
}

export interface ExplainBundle {
  summary: string;
  is_error: boolean;
  request: RequestDetail;
  chain: RequestChain;
  logs: LogEntry[];
}

export interface CapturedEmail extends EmailSummary {
  raw: string;
  text?: string;
  html?: string;
}

export interface SettingsView {
  tld: string;
  default_php: string;
  auto_start: boolean;
  http_port: number;
  https_port: number;
  dns_port: number;
  mail_enabled: boolean;
  mail_port: number;
  parked: string[];
  php_versions: string[];
}

export interface NodeVersion {
  major: string;
  installed: boolean;
  version?: string;
}

export interface LogSource {
  name: string;
  path: string;
  kind: string;
}

export interface LogEntry {
  level: string;
  datetime: string;
  message: string;
  context?: string;
}

export interface SettingsPatch {
  tld?: string;
  default_php?: string;
  auto_start?: boolean;
  http_port?: number;
  https_port?: number;
  dns_port?: number;
  mail_enabled?: boolean;
  mail_port?: number;
}
