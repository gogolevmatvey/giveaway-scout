import { invoke } from "@tauri-apps/api/core";

export function notificationErrorKey(error: unknown) {
  const code = error instanceof Error ? error.message : String(error);
  switch (code) {
    case "MACOS_NOTIFICATION_APP_BUNDLE_REQUIRED":
      return "testNotificationMacosBundleRequired" as const;
    case "MACOS_NOTIFICATION_PERMISSION_DENIED":
      return "testNotificationMacosPermissionDenied" as const;
    case "MACOS_NOTIFICATION_PERMISSION_PENDING":
      return "testNotificationMacosPermissionPending" as const;
    default:
      return "testNotificationError" as const;
  }
}

export interface UpdateCandidate { token: string; version: string; notes: string }
export interface UpdateSnapshot {
  mode: "enabled" | "development" | "manual" | "packageManager" | "notConfigured" | "invalidConfiguration";
  phase: "idle" | "checking" | "available" | "downloading" | "installing" | "failed";
  currentVersion: string;
  candidate: UpdateCandidate | null;
  bannerDismissed: boolean;
  downloadedBytes: number;
  totalBytes: number | null;
  lastCheckedAtUtc: string | null;
  lastError: string | null;
}

export function getUpdateState(): Promise<UpdateSnapshot> { return invoke("get_update_state"); }
export function checkForUpdates(): Promise<UpdateSnapshot> { return invoke("check_for_updates"); }
export function dismissUpdate(token: string): Promise<UpdateSnapshot> { return invoke("dismiss_update", { token }); }
export function installUpdate(token: string): Promise<void> { return invoke("install_update", { token, confirmed: true }); }

export interface AppState {
  appName: string;
  version: string;
  platform: string;
  architecture: string;
  isDevelopment: boolean;
}

export interface AppSettings {
  checkIntervalMinutes: number;
  sourceCheckIntervals: SourceCheckIntervals;
  httpTimeoutSeconds: number;
  httpRetryCount: number;
  giveeClubEnabled: boolean;
  gamesForFarmEnabled: boolean;
  zerosEnabled: boolean;
  freeSteamKeysEnabled: boolean;
  steamNotificationsEnabled: boolean;
  epicGamesNotificationsEnabled: boolean;
  gogNotificationsEnabled: boolean;
  notificationSoundEnabled: boolean;
  startAtLogin: boolean;
  autostartTarget: "gui" | "cli";
  startInTray: boolean;
  keepRunningInTray: boolean;
  automaticUpdateChecks: boolean;
  languageCode: "ru" | "en" | null;
  verboseLogging: boolean;
  logMaxFileSizeMb: number;
  logFileCount: number;
  logDirectory: string | null;
  initialSyncMode: InitialSyncMode | null;
}

export interface LoggingPolicy {
  maxFileSizeMb: number;
  fileCount: number;
  directory: string | null;
}

export interface LoggingSnapshot {
  requestedAtStartup: LoggingPolicy;
  effectiveDirectory: string | null;
  activeFilePath: string | null;
  maxFileSizeMb: number;
  fileCount: number;
  restartRequired: boolean;
  lastError: string | null;
  usingFallback: boolean;
}

export function getLoggingState(): Promise<LoggingSnapshot> {
  return invoke<LoggingSnapshot>("get_logging_state");
}

export function chooseLogDirectory(): Promise<string | null> {
  return invoke<string | null>("choose_log_directory");
}

export function openLogDirectory(): Promise<void> {
  return invoke<void>("open_log_directory");
}

export type InitialSyncMode = "silent" | "notifyCurrent";

export interface DataCounts {
  trackedGiveaways: number;
  notificationHistory: number;
  unreadNotifications: number;
}

export interface DatabaseRecoveryReport {
  backupPath: string;
  originalError: string;
}

export interface StorageSnapshot {
  settings: AppSettings;
  counts: DataCounts;
  dataSizeBytes: number;
  databaseRecovery: DatabaseRecoveryReport | null;
  initialSyncChoiceRequired: boolean;
  monitor: MonitorSnapshot;
}

