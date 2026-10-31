import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  checkNow,
  checkForUpdates,
  clearGiveawayData,
  getAppState,
  getNotificationHistory,
  getStorageSnapshot,
  getUpdateState,
  installUpdate,
  markAllNotificationsRead,
  notificationErrorKey,
  openNotification,
  sendTestNotification,
  updateSettings,
} from "./api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

describe("desktop API", () => {
  it("checks update metadata without granting permission to install", async () => {
    vi.mocked(invoke).mockResolvedValue({ mode: "notConfigured", phase: "idle", candidate: null });
    await getUpdateState();
    await checkForUpdates();
    expect(invoke).toHaveBeenNthCalledWith(1, "get_update_state");
    expect(invoke).toHaveBeenNthCalledWith(2, "check_for_updates");
    expect(invoke).not.toHaveBeenCalledWith("install_update", expect.anything());
  });

  it("sends explicit confirmation bound to a candidate token, never a URL or key", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    await installUpdate("candidate-token");
    expect(invoke).toHaveBeenCalledWith("install_update", { token: "candidate-token", confirmed: true });
  });
  it("maps native macOS notification errors to localized messages", () => {
    expect(notificationErrorKey("MACOS_NOTIFICATION_APP_BUNDLE_REQUIRED")).toBe("testNotificationMacosBundleRequired");
    expect(notificationErrorKey(new Error("MACOS_NOTIFICATION_PERMISSION_DENIED"))).toBe("testNotificationMacosPermissionDenied");
    expect(notificationErrorKey("MACOS_NOTIFICATION_PERMISSION_PENDING")).toBe("testNotificationMacosPermissionPending");
    expect(notificationErrorKey("Unexpected native error")).toBe("testNotificationError");
    expect(notificationErrorKey(null)).toBe("testNotificationError");
  });
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("requests the application state from Rust", async () => {
    const expected = {
      appName: "Giveaway Scout",
      version: "0.1.0",
      platform: "linux",
      architecture: "x86_64",
      isDevelopment: true,
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(getAppState()).resolves.toEqual(expected);
    expect(invoke).toHaveBeenCalledWith("get_app_state");
  });

  it("requests the SQLite storage snapshot from Rust", async () => {
    const expected = {
      settings: {
        checkIntervalMinutes: 15,
        httpTimeoutSeconds: 30,
        httpRetryCount: 2,
        giveeClubEnabled: true,
        gamesForFarmEnabled: true,
        zerosEnabled: false,
        steamNotificationsEnabled: true,
        epicGamesNotificationsEnabled: true,
        notificationSoundEnabled: true,
        startAtLogin: false,
        startInTray: true,
        keepRunningInTray: false,
        languageCode: null,
        verboseLogging: false,
        initialSyncMode: null,
      },
      counts: {
        trackedGiveaways: 0,
        notificationHistory: 0,
        unreadNotifications: 0,
      },
      dataSizeBytes: 4096,
      databaseRecovery: null,
      initialSyncChoiceRequired: true,
      monitor: {
        isChecking: false,
        lastStartedAtUtc: null,
        lastCompletedAtUtc: null,
        lastSuccessfulCheckUtc: null,
        lastError: null,
        lastNewItems: 0,
        lastFailureCount: 0,
        sourceFailures: [],
      },
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(getStorageSnapshot()).resolves.toEqual(expected);
    expect(invoke).toHaveBeenCalledWith("get_storage_snapshot");
  });

  it("requests an immediate giveaway check from Rust", async () => {
    const expected = {
      checkPerformed: true,
      isInitialSynchronization: false,
      baselineDeferred: false,
      newItems: [],
      failures: [],
    };
    vi.mocked(invoke).mockResolvedValue(expected);

    await expect(checkNow()).resolves.toEqual(expected);
    expect(invoke).toHaveBeenCalledWith("check_now");
  });

  it("requests and opens notification history through Rust", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]).mockResolvedValueOnce(undefined);

    await expect(getNotificationHistory()).resolves.toEqual([]);
    await expect(openNotification("notification-1")).resolves.toBeUndefined();

    expect(invoke).toHaveBeenNthCalledWith(1, "get_notification_history");
    expect(invoke).toHaveBeenNthCalledWith(2, "open_notification", {
      notificationId: "notification-1",
    });
  });

  it("marks all notifications as read through Rust", async () => {
    vi.mocked(invoke).mockResolvedValue(3);

    await expect(markAllNotificationsRead()).resolves.toBe(3);
    expect(invoke).toHaveBeenCalledWith("mark_all_notifications_read");
  });

  it("requests a test system notification through Rust", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(sendTestNotification()).resolves.toBeUndefined();

    expect(invoke).toHaveBeenCalledWith("send_test_notification");
  });

  it("updates settings and clears saved giveaway data through Rust", async () => {
    const settings = {
      checkIntervalMinutes: 30,
      httpTimeoutSeconds: 45,
      httpRetryCount: 3,
      giveeClubEnabled: true,
      gamesForFarmEnabled: false,
      zerosEnabled: true,
      steamNotificationsEnabled: false,
      epicGamesNotificationsEnabled: true,
      notificationSoundEnabled: true,
      startAtLogin: false,
      startInTray: true,
      keepRunningInTray: false,
      languageCode: "en" as const,
      verboseLogging: false,
      initialSyncMode: "silent" as const,
    };
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(updateSettings(settings)).resolves.toBeUndefined();
    await expect(clearGiveawayData()).resolves.toBeUndefined();

    expect(invoke).toHaveBeenNthCalledWith(1, "update_settings", { settings });
    expect(invoke).toHaveBeenNthCalledWith(2, "clear_giveaway_data");
  });
});
