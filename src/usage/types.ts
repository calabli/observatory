import type {
  UsageSummary,
  UsageSummaryByApp,
  DailyStats,
  ProviderStats,
  ModelStats,
  PaginatedLogs,
  LogFilters,
} from "../types/usage";
export interface SourceStatus {
  id: string;
  name: string;
  nativeSupported: boolean;
  enabled: boolean;
  requests: number;
}
export interface DeviceInfo {
  deviceId: string;
  deviceName: string;
  appVersion: string;
  platform: string;
  startedAt: number;
}
export interface NativeStatus {
  version: string;
  dataDir: string;
  databasePath: string;
  device: DeviceInfo;
  ccSwitchPath: string;
  ccSwitchAvailable: boolean;
  detailRecords: number;
  lastScanAt: number | null;
  refreshInterval: string;
  initialImportError?: string | null;
  initialImportNotice?: string | null;
  sources: SourceStatus[];
}
export interface Dashboard {
  summary: UsageSummary;
  byApp: UsageSummaryByApp[];
  trends: DailyStats[];
  providers: ProviderStats[];
  models: ModelStats[];
  logs: PaginatedLogs;
  firstDate: string | null;
  options: { models: string[]; providers: string[] };
}
export interface UsageFilters extends LogFilters {
  multiDevice: boolean;
  page: number;
  pageSize: number;
}
export interface LanSettings {
  sharedAppTypes: string[];
  enabled: boolean;
  port: number;
  deviceName: string;
  autoSync: boolean;
  autoSyncIntervalSecs: number;
}
export interface Peer {
  deviceId: string;
  deviceName: string;
  host: string;
  port: number;
  enabled: boolean;
  paired: boolean;
  lastSeenAt?: number;
  lastSyncAt?: number;
  lastError?: string;
  lastImported: number;
}
export interface LanStatus {
  settings: LanSettings;
  local: DeviceInfo;
  devices: Peer[];
}
export interface Discovered extends DeviceInfo {
  host: string;
  port: number;
}