export interface MonitorSnapshot {
  isChecking: boolean;
  lastStartedAtUtc: string | null;
  lastCompletedAtUtc: string | null;
  lastSuccessfulCheckUtc: string | null;
  lastError: string | null;
  lastNewItems: number;
  lastFailureCount: number;
  sourceFailures: SourceFailure[];
  sourceStates: SourceStatus[];
}

export type GiveawaySource = "giveeClub" | "gamesForFarm" | "zeros" | "freeSteamKeys";
export type SourceCheckIntervals = Record<GiveawaySource, number>;
export type SourcePhase = "disabled" | "waiting" | "checking" | "ok" | "error";

export interface SourceStatus {
  source: GiveawaySource;
  phase: SourcePhase;
  lastStartedAtUtc: string | null;
  lastCompletedAtUtc: string | null;
  lastSuccessfulCheckUtc: string | null;
  nextCheckAtUtc: string | null;
  lastError: SourceFailure | null;
  lastNewItems: number;
  unresolvedLinks: number;
}

export interface CheckStartResult {
  startedSources: GiveawaySource[];
  alreadyRunningSources: GiveawaySource[];
}

export interface GiveawayItem {
  source: GiveawaySource;
  sourceId: string;
  title: string;
  url: string;
  kind: "giveaway" | "raffle";
  store: "steam" | "epicGames" | "gog" | "unknown";
  details: string | null;
}

export interface SourceFailure {
  source: GiveawaySource;
  message: string;
  reason:
    | { code: "unknown" | "timeout" | "connection" | "invalidFeed" | "zerosConnectionLimit" }
    | { code: "http"; status: number };
}

export interface MonitoringOutcome {
  checkPerformed: boolean;
  isInitialSynchronization: boolean;
  baselineDeferred: boolean;
  newItems: GiveawayItem[];
  failures: SourceFailure[];
}

export interface NotificationRecord {
  store: GiveawayItem["store"];
  id: string;
  trackingKey: string | null;
  source: string;
  campaignId?: string | null;
  originSource?: GiveawaySource | null;
  originName?: string | null;
  detectedSources?: GiveawaySource[];
  kind: string;
  title: string;
  url: string;
  receivedAtUtc: string;
  readAtUtc: string | null;
  deliveryState: string;
  deliveryAttempts: number;
  lastDeliveryError: string | null;
}

export interface NotificationHistoryPage {
  items: NotificationRecord[];
  nextCursor: string | null;
}

export function getNotificationHistoryPage(cursor: string | null = null): Promise<NotificationHistoryPage> {
  return invoke<NotificationHistoryPage>("get_notification_history_page", { cursor });
}

export function getAppState(): Promise<AppState> {
  return invoke<AppState>("get_app_state");
}

export function getStorageSnapshot(): Promise<StorageSnapshot> {
  return invoke<StorageSnapshot>("get_storage_snapshot");
}

export function updateSettings(settings: AppSettings): Promise<void> {
  return invoke<void>("update_settings", { settings });
}

export function clearGiveawayData(): Promise<void> {
  return invoke<void>("clear_giveaway_data");
}

export function checkNow(): Promise<CheckStartResult> {
  return invoke<CheckStartResult>("check_now");
}

export function getNotificationHistory(): Promise<NotificationRecord[]> {
  return invoke<NotificationRecord[]>("get_notification_history");
}

export function openNotification(notificationId: string): Promise<void> {
  return invoke<void>("open_notification", { notificationId });
}

export function markAllNotificationsRead(): Promise<number> {
  return invoke<number>("mark_all_notifications_read");
}

export function markNotificationRead(notificationId: string): Promise<void> {
  return invoke<void>("mark_notification_read", { notificationId });
}

export function sendTestNotification(): Promise<void> {
  return invoke<void>("send_test_notification");
}
