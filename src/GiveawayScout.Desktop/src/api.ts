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
  downloadedBytes: number;
  totalBytes: number | null;
  lastCheckedAtUtc: string | null;
  lastError: string | null;
}

export function getUpdateState(): Promise<UpdateSnapshot> { return invoke("get_update_state"); }
export function checkForUpdates(): Promise<UpdateSnapshot> { return invoke("check_for_updates"); }
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
  httpTimeoutSeconds: number;
  httpRetryCount: number;
  giveeClubEnabled: boolean;
  gamesForFarmEnabled: boolean;
  zerosEnabled: boolean;
  steamNotificationsEnabled: boolean;
  epicGamesNotificationsEnabled: boolean;
  gogNotificationsEnabled: boolean;
  notificationSoundEnabled: boolean;
  startAtLogin: boolean;
  startInTray: boolean;
  keepRunningInTray: boolean;
  languageCode: "ru" | "en" | null;
  verboseLogging: boolean;
  initialSyncMode: InitialSyncMode | null;
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
}

export interface GiveawayItem {
  source: "giveeClub" | "gamesForFarm" | "zeros";
  sourceId: string;
  title: string;
  url: string;
  kind: "giveaway" | "raffle";
  store: "steam" | "epicGames" | "gog" | "unknown";
  details: string | null;
}

export interface SourceFailure {
  source: "giveeClub" | "gamesForFarm" | "zeros";
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
  kind: string;
  title: string;
  url: string;
  receivedAtUtc: string;
  readAtUtc: string | null;
  deliveryState: string;
  deliveryAttempts: number;
  lastDeliveryError: string | null;
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

export function checkNow(): Promise<MonitoringOutcome> {
  return invoke<MonitoringOutcome>("check_now");
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

export function sendTestNotification(): Promise<void> {
  return invoke<void>("send_test_notification");
}
