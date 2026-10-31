import { useCallback, useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Bell,
  Check,
  Clock3,
  HardDrive,
  History,
  LoaderCircle,
  RadioTower,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  type AppSettings,
  type InitialSyncMode,
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
import "./App.css";
import { sourceFailureText, sourceName } from "./source-errors";
import { settingsHaveChanges } from "./settings-draft";

const sources = [
  {
    name: "Givee Club",
    settingKey: "giveeClubEnabled",
  },
  {
    name: "Games for Farm",
    settingKey: "gamesForFarmEnabled",
  },
  {
    name: "Zeros",
    descriptionKey: "zerosDescription",
    settingKey: "zerosEnabled",
  },
] as const;

function App() {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings | null>(null);
  const [settingsBaseline, setSettingsBaseline] = useState<AppSettings | null>(null);
  const [confirmSettingsClose, setConfirmSettingsClose] = useState(false);
  const [intervalValue, setIntervalValue] = useState("15");
  const [httpTimeoutValue, setHttpTimeoutValue] = useState("30");
  const [httpRetryValue, setHttpRetryValue] = useState("2");
  const [firstLaunchMode, setFirstLaunchMode] = useState<InitialSyncMode>("silent");
  const appState = useQuery({
    queryKey: ["app-state"],
    queryFn: getAppState,
  });
  const storageState = useQuery({
    queryKey: ["storage-snapshot"],
    queryFn: getStorageSnapshot,
    refetchInterval: 3_000,
  });
  const notificationHistory = useQuery({
    queryKey: ["notification-history"],
    queryFn: getNotificationHistory,
    refetchInterval: 3_000,
  });
  const updates = useQuery({ queryKey: ["updates"], queryFn: getUpdateState, refetchInterval: 1_000 });
  const checkUpdatesMutation = useMutation({ mutationFn: checkForUpdates, onSettled: () => updates.refetch() });
  const installUpdateMutation = useMutation({ mutationFn: installUpdate, onSettled: () => updates.refetch() });
  const updatePhase = updates.data?.phase;
  const updateInstalling = updatePhase === "downloading" || updatePhase === "installing" || installUpdateMutation.isPending;
  const updateBusy = updateInstalling || updatePhase === "checking" || checkUpdatesMutation.isPending;
  const storageReady = storageState.isSuccess && !storageState.data.initialSyncChoiceRequired;
  const checkMutation = useMutation({
    mutationFn: checkNow,
    onSuccess: async () => {
      await storageState.refetch();
    },
  });
  const openNotificationMutation = useMutation({
    mutationFn: openNotification,
    onSuccess: async () => {
      await Promise.all([notificationHistory.refetch(), storageState.refetch()]);
    },
  });
  const markAllReadMutation = useMutation({
    mutationFn: markAllNotificationsRead,
    onSuccess: async () => {
      await Promise.all([notificationHistory.refetch(), storageState.refetch()]);
    },
  });
  const testNotificationMutation = useMutation({
    mutationFn: sendTestNotification,
  });
  const sourceSettingsMutation = useMutation({
    mutationFn: updateSettings,
    onSuccess: async () => {
      await storageState.refetch();
    },
  });
  const updateSettingsMutation = useMutation({
    mutationFn: updateSettings,
    onSuccess: async (_, settings) => {
      await i18n.changeLanguage(settings.languageCode ?? detectedLanguage());
      document.documentElement.lang = i18n.language;
      await storageState.refetch();
      setSettingsOpen(false);
    },
  });
  const firstLaunchMutation = useMutation({
    mutationFn: (initialSyncMode: InitialSyncMode) => {
      const settings = storageState.data?.settings;
      if (!settings) return Promise.reject(new Error("Storage is not ready"));
      return updateSettings({ ...settings, initialSyncMode });
    },
    onSuccess: async () => {
      await storageState.refetch();
    },
  });
  const clearDataMutation = useMutation({
    mutationFn: clearGiveawayData,
    onSuccess: async () => {
      await Promise.all([notificationHistory.refetch(), storageState.refetch()]);
    },
  });
  const hasStatusError =
    appState.isError || storageState.isError ||
    Boolean(storageState.data?.monitor.lastError);
  const sourceFailures = storageState.data?.monitor.sourceFailures ?? [];
  const isChecking = Boolean(storageState.data?.monitor.isChecking);
  const clearBlockedByCheck = isChecking || checkMutation.isPending || updateInstalling;
  const isDevelopment = Boolean(appState.data?.isDevelopment);

  const intervalMinutes = Number(intervalValue);
  const intervalIsValid =
    Number.isInteger(intervalMinutes) && intervalMinutes >= 5 && intervalMinutes <= 1_440;
  const httpTimeoutSeconds = Number(httpTimeoutValue);
  const httpTimeoutIsValid =
    Number.isInteger(httpTimeoutSeconds) &&
    httpTimeoutSeconds >= 5 &&
    httpTimeoutSeconds <= 120;
  const httpRetryCount = Number(httpRetryValue);
  const httpRetryIsValid =
    httpRetryValue.trim() !== "" &&
    Number.isInteger(httpRetryCount) && httpRetryCount >= 0 && httpRetryCount <= 5;
  const settingsAreValid = intervalIsValid && httpTimeoutIsValid && httpRetryIsValid;
  const settingsDirty = settingsHaveChanges(settingsBaseline, settingsDraft, {
    interval: intervalValue, timeout: httpTimeoutValue, retries: httpRetryValue,
  });
  const requestCloseSettings = useCallback(() => {
    if (updateSettingsMutation.isPending) return;
    if (settingsDirty) setConfirmSettingsClose(true);
    else setSettingsOpen(false);
  }, [settingsDirty, updateSettingsMutation.isPending]);

  useEffect(() => {
    const language = storageState.data?.settings.languageCode;
    if (language && !i18n.language.startsWith(language)) {
      void i18n.changeLanguage(language);
      document.documentElement.lang = language;
    }
  }, [i18n, storageState.data?.settings.languageCode]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen("history-changed", () => {
      void Promise.all([
        queryClient.invalidateQueries({ queryKey: ["notification-history"] }),
        queryClient.invalidateQueries({ queryKey: ["storage-snapshot"] }),
      ]);
    }).then((stopListening) => {
      unlisten = stopListening;
    });
    return () => unlisten?.();
  }, [queryClient]);

  useEffect(() => {
    if (!settingsOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (confirmSettingsClose) setConfirmSettingsClose(false);
      else requestCloseSettings();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [settingsOpen, confirmSettingsClose, requestCloseSettings]);

  const openSettings = () => {
    const settings = storageState.data?.settings;
    if (!settings) return;
    const baseline = {
      ...settings,
      languageCode: settings.languageCode ?? detectedLanguage(),
    };
    setSettingsDraft(baseline);
    setSettingsBaseline(baseline);
    setConfirmSettingsClose(false);
    setIntervalValue(String(settings.checkIntervalMinutes));
    setHttpTimeoutValue(String(settings.httpTimeoutSeconds));
    setHttpRetryValue(String(settings.httpRetryCount));
    updateSettingsMutation.reset();
    clearDataMutation.reset();
    setSettingsOpen(true);
  };

  const saveSettings = () => {
    if (!settingsDraft || !settingsAreValid) return;
    updateSettingsMutation.mutate({
      ...settingsDraft,
      checkIntervalMinutes: intervalMinutes,
      httpTimeoutSeconds,
      httpRetryCount,
    });
  };

  const confirmClearData = () => {
    if (clearBlockedByCheck || clearDataMutation.isPending) return;
    if (window.confirm(t("clearDataConfirm"))) clearDataMutation.mutate();
  };

  const toggleSource = (settingKey: (typeof sources)[number]["settingKey"]) => {
    const settings = storageState.data?.settings;
    if (!settings) return;
    sourceSettingsMutation.mutate({
      ...settings,
      [settingKey]: !settings[settingKey],
    });
  };

  return (
    <main className="app-shell" data-app="giveaway-scout">
      <section className="status-panel" aria-live="polite">
        <div className={`connection-dot ${hasStatusError ? "error" : ""}`} />
        <div>
          <strong>
            {appState.isPending || storageState.isPending
              ? t("statusStarting")
              : appState.isError || storageState.isError
                ? t("statusUnavailable")
                : storageState.data?.initialSyncChoiceRequired
                  ? t("statusWaitingInitialMode")
                  : isChecking
                    ? t("statusChecking")
                    : t("statusActive")}
          </strong>
          {storageState.data?.monitor.lastSuccessfulCheckUtc && (
            <span>
              {t("lastSuccessfulCheck")}: {formatCheckTime(
                storageState.data.monitor.lastSuccessfulCheckUtc,
                i18n.language,
              )}
            </span>
          )}
          {sourceFailures.map((failure, index) => (
            <span className="status-error" key={`${failure.source}-${index}`}>
              {sourceFailureText(failure, t)}
            </span>
          ))}
          {storageState.data?.monitor.lastError && sourceFailures.length === 0 && (
            <span className="status-error">{t("lastCheckError")}: {t("checkFailed")}</span>
          )}
        </div>
        <div className="status-actions">
          {(appState.isPending || storageState.isPending || isChecking) && (
            <LoaderCircle className="spinner" size={19} aria-hidden="true" />
          )}
          <button
            className="header-button"
            disabled={!storageReady}
            onClick={openSettings}
            type="button"
          >
            <Settings size={17} aria-hidden="true" />
            {t("settings")}
          </button>
        </div>
      </section>

      {storageState.data?.databaseRecovery && (
        <section className="recovery-panel" role="alert">
          <AlertTriangle size={21} aria-hidden="true" />
          <div>
            <strong>{t("databaseRecoveryTitle")}</strong>
            <p>{t("databaseRecoveryDescription")}</p>
            <span>{t("databaseRecoveryBackup")}</span>
            <code>{storageState.data.databaseRecovery.backupPath}</code>
            <small>{t("databaseRecoveryAction")}</small>
          </div>
        </section>
      )}

      <div className="content-grid">
        <section className="card sources-card">
          <div className="section-heading">
            <div>
              <h2>{t("sources")}</h2>
            </div>
            <RadioTower size={20} aria-hidden="true" />
          </div>

          <div className="source-list">
            {sources.map((source) => {
              const enabled = storageState.data?.settings[source.settingKey] ?? false;
              return (
                <button
                  role="checkbox"
                  aria-checked={enabled}
                  className="source-row"
                  disabled={!storageReady || sourceSettingsMutation.isPending}
                  key={source.name}
                  onClick={() => toggleSource(source.settingKey)}
                  title={source.settingKey === "zerosEnabled" ? t("zerosWarning") : undefined}
                  type="button"
                >
                  <span
                    className={`source-check ${enabled ? "" : "disabled"}`}
                    aria-hidden="true"
                  >
                    {enabled && <Check size={15} />}
                  </span>
                  <div>
                    <strong>{source.name}</strong>
                    {"descriptionKey" in source && <span>{t(source.descriptionKey)}</span>}
                  </div>
                </button>
              );
            })}
          </div>
          {sourceSettingsMutation.isError && (
            <p className="monitor-message error">{t("sourceUpdateError")}</p>
          )}

          <div className="control-row">
            <div>
              <Clock3 size={17} aria-hidden="true" />
              <span>{t("interval")}</span>
            </div>
            <strong>
              {t("minutes", {
                count: storageState.data?.settings.checkIntervalMinutes ?? 15,
              })}
            </strong>
          </div>

          <div className="source-actions">
            <button
              className="primary-button"
              disabled={
                !storageReady ||
                checkMutation.isPending ||
                clearDataMutation.isPending ||
                updateInstalling ||
                storageState.data?.monitor.isChecking
              }
              onClick={() => checkMutation.mutate()}
              title={!storageReady ? t("checkUnavailable") : undefined}
              type="button"
            >
              {checkMutation.isPending ? t("checkingNow") : t("checkNow")}
            </button>
            <button
              className="secondary-button test-notification-button"
              disabled={!storageReady || testNotificationMutation.isPending}
              onClick={() => testNotificationMutation.mutate()}
              type="button"
            >
              <Bell size={16} aria-hidden="true" />
              {testNotificationMutation.isPending
                ? t("sendingTestNotification")
                : t("testNotification")}
            </button>
          </div>
          {checkMutation.isError && (
            <p className="monitor-message error">{t("checkFailed")}</p>
          )}
          {updates.data?.candidate && (
            <button className="secondary-button update-banner" type="button" onClick={openSettings}>
              {t("updateAvailable", { version: updates.data.candidate.version })}
            </button>
          )}
          {testNotificationMutation.isSuccess && (
            <p className="monitor-message">{t("testNotificationSent")}</p>
          )}
          {testNotificationMutation.isError && (
            <p className="monitor-message error">{t(notificationErrorKey(testNotificationMutation.error))}</p>
          )}
          {checkMutation.data && (
            <p className="monitor-message">
              {t(
                checkMutation.data.baselineDeferred
                  ? "checkBaselineDeferred"
                  : checkMutation.data.isInitialSynchronization
                    ? "checkBaselineReady"
                    : checkMutation.data.failures.length > 0
                      ? "checkPartial"
                      : checkMutation.data.newItems.length > 0
                        ? "checkNewItems"
                        : "checkNoNewItems",
                {
                  count: checkMutation.data.newItems.length,
                  failures: checkMutation.data.failures.length,
                },
              )}
            </p>
          )}
        </section>

        <section className="card history-card">
          <div className="section-heading">
            <div>
              <h2>{t("history")}</h2>
            </div>
            <History size={20} aria-hidden="true" />
          </div>

          <div className="history-toolbar">
            <span>
              {t("unreadCount", {
                count: storageState.data?.counts.unreadNotifications ?? 0,
              })}
            </span>
            <button
              className="secondary-button"
              disabled={
                markAllReadMutation.isPending ||
                (storageState.data?.counts.unreadNotifications ?? 0) === 0
              }
              onClick={() => markAllReadMutation.mutate()}
              type="button"
            >
              {t("markAllRead")}
            </button>
          </div>

          {notificationHistory.isPending ? (
            <div className="empty-state">
              <LoaderCircle className="spinner" size={25} aria-hidden="true" />
              <p>{t("historyLoading")}</p>
            </div>
          ) : notificationHistory.isError ? (
            <div className="empty-state error-state">
              <History size={28} aria-hidden="true" />
              <p>{t("historyUnavailable")}</p>
            </div>
          ) : notificationHistory.data.length === 0 ? (
            <div className="empty-state">
              <History size={28} aria-hidden="true" />
              <p>{t("emptyHistory")}</p>
            </div>
          ) : (
            <div className="history-list">
              {notificationHistory.data.map((notification) => (
                <button
                  className={`history-item ${notification.readAtUtc ? "" : "unread"}`}
                  disabled={
                    openNotificationMutation.isPending &&
                    openNotificationMutation.variables === notification.id
                  }
                  key={notification.id}
                  onClick={(event) => {
                    event.currentTarget.blur();
                    openNotificationMutation.mutate(notification.id);
                  }}
                  title={t("openNotification")}
                  type="button"
                >
                  <span className="history-item-main">
                    <strong>{notification.title}</strong>
                    <small>
                      {notification.store === "steam" ? "Steam"
                        : notification.store === "epicGames" ? "Epic Games"
                        : notification.store === "gog" ? "GOG"
                        : t("unknownPlatform")} · {sourceName(notification.source)} · {t(`kind.${notification.kind}`)}
                    </small>
                  </span>
                  <time dateTime={notification.receivedAtUtc}>
                    {formatCheckTime(notification.receivedAtUtc, i18n.language)}
                  </time>
                  {!notification.readAtUtc && (
                    <span className="unread-dot" aria-label={t("unread")} />
                  )}
                </button>
              ))}
            </div>
          )}
          {openNotificationMutation.isError && (
            <p className="monitor-message error">{t("openNotificationError")}</p>
          )}
        </section>
      </div>

      {settingsOpen && settingsDraft && (
        <div
          className="modal-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) requestCloseSettings();
          }}
          role="presentation"
        >
          <section
            aria-labelledby="settings-title"
            aria-modal="true"
            aria-busy={updateSettingsMutation.isPending}
            className="settings-dialog settings-dialog-scrollable"
            role="dialog"
          >
            <div className="dialog-header">
              <div>
                <h2 id="settings-title">{t("settings")}</h2>
              </div>
              <button
                aria-label={t("closeSettings")}
                className="icon-button"
                disabled={updateSettingsMutation.isPending}
                onClick={requestCloseSettings}
                type="button"
              >
                <X size={18} aria-hidden="true" />
              </button>
            </div>

            <div className="settings-content" inert={updateSettingsMutation.isPending}>
              <fieldset className="settings-group">
                <legend>{t("sources")}</legend>
                <p className="setting-hint">{t("zerosWarning")}</p>
                <label className="check-setting">
                  <input
                    checked={settingsDraft.giveeClubEnabled}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        giveeClubEnabled: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>Givee Club</span>
                </label>
                <label className="check-setting">
                  <input
                    checked={settingsDraft.gamesForFarmEnabled}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        gamesForFarmEnabled: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>Games for Farm</span>
                </label>
                <label className="check-setting">
                  <input
                    checked={settingsDraft.zerosEnabled}
                    onChange={(event) => setSettingsDraft({ ...settingsDraft, zerosEnabled: event.target.checked })}
                    type="checkbox"
                  />
                  <span>Zeros (HTTP)</span>
                </label>
              </fieldset>

              <div className="settings-group">
                <label className="input-setting" htmlFor="check-interval">
                  <span>{t("intervalMinutes")}</span>
                  <input
                    aria-invalid={!intervalIsValid}
                    id="check-interval"
                    inputMode="numeric"
                    max={1_440}
                    min={5}
                    onChange={(event) => setIntervalValue(event.target.value)}
                    type="number"
                    value={intervalValue}
                  />
                </label>
                <small className={intervalIsValid ? "setting-hint" : "setting-error"}>
                  {t(intervalIsValid ? "intervalHint" : "intervalError")}
                </small>
              </div>

              <fieldset className="settings-group">
                <legend>{t("network")}</legend>
                <label className="input-setting" htmlFor="http-timeout">
                  <span>{t("httpTimeoutSeconds")}</span>
                  <input
                    aria-invalid={!httpTimeoutIsValid}
                    id="http-timeout"
                    inputMode="numeric"
                    max={120}
                    min={5}
                    onChange={(event) => setHttpTimeoutValue(event.target.value)}
                    type="number"
                    value={httpTimeoutValue}
                  />
                </label>
                <small className={httpTimeoutIsValid ? "setting-hint" : "setting-error"}>
                  {t(httpTimeoutIsValid ? "httpTimeoutHint" : "httpTimeoutError")}
                </small>
                <label className="input-setting" htmlFor="http-retries">
                  <span>{t("httpRetryCount")}</span>
                  <input
                    aria-invalid={!httpRetryIsValid}
                    id="http-retries"
                    inputMode="numeric"
                    max={5}
                    min={0}
                    onChange={(event) => setHttpRetryValue(event.target.value)}
                    type="number"
                    value={httpRetryValue}
                  />
                </label>
                <small className={httpRetryIsValid ? "setting-hint" : "setting-error"}>
                  {t(httpRetryIsValid ? "httpRetryHint" : "httpRetryError")}
                </small>
              </fieldset>

              <fieldset className="settings-group">
                <legend>{t("language")}</legend>
                <div className="segmented-control">
                  {(["ru", "en"] as const).map((language) => (
                    <button
                      className={settingsDraft.languageCode === language ? "active" : ""}
                      key={language}
                      onClick={() =>
                        setSettingsDraft({ ...settingsDraft, languageCode: language })
                      }
                      type="button"
                    >
                      {t(`languageName.${language}`)}
                    </button>
                  ))}
                </div>
                <small className="setting-hint">{t("languageHint")}</small>
              </fieldset>

              <fieldset className="settings-group">
                <legend>{t("systemBehavior")}</legend>
                <label
                  className={`check-setting check-setting-details ${isDevelopment ? "disabled" : ""}`}
                >
                  <input
                    id="start-at-login"
                    checked={settingsDraft.startAtLogin}
                    disabled={isDevelopment}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        startAtLogin: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("startAtLogin")}</strong>
                    <small>
                      {t(
                        isDevelopment
                          ? "startAtLoginDevelopmentDescription"
                          : "startAtLoginDescription",
                      )}
                    </small>
                  </span>
                </label>
                <label className={`check-setting check-setting-details ${!settingsDraft.startAtLogin ? "disabled" : ""}`}>
                  <input
                    id="start-in-tray"
                    checked={settingsDraft.startInTray}
                    disabled={!settingsDraft.startAtLogin}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      startInTray: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("startInTray")}</strong>
                    <small>{t("startInTrayDescription")}</small>
                  </span>
                </label>
                <label className="check-setting check-setting-details">
                  <input
                    id="keep-running-in-tray"
                    checked={settingsDraft.keepRunningInTray}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        keepRunningInTray: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("keepRunningInTray")}</strong>
                    <small>{t("keepRunningInTrayDescription")}</small>
                  </span>
                </label>
              </fieldset>

              <fieldset className="settings-group">
                <legend>{t("notifications")}</legend>
                <label className="check-setting check-setting-details">
                  <input
                    checked={settingsDraft.steamNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      steamNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("steamNotifications")}</strong>
                    <small>{t("storeNotificationsDescription", { store: "Steam" })}</small>
                  </span>
                </label>
                <label className="check-setting check-setting-details">
                  <input
                    checked={settingsDraft.epicGamesNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      epicGamesNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("epicGamesNotifications")}</strong>
                    <small>{t("storeNotificationsDescription", { store: "Epic Games" })}</small>
                  </span>
                </label>
                <label className="check-setting check-setting-details">
                  <input
                    checked={settingsDraft.gogNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      gogNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("gogNotifications")}</strong>
                    <small>{t("storeNotificationsDescription", { store: "GOG" })}</small>
                  </span>
                </label>
                <small className="setting-hint">{t("storeFiltersHint")}</small>
                <label className="check-setting check-setting-details">
                  <input
                    checked={settingsDraft.notificationSoundEnabled}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        notificationSoundEnabled: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("notificationSound")}</strong>
                    <small>{t("notificationSoundDescription")}</small>
                  </span>
                </label>
              </fieldset>

              <fieldset className="settings-group">
                <legend>{t("diagnostics")}</legend>
                <label className="check-setting check-setting-details">
                  <input
                    checked={settingsDraft.verboseLogging}
                    onChange={(event) =>
                      setSettingsDraft({
                        ...settingsDraft,
                        verboseLogging: event.target.checked,
                      })
                    }
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("verboseLogging")}</strong>
                    <small>{t("verboseLoggingDescription")}</small>
                  </span>
                </label>
              </fieldset>

              <fieldset className="settings-group app-updates">
                <legend>{t("appUpdates")}</legend>
                <p>{t("installedVersion", { version: updates.data?.currentVersion ?? appState.data?.version ?? "—" })}</p>
                {updates.data?.mode !== "enabled" && <p className="setting-hint">{t(
                  updates.data?.mode === "development" ? "updatesDevelopment" :
                  updates.data?.mode === "manual" ? "updatesManual" :
                  updates.data?.mode === "packageManager" ? "updatesPackageManager" :
                  updates.data?.mode === "invalidConfiguration" ? "updatesInvalidConfiguration" : "updatesNotConfigured")}</p>}
                {updates.data?.mode === "enabled" && <>
                  <button className="secondary-button" disabled={updateBusy} type="button" onClick={() => checkUpdatesMutation.mutate()}>
                    {t(updatePhase === "checking" ? "checkingUpdates" : "checkUpdates")}
                  </button>
                  {updates.data.lastCheckedAtUtc && !updates.data.candidate && updatePhase === "idle" && <p role="status">{t("updatesUpToDate")}</p>}
                  {updates.data.candidate && <div className="update-candidate">
                    <strong>{t("updateAvailable", { version: updates.data.candidate.version })}</strong>
                    {updates.data.candidate.notes && <pre className="update-notes">{updates.data.candidate.notes}</pre>}
                    <button className="primary-button" type="button" disabled={updateBusy || clearBlockedByCheck || clearDataMutation.isPending}
                      onClick={() => {
                        const candidate = updates.data?.candidate;
                        if (candidate && window.confirm(t("confirmInstallUpdate", { version: candidate.version }))) {
                          installUpdateMutation.mutate(candidate.token);
                        }
                      }}>{t("installUpdate")}</button>
                  </div>}
                  {updatePhase === "downloading" && <p role="status">{t("downloadingUpdate", {
                    progress: `${formatBytes(updates.data.downloadedBytes, i18n.language)}${updates.data.totalBytes === null ? "" : ` / ${formatBytes(updates.data.totalBytes, i18n.language)}`}`,
                  })}</p>}
                  {updatePhase === "installing" && <p role="status">{t("installingUpdate")}</p>}
                  {(updates.data.lastError || checkUpdatesMutation.isError || installUpdateMutation.isError) && <p className="setting-error" role="alert">{t("updateFailed")}</p>}
                </>}
              </fieldset>

              <div className="settings-group data-settings">
                <div className="data-summary">
                  <HardDrive size={19} aria-hidden="true" />
                  <div>
                    <strong>{t("savedData")}</strong>
                    <span>
                      {formatBytes(storageState.data?.dataSizeBytes ?? 0, i18n.language)} · {t(
                        "savedGiveawaysCount",
                        { count: storageState.data?.counts.trackedGiveaways ?? 0 },
                      )}
                    </span>
                  </div>
                </div>
                <p>{t("clearDataDescription")}</p>
                <button
                  className="danger-button"
                  disabled={clearDataMutation.isPending || clearBlockedByCheck}
                  aria-describedby={clearBlockedByCheck ? "clear-data-busy-hint" : undefined}
                  onClick={confirmClearData}
                  type="button"
                >
                  <Trash2 size={16} aria-hidden="true" />
                  {clearDataMutation.isPending ? t("clearingData") : t("clearData")}
                </button>
                {clearBlockedByCheck && (
                  <small className="setting-hint" id="clear-data-busy-hint">
                    {t(updateInstalling ? "updateDataBusy" : "clearDataBusy")}
                  </small>
                )}
                {clearDataMutation.isSuccess && (
                  <small className="success-message">{t("clearDataSuccess")}</small>
                )}
                {clearDataMutation.isError && (
                  <small className="setting-error">
                    {t(String(clearDataMutation.error) === "data operation is already running"
                      ? "clearDataBusy"
                      : "clearDataError")}
                  </small>
                )}
              </div>
            </div>

            <footer className="settings-footer">
              <p className="settings-draft-status" role="status">
                {t(settingsDirty ? "settingsUnsaved" : "settingsUnchanged")}
              </p>
              {!settingsAreValid && (
                <button className="settings-validation-link" type="button" onClick={() => {
                  document.getElementById(!intervalIsValid ? "check-interval" :
                    !httpTimeoutIsValid ? "http-timeout" : "http-retries")?.focus();
                }}>
                  {t(!intervalIsValid ? "intervalError" : !httpTimeoutIsValid ? "httpTimeoutError" : "httpRetryError")}
                </button>
              )}
              {updateSettingsMutation.isError && (
                <p className="dialog-error" role="alert">{t("saveSettingsError")}</p>
              )}
              {confirmSettingsClose && (
                <div className="settings-close-prompt" role="alert">
                  <p>{t("settingsClosePrompt")}</p>
                  <div className="settings-close-actions">
                    <button className="secondary-button" type="button"
                      disabled={updateSettingsMutation.isPending}
                      onClick={() => setConfirmSettingsClose(false)}>{t("settingsContinueEditing")}</button>
                    <button className="secondary-button" type="button"
                      disabled={updateSettingsMutation.isPending}
                      onClick={() => setSettingsOpen(false)}>{t("settingsDiscard")}</button>
                  </div>
                </div>
              )}
              <div className="dialog-actions">
                <button
                  className="secondary-button dialog-button"
                  disabled={updateSettingsMutation.isPending}
                  onClick={requestCloseSettings}
                  type="button"
                >
                  {t("cancel")}
                </button>
                <button
                  className="primary-button dialog-button"
                  disabled={!settingsAreValid || updateSettingsMutation.isPending}
                  onClick={saveSettings}
                  type="button"
                >
                  {updateSettingsMutation.isPending ? t("saving") : t("save")}
                </button>
              </div>
            </footer>
          </section>
        </div>
      )}

      {storageState.data?.initialSyncChoiceRequired && (
        <div className="modal-backdrop first-launch-backdrop" role="presentation">
          <section
            aria-labelledby="first-launch-title"
            aria-modal="true"
            className="settings-dialog first-launch-dialog"
            role="dialog"
          >
            <h2 id="first-launch-title">{t("firstLaunchTitle")}</h2>
            <p>{t("firstLaunchDescription")}</p>
            <div className="mode-options first-launch-options">
              {(["silent", "notifyCurrent"] as const).map((mode) => (
                <label className="mode-option" key={mode}>
                  <input
                    checked={firstLaunchMode === mode}
                    name="first-launch-mode"
                    onChange={() => setFirstLaunchMode(mode)}
                    type="radio"
                  />
                  <span>
                    <strong>{t(`initialSyncModeName.${mode}`)}</strong>
                    <small>{t(`initialSyncModeDescription.${mode}`)}</small>
                  </span>
                </label>
              ))}
            </div>
            <button
              className="primary-button"
              disabled={firstLaunchMutation.isPending}
              onClick={() => firstLaunchMutation.mutate(firstLaunchMode)}
              type="button"
            >
              {firstLaunchMutation.isPending ? t("saving") : t("startMonitoring")}
            </button>
            {firstLaunchMutation.isError && (
              <p className="dialog-error">{t("initialSyncModeSaveError")}</p>
            )}
          </section>
        </div>
      )}
    </main>
  );
}

export default App;

function formatCheckTime(value: string, language: string): string {
  return new Date(value).toLocaleString(language.startsWith("ru") ? "ru-RU" : "en-US", {
    dateStyle: "short",
    timeStyle: "medium",
  });
}

function detectedLanguage(): "ru" | "en" {
  return navigator.language.toLowerCase().startsWith("ru") ? "ru" : "en";
}

function formatBytes(bytes: number, language: string): string {
  const locale = language.startsWith("ru") ? "ru-RU" : "en-US";
  if (bytes < 1_024) return `${bytes} ${language.startsWith("ru") ? "Б" : "B"}`;
  const units = language.startsWith("ru")
    ? ["КБ", "МБ", "ГБ"]
    : ["KB", "MB", "GB"];
  let value = bytes / 1_024;
  let unitIndex = 0;
  while (value >= 1_024 && unitIndex < units.length - 1) {
    value /= 1_024;
    unitIndex += 1;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} ${units[unitIndex]}`;
}
