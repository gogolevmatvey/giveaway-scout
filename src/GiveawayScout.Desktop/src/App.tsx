import { useCallback, useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Bell,
  Check,
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
  type SourceCheckIntervals,
  checkNow,
  checkForUpdates,
  clearGiveawayData,
  chooseLogDirectory,
  dismissUpdate,
  getAppState,
  getLoggingState,
  getNotificationHistoryPage,
  getStorageSnapshot,
  getUpdateState,
  installUpdate,
  markAllNotificationsRead,
  markNotificationRead,
  notificationErrorKey,
  openNotification,
  openLogDirectory,
  sendTestNotification,
  updateSettings,
} from "./api";
import "./App.css";
import { sourceFailureText } from "./source-errors";
import { Select } from "./components/Select";
import { SettingCard } from "./components/SettingCard";
import { VirtualHistoryList } from "./components/VirtualHistoryList";
import { notificationDetectorNames, notificationOriginName, sourceCatalog as sources } from "./history";
import { intervalDraft, isIntegerInRange, settingsHaveChanges } from "./settings-draft";

const settingsSections = ["sources", "notifications", "application", "network", "diagnostics", "updatesData"] as const;
type SettingsSection = (typeof settingsSections)[number];

function App() {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [historySubscribed, setHistorySubscribed] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("sources");
  const [technicalDetailsOpen, setTechnicalDetailsOpen] = useState(false);
  const [settingToFocus, setSettingToFocus] = useState<string | null>(null);
  const [settingsDraft, setSettingsDraft] = useState<AppSettings | null>(null);
  const [settingsBaseline, setSettingsBaseline] = useState<AppSettings | null>(null);
  const [confirmSettingsClose, setConfirmSettingsClose] = useState(false);
  const [intervalValues, setIntervalValues] = useState(intervalDraft);
  const [httpTimeoutValue, setHttpTimeoutValue] = useState("30");
  const [httpRetryValue, setHttpRetryValue] = useState("2");
  const [logSizeValue, setLogSizeValue] = useState("2");
  const [logFilesValue, setLogFilesValue] = useState("1");
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
  const notificationHistory = useInfiniteQuery({
    queryKey: ["notification-history-pages"],
    queryFn: ({ pageParam }) => getNotificationHistoryPage(pageParam),
    initialPageParam: null as string | null,
    enabled: historySubscribed,
    getNextPageParam: (lastPage, _pages, _pageParam, pageParams) =>
      lastPage.nextCursor && !pageParams.includes(lastPage.nextCursor) ? lastPage.nextCursor : undefined,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  });
  const updates = useQuery({ queryKey: ["updates"], queryFn: getUpdateState, refetchInterval: 1_000 });
  const logging = useQuery({
    queryKey: ["logging"], queryFn: getLoggingState,
    refetchInterval: 3_000,
  });
  const chooseLogDirectoryMutation = useMutation({
    mutationFn: chooseLogDirectory,
    onSuccess: directory => {
      if (directory !== null) setSettingsDraft(draft => draft ? {...draft, logDirectory: directory} : draft);
    },
  });
  const openLogDirectoryMutation = useMutation({ mutationFn: openLogDirectory });
  const checkUpdatesMutation = useMutation({ mutationFn: checkForUpdates, onSettled: () => updates.refetch() });
  const installUpdateMutation = useMutation({ mutationFn: installUpdate, onSettled: () => updates.refetch() });
  const dismissUpdateMutation = useMutation({
    mutationFn: dismissUpdate,
    onSuccess: snapshot => queryClient.setQueryData(["updates"], snapshot),
    onSettled: () => updates.refetch(),
  });
  const updatePhase = updates.data?.phase;
  const updateInstalling = updatePhase === "downloading" || updatePhase === "installing" || installUpdateMutation.isPending;
  const updateBusy = updateInstalling || updatePhase === "checking" || checkUpdatesMutation.isPending || dismissUpdateMutation.isPending;
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
  const markReadMutation = useMutation({
    mutationFn: markNotificationRead,
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
      await Promise.all([storageState.refetch(), logging.refetch()]);
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
      await Promise.all([queryClient.resetQueries({ queryKey: ["notification-history-pages"] }), storageState.refetch()]);
    },
  });
  const sourceStates = storageState.data?.monitor.sourceStates ?? [];
  const sourceFailures = sourceStates.length
    ? sourceStates.filter(state => state.phase !== "disabled" && state.lastError).flatMap(state => state.lastError ? [state.lastError] : [])
    : storageState.data?.monitor.sourceFailures ?? [];
  const hasStatusError = appState.isError || storageState.isError || sourceFailures.length > 0 ||
    (!sourceStates.length && Boolean(storageState.data?.monitor.lastError));
  const history = useMemo(() => {
    const seen = new Set<string>();
    return (notificationHistory.data?.pages ?? []).flatMap(page => page.items).filter(record => {
      if (seen.has(record.id)) return false;
      seen.add(record.id); return true;
    });
  }, [notificationHistory.data]);
  const loadMoreHistory = useCallback(() => notificationHistory.fetchNextPage({ cancelRefetch: false }), [notificationHistory.fetchNextPage]);
  const retryHistory = useCallback(() => notificationHistory.isFetchNextPageError ? loadMoreHistory() : notificationHistory.refetch(),
    [notificationHistory.isFetchNextPageError, loadMoreHistory, notificationHistory.refetch]);
  const isChecking = Boolean(storageState.data?.monitor.isChecking);
  const clearDataBlocked = checkMutation.isPending || updateInstalling;
  const isDevelopment = Boolean(appState.data?.isDevelopment);

  const invalidIntervalSource = sources.find(({ source }) => !isIntegerInRange(intervalValues[source], 5, 1_440));
  const intervalsAreValid = !invalidIntervalSource;
  const httpTimeoutSeconds = Number(httpTimeoutValue);
  const httpTimeoutIsValid =
    Number.isInteger(httpTimeoutSeconds) &&
    httpTimeoutSeconds >= 5 &&
    httpTimeoutSeconds <= 120;
  const httpRetryCount = Number(httpRetryValue);
  const httpRetryIsValid =
    httpRetryValue.trim() !== "" &&
    Number.isInteger(httpRetryCount) && httpRetryCount >= 0 && httpRetryCount <= 5;
  const logSizeIsValid = isIntegerInRange(logSizeValue, 1, 100);
  const logFilesIsValid = isIntegerInRange(logFilesValue, 1, 10);
  const settingsAreValid = intervalsAreValid && httpTimeoutIsValid && httpRetryIsValid && logSizeIsValid && logFilesIsValid;
  const settingsDirty = settingsHaveChanges(settingsBaseline, settingsDraft, {
    intervals: intervalValues, timeout: httpTimeoutValue, retries: httpRetryValue,
    logSize: logSizeValue, logFiles: logFilesValue,
  });
  const firstInvalidSetting = invalidIntervalSource ? [`check-interval-${invalidIntervalSource.source}`, "intervalError"] :
    !httpTimeoutIsValid ? ["http-timeout", "httpTimeoutError"] :
    !httpRetryIsValid ? ["http-retries", "httpRetryError"] :
    !logSizeIsValid ? ["log-file-size", "logFileSizeError"] : ["log-file-count", "logFileCountError"];
  const settingsLocked = updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending;
  const selectSettingsSection = (section: SettingsSection) => {
    if (settingsLocked) return;
    setSettingsSection(section);
  };
  const onSettingsNavigationKey = (event: ReactKeyboardEvent<HTMLButtonElement>, section: SettingsSection) => {
    const index = settingsSections.indexOf(section);
    const next = event.key === "Home" ? 0 : event.key === "End" ? settingsSections.length - 1 :
      event.key === "ArrowDown" ? (index + 1) % settingsSections.length :
      event.key === "ArrowUp" ? (index + settingsSections.length - 1) % settingsSections.length : null;
    if (next === null || settingsLocked || event.ctrlKey || event.metaKey || event.altKey) return;
    event.preventDefault();
    setSettingsSection(settingsSections[next]);
    document.getElementById(`settings-tab-${settingsSections[next]}`)?.focus();
  };
  const focusInvalidSetting = () => {
    if (settingsLocked) return;
    const id = firstInvalidSetting[0];
    const section: SettingsSection = id.startsWith("check-interval-") ? "sources" : id.startsWith("http-") ? "network" : "diagnostics";
    setSettingsSection(section);
    if (section === "diagnostics") setTechnicalDetailsOpen(true);
    setSettingToFocus(id);
  };
  const requestCloseSettings = useCallback(() => {
    if (updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending) return;
    if (settingsDirty) setConfirmSettingsClose(true);
    else setSettingsOpen(false);
  }, [settingsDirty, updateSettingsMutation.isPending, chooseLogDirectoryMutation.isPending]);

  useEffect(() => {
    const language = storageState.data?.settings.languageCode;
    if (language && !i18n.language.startsWith(language)) {
      void i18n.changeLanguage(language);
      document.documentElement.lang = language;
    }
  }, [i18n, storageState.data?.settings.languageCode]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    let historyTimer: ReturnType<typeof setTimeout> | null = null;
    const refresh = (event: string) => {
      void queryClient.invalidateQueries({ queryKey: ["storage-snapshot"] });
      if (event !== "history-changed") return;
      if (historyTimer !== null) clearTimeout(historyTimer);
      historyTimer = setTimeout(() => {
        historyTimer = null;
        void (async () => {
          const queryKey = ["notification-history-pages"];
          // A first request has no cached data, so invalidation alone would reuse
          // its promise and could commit a page read before this change event.
          if (queryClient.getQueryState(queryKey)?.data === undefined) {
            await queryClient.cancelQueries({ queryKey, exact: true });
          }
          if (!disposed) await queryClient.invalidateQueries({ queryKey, exact: true });
        })();
      }, 80);
    };
    for (const event of ["history-changed", "monitor-changed"]) {
      void listen(event, () => refresh(event)).then(stopListening => {
        if (disposed) stopListening();
        else {
          unlisteners.push(stopListening);
          // Start the first page read after its change listener is already active.
          if (event === "history-changed") setHistorySubscribed(true);
        }
      });
    }
    return () => { disposed = true; if (historyTimer !== null) clearTimeout(historyTimer); unlisteners.forEach(stop => stop()); };
  }, [queryClient]);

  useEffect(() => {
    if (!settingsOpen) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      if (confirmSettingsClose) setConfirmSettingsClose(false);
      else requestCloseSettings();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [settingsOpen, confirmSettingsClose, requestCloseSettings]);

  useEffect(() => {
    if (!settingsOpen) return;
    const content = document.querySelector<HTMLElement>(".settings-dialog-scrollable .settings-content");
    if (content) content.scrollTop = 0;
  }, [settingsOpen, settingsSection]);

  useEffect(() => {
    if (!settingsOpen || !settingToFocus) return;
    const frame = requestAnimationFrame(() => {
      const input = document.getElementById(settingToFocus);
      input?.focus({ preventScroll: true });
      input?.scrollIntoView({ block: "nearest" });
      setSettingToFocus(null);
    });
    return () => cancelAnimationFrame(frame);
  }, [settingsOpen, settingsSection, settingToFocus, technicalDetailsOpen]);

  useEffect(() => {
    if (settingsOpen && (logging.isError || logging.data?.lastError || chooseLogDirectoryMutation.isPending ||
      chooseLogDirectoryMutation.isError || openLogDirectoryMutation.isError)) setTechnicalDetailsOpen(true);
  }, [settingsOpen, logging.isError, logging.data?.lastError, chooseLogDirectoryMutation.isPending,
    chooseLogDirectoryMutation.isError, openLogDirectoryMutation.isError]);

  const openSettings = () => {
    const settings = storageState.data?.settings;
    if (!settings) return;
    if (!testNotificationMutation.isPending) testNotificationMutation.reset();
    const baseline = {
      ...settings,
      languageCode: settings.languageCode ?? detectedLanguage(),
    };
    setSettingsDraft(baseline);
    setSettingsBaseline(baseline);
    setSettingsSection("sources");
    setConfirmSettingsClose(false);
    setTechnicalDetailsOpen(false);
    setSettingToFocus(null);
    setIntervalValues(intervalDraft(settings.sourceCheckIntervals));
    setHttpTimeoutValue(String(settings.httpTimeoutSeconds));
    setHttpRetryValue(String(settings.httpRetryCount));
    setLogSizeValue(String(settings.logMaxFileSizeMb));
    setLogFilesValue(String(settings.logFileCount));
    updateSettingsMutation.reset();
    chooseLogDirectoryMutation.reset();
    openLogDirectoryMutation.reset();
    clearDataMutation.reset();
    void logging.refetch();
    setSettingsOpen(true);
  };

  const saveSettings = () => {
    if (!settingsDraft || !settingsAreValid || !settingsDirty || settingsLocked) return;
    updateSettingsMutation.mutate({
      ...settingsDraft,
      sourceCheckIntervals: Object.fromEntries(sources.map(({ source }) => [source, Number(intervalValues[source])])) as SourceCheckIntervals,
      httpTimeoutSeconds,
      httpRetryCount,
      logMaxFileSizeMb: Number(logSizeValue),
      logFileCount: Number(logFilesValue),
    });
  };

  const confirmClearData = () => {
    if (clearDataBlocked || clearDataMutation.isPending) return;
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

  const confirmInstallUpdate = () => {
    const candidate = updates.data?.candidate;
    if (candidate && window.confirm(t("confirmInstallUpdate", { version: candidate.version }))) {
      installUpdateMutation.mutate(candidate.token);
    }
  };

  const dismissUpdateBanner = () => {
    const candidate = updates.data?.candidate;
    if (candidate && !updateInstalling && !dismissUpdateMutation.isPending) dismissUpdateMutation.mutate(candidate.token);
  };

  return (
    <main className="app-shell" data-app="giveaway-scout">
      {updates.data?.mode === "enabled" && updates.data.candidate && !updates.data.bannerDismissed && (
        <section className="update-banner" role="status" aria-busy={dismissUpdateMutation.isPending}>
          <Bell size={20} aria-hidden="true" />
          <div className="update-banner-message">
            <strong>{t("updateAvailable", { version: updates.data.candidate.version })}</strong>
            {updatePhase === "downloading" && <span>{t("downloadingUpdate", {
              progress: `${formatBytes(updates.data.downloadedBytes, i18n.language)}${updates.data.totalBytes === null ? "" : ` / ${formatBytes(updates.data.totalBytes, i18n.language)}`}`,
            })}</span>}
            {updatePhase === "installing" && <span>{t("installingUpdate")}</span>}
            {(updates.data.lastError || installUpdateMutation.isError) && (
              <span className="setting-error" role="alert">{t("updateFailed")}</span>
            )}
            {dismissUpdateMutation.isPending && <span>{t("dismissingUpdateNotification")}</span>}
            {dismissUpdateMutation.isError && dismissUpdateMutation.variables === updates.data.candidate.token && (
              <span className="setting-error" role="alert">{t("dismissUpdateNotificationFailed")}</span>
            )}
          </div>
          <button
            className="primary-button update-banner-action"
            type="button"
            disabled={updateBusy || isChecking || clearDataBlocked || clearDataMutation.isPending}
            onClick={confirmInstallUpdate}
          >
            {updateInstalling && <LoaderCircle className="spinner" size={16} aria-hidden="true" />}
            {t(updateInstalling ? "updating" : "updateNow")}
          </button>
          <button className="update-banner-dismiss" type="button"
            aria-label={t("dismissUpdateNotification")} title={t("dismissUpdateNotification")}
            disabled={updateInstalling || dismissUpdateMutation.isPending}
            onClick={dismissUpdateBanner}>
            <X size={18} aria-hidden="true" />
          </button>
        </section>
      )}

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
                    : t(sourceFailures.length ? "statusPartial" : "statusActive")}
          </strong>
          {storageState.data?.monitor.lastSuccessfulCheckUtc && (
            <span>
              {t("lastSuccessfulCheck")}: {formatCheckTime(
                storageState.data.monitor.lastSuccessfulCheckUtc,
                i18n.language,
              )}
            </span>
          )}
          {sourceFailures.length > 0 && <span className="status-error">{t("sourceProblemCount", { count: sourceFailures.length })}</span>}
          {!sourceStates.length && storageState.data?.monitor.lastError && sourceFailures.length === 0 && (
            <span className="status-error">{t("lastCheckError")}: {t("checkFailed")}</span>
          )}
          {logging.data?.restartRequired && <span className="logging-status" role="status">{t("logRestartRequired")}</span>}
          {logging.data?.lastError && <span className="logging-status setting-error" role="alert">{t(
            logging.data.usingFallback ? "logFallbackStatus" : logging.data.activeFilePath ? "logWriteErrorStatus" : "logDisabledStatus",
          )}</span>}
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
              const state = sourceStates.find(state => state.source === source.source);
              const phase = enabled ? (state?.phase ?? "waiting") : "disabled";
              return (
                <div className="source-entry" key={source.source} data-source={source.source}>
                  <button
                    role="checkbox"
                    aria-checked={enabled}
                    aria-label={source.name}
                    className="source-row"
                    disabled={!storageReady || sourceSettingsMutation.isPending}
                    onClick={() => toggleSource(source.settingKey)}
                    title={source.settingKey === "zerosEnabled" ? t("zerosWarning") : undefined}
                    type="button"
                  >
                    <span className={`source-check ${enabled ? "" : "disabled"}`} aria-hidden="true">
                      {enabled && <Check size={15} />}
                    </span>
                    <div>
                      <strong>{source.name}</strong>
                      <span>{t(source.descriptionKey)}</span>
                    </div>
                    <span className={`source-phase phase-${phase}`}>{t(`sourcePhase.${phase}`)}</span>
                  </button>
                  {(state?.lastError || Boolean(state?.unresolvedLinks)) && (
                    <div className="source-health">
                      <details className="source-details">
                        <summary>{t("sourceDetails")}</summary>
                        {state?.lastError && <p className="setting-error">{sourceFailureText(state.lastError, t)}</p>}
                        {Boolean(state?.unresolvedLinks) && <p>{t("sourceUnresolvedLinks", { count: state?.unresolvedLinks })}</p>}
                      </details>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          {sourceSettingsMutation.isError && (
            <p className="monitor-message error">{t("sourceUpdateError")}</p>
          )}

          <div className="source-actions">
            <button
              className="primary-button"
              disabled={
                !storageReady ||
                checkMutation.isPending ||
                clearDataMutation.isPending ||
                updateInstalling
              }
              onClick={() => checkMutation.mutate()}
              title={!storageReady ? t("checkUnavailable") : undefined}
              type="button"
            >
              {checkMutation.isPending ? t("checkingNow") : t("checkNow")}
            </button>
          </div>
          {checkMutation.isError && (
            <p className="monitor-message error">{t("checkFailed")}</p>
          )}
          {checkMutation.data && (
            <p className="monitor-message" role="status">
              {t("checkStarted", { count: checkMutation.data.startedSources.length, running: checkMutation.data.alreadyRunningSources.length })}
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
            <div className="empty-state"><LoaderCircle className="spinner" size={25} aria-hidden="true" /><p>{t("historyLoading")}</p></div>
          ) : notificationHistory.isError && history.length === 0 ? (
            <div className="empty-state error-state"><History size={28} aria-hidden="true" /><p>{t("historyUnavailable")}</p>
              <button className="secondary-button history-retry" type="button" onClick={() => void notificationHistory.refetch()}>{t("historyRetry")}</button>
            </div>
          ) : history.length === 0 ? (
            <div className="empty-state"><History size={28} aria-hidden="true" /><p>{t("emptyHistory")}</p></div>
          ) : (
            <VirtualHistoryList records={history} totalCount={storageState.data?.counts.notificationHistory ?? history.length}
              label={t("history")} layoutKey={i18n.language} hasMore={Boolean(notificationHistory.hasNextPage)}
              fetching={notificationHistory.isFetching} loadingMore={notificationHistory.isFetchingNextPage}
              error={notificationHistory.isFetchNextPageError || notificationHistory.isRefetchError}
              loadingText={t("historyLoadingMore")} errorText={t("historyPageUnavailable")} retryText={t("historyRetry")}
              onLoadMore={loadMoreHistory} onRetry={retryHistory}
              renderRow={(notification, rowProps) => {
                const detectors = notificationDetectorNames(notification);
                return (
                  <article {...rowProps} className={`history-item ${notification.readAtUtc ? "" : "unread"}`} key={notification.id} data-notification-id={notification.id}>
                    <div className="history-item-layout">
                    <button
                      className="history-open"
                      disabled={openNotificationMutation.isPending && openNotificationMutation.variables === notification.id}
                      onClick={event => { event.currentTarget.blur(); openNotificationMutation.mutate(notification.id); }}
                      title={t("openNotification")}
                      type="button"
                    >
                      <span className="history-item-main">
                        <span className="history-item-title">
                          {!notification.readAtUtc && <span className="unread-dot" aria-label={t("unread")} />}
                          <strong>{notification.title}</strong>
                        </span>
                        <small>
                          {notification.store === "steam" ? "Steam" : notification.store === "epicGames" ? "Epic Games" : notification.store === "gog" ? "GOG" : t("unknownPlatform")} · {notificationOriginName(notification)}{detectors ? ` · ${t("foundVia", { sources: detectors })}` : ""} · {t(`kind.${notification.kind}`)}
                        </small>
                      </span>
                    </button>
                    <div className="history-item-actions">
                      <time dateTime={notification.receivedAtUtc}>{formatCheckTime(notification.receivedAtUtc, i18n.language)}</time>
                      {!notification.readAtUtc && <button className="mark-read-button secondary-button" type="button" disabled={markReadMutation.isPending && markReadMutation.variables === notification.id} onClick={() => markReadMutation.mutate(notification.id)}>{t("markRead")}</button>}
                    </div>
                    </div>
                  </article>
                );
              }} />
          )}
          {markReadMutation.isError && <p className="monitor-message error">{t("markReadError")}</p>}
          {openNotificationMutation.isError && (
            <p className="monitor-message error">{t("openNotificationError")}</p>
          )}
        </section>
      </div>

      {settingsOpen && settingsDraft && (
        <div
          className="modal-backdrop settings-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) requestCloseSettings();
          }}
          role="presentation"
        >
          <section
            aria-labelledby="settings-title"
            aria-modal="true"
            aria-busy={updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
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
                disabled={updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
                onClick={requestCloseSettings}
                type="button"
              >
                <X size={18} aria-hidden="true" />
              </button>
            </div>

            <div className="settings-body">
              <nav className="settings-navigation" role="tablist" aria-label={t("settingsSectionsLabel")} aria-orientation="vertical">
                {settingsSections.map(section => (
                  <button key={section} type="button" role="tab" id={`settings-tab-${section}`}
                    data-settings-section={section} aria-selected={settingsSection === section}
                    aria-controls={`settings-panel-${section}`} tabIndex={settingsSection === section ? 0 : -1}
                    disabled={settingsLocked} onClick={() => selectSettingsSection(section)}
                    onKeyDown={event => onSettingsNavigationKey(event, section)}>
                    {t(`settingsSections.${section}`)}
                  </button>
                ))}
              </nav>
            <div className="settings-content" inert={settingsLocked}>
              <section className="settings-panel" role="tabpanel" id="settings-panel-sources"
                aria-labelledby="settings-tab-sources" hidden={settingsSection !== "sources"}>
              <fieldset className="settings-group source-interval-settings">
                <legend>{t("sourcesAndIntervals")}</legend>
                <p className="setting-hint" id="source-interval-range">{t("intervalHint")}</p>
                {sources.map(({ source, name, settingKey }) => {
                  const valid = isIntegerInRange(intervalValues[source], 5, 1_440);
                  return (
                    <div className="source-setting setting-card" key={source} data-source-setting={source}>
                      <div className="source-setting-row">
                        <div className="setting-card-copy">
                        <label className="check-setting" htmlFor={`source-enabled-${source}`}>
                          <input id={`source-enabled-${source}`} type="checkbox" checked={settingsDraft[settingKey]}
                            onChange={event => setSettingsDraft({ ...settingsDraft, [settingKey]: event.target.checked })} />
                          <span>{name}{source === "zeros" ? " (HTTP)" : ""}</span>
                        </label>
                        {source === "zeros" && <div className="source-setting-note">
                          <p className="setting-hint">{t("zerosConnectionSummary")}</p>
                          <details className="settings-explanation"><summary>{t("connectionDetails")}</summary><p>{t("zerosWarning")}</p></details>
                        </div>}
                        <small id={`check-interval-${source}-hint`} className="setting-error" hidden={valid}>
                          {t("intervalError")}
                        </small>
                        </div>
                        <div className="source-setting-interval">
                          <input aria-label={t("sourceIntervalLabel", { source: name })}
                          aria-describedby={`source-interval-range${valid ? "" : ` check-interval-${source}-hint`}`}
                          aria-invalid={!valid}
                          id={`check-interval-${source}`}
                          inputMode="numeric"
                          max={1_440}
                          min={5}
                          onChange={(event) => setIntervalValues(values => ({ ...values, [source]: event.target.value }))}
                          type="number"
                          value={intervalValues[source]}
                        />
                          <span aria-hidden="true">{t("minutesUnit")}</span>
                        </div>
                      </div>
                    </div>
                  );
                })}
              </fieldset>
              </section>

              <section className="settings-panel" role="tabpanel" id="settings-panel-network"
                aria-labelledby="settings-tab-network" hidden={settingsSection !== "network"}>
              <fieldset className="settings-group">
                <legend>{t("network")}</legend>
                <SettingCard controlId="http-timeout" title={t("httpTimeoutSeconds")}
                  description={<small id="http-timeout-hint" className={httpTimeoutIsValid ? "setting-hint" : "setting-error"}>
                    {t(httpTimeoutIsValid ? "httpTimeoutHint" : "httpTimeoutError")}
                  </small>}>
                  <input
                    aria-invalid={!httpTimeoutIsValid}
                    aria-describedby="http-timeout-hint"
                    id="http-timeout"
                    inputMode="numeric"
                    max={120}
                    min={5}
                    onChange={(event) => setHttpTimeoutValue(event.target.value)}
                    type="number"
                    value={httpTimeoutValue}
                  />
                </SettingCard>
                <SettingCard controlId="http-retries" title={t("httpRetryCount")}
                  description={<small id="http-retries-hint" className={httpRetryIsValid ? "setting-hint" : "setting-error"}>
                    {t(httpRetryIsValid ? "httpRetryHint" : "httpRetryError")}
                  </small>}>
                  <input
                    aria-invalid={!httpRetryIsValid}
                    aria-describedby="http-retries-hint"
                    id="http-retries"
                    inputMode="numeric"
                    max={5}
                    min={0}
                    onChange={(event) => setHttpRetryValue(event.target.value)}
                    type="number"
                    value={httpRetryValue}
                  />
                </SettingCard>
              </fieldset>
              </section>

              <section className="settings-panel" role="tabpanel" id="settings-panel-application"
                aria-labelledby="settings-tab-application" hidden={settingsSection !== "application"}>
              <fieldset className="settings-group setting-card-block">
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
                  className={`setting-card check-setting check-setting-details ${isDevelopment ? "disabled" : ""}`}
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
                <SettingCard controlId="autostart-target" title={t("autostartTarget")}
                  description={<small id="autostart-target-hint" className="setting-hint">{t("autostartTargetHint")}</small>}>
                  <Select<"gui" | "cli">
                    id="autostart-target"
                    label={t("autostartTarget")}
                    describedBy="autostart-target-hint"
                    disabled={isDevelopment}
                    value={settingsDraft.autostartTarget}
                    onValueChange={(autostartTarget) => setSettingsDraft({
                      ...settingsDraft,
                      autostartTarget,
                    })}
                    options={[
                      { value: "gui", label: t("autostartTargetGui") },
                      { value: "cli", label: t("autostartTargetCli") },
                    ]}
                  />
                </SettingCard>
                <label className={`setting-card check-setting check-setting-details ${!settingsDraft.startAtLogin || settingsDraft.autostartTarget === "cli" ? "disabled" : ""}`}>
                  <input
                    id="start-in-tray"
                    checked={settingsDraft.startInTray}
                    disabled={!settingsDraft.startAtLogin || settingsDraft.autostartTarget === "cli"}
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
                <label className="setting-card check-setting check-setting-details">
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
              </section>

              <section className="settings-panel" role="tabpanel" id="settings-panel-notifications"
                aria-labelledby="settings-tab-notifications" hidden={settingsSection !== "notifications"}>
              <fieldset className="settings-group">
                <legend>{t("notifications")}</legend>
                <div className="setting-card-block">
                <p className="setting-hint">{t("notificationsHint")}</p>
                <div className="store-notification-options">
                <label className="check-setting">
                  <input
                    id="notifications-steam"
                    checked={settingsDraft.steamNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      steamNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("steamNotifications")}</strong>
                  </span>
                </label>
                <label className="check-setting">
                  <input
                    id="notifications-epicGames"
                    checked={settingsDraft.epicGamesNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      epicGamesNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("epicGamesNotifications")}</strong>
                  </span>
                </label>
                <label className="check-setting">
                  <input
                    id="notifications-gog"
                    checked={settingsDraft.gogNotificationsEnabled}
                    onChange={(event) => setSettingsDraft({
                      ...settingsDraft,
                      gogNotificationsEnabled: event.target.checked,
                    })}
                    type="checkbox"
                  />
                  <span>
                    <strong>{t("gogNotifications")}</strong>
                  </span>
                </label>
                </div>
                <details className="settings-explanation"><summary>{t("notificationFilterDetails")}</summary><p>{t("storeFiltersHint")}</p></details>
                </div>
                <label className="setting-card check-setting check-setting-details">
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
                <div className="notification-test setting-card-block">
                  <button
                    className="secondary-button test-notification-button"
                    disabled={!storageReady || testNotificationMutation.isPending || updateSettingsMutation.isPending}
                    onClick={() => testNotificationMutation.mutate()}
                    aria-describedby="notification-test-hint"
                    type="button"
                  >
                    <Bell size={16} aria-hidden="true" />
                    {testNotificationMutation.isPending ? t("sendingTestNotification") : t("testNotification")}
                  </button>
                  <small className="setting-hint" id="notification-test-hint">{t("testNotificationSettingsHint")}</small>
                  {testNotificationMutation.isSuccess && <p className="monitor-message" role="status">{t("testNotificationSent")}</p>}
                  {testNotificationMutation.isError && <p className="monitor-message error" role="alert">{t(notificationErrorKey(testNotificationMutation.error))}</p>}
                </div>
              </fieldset>
              </section>

              <section className="settings-panel" role="tabpanel" id="settings-panel-diagnostics"
                aria-labelledby="settings-tab-diagnostics" hidden={settingsSection !== "diagnostics"}>
              <fieldset className="settings-group logging-settings">
                <legend>{t("diagnostics")}</legend>
                <label className="setting-card check-setting check-setting-details">
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
                <SettingCard controlId="log-file-size" title={t("logFileSize")}
                  description={<small id="log-file-size-hint" className={logSizeIsValid ? "setting-hint" : "setting-error"}>
                    {t(logSizeIsValid ? "logFileSizeHint" : "logFileSizeError")}
                  </small>}>
                  <input id="log-file-size" type="number" inputMode="numeric" min={1} max={100} step={1}
                    aria-invalid={!logSizeIsValid} aria-describedby="log-file-size-hint"
                    value={logSizeValue} onChange={event => setLogSizeValue(event.target.value)} />
                </SettingCard>
                <SettingCard controlId="log-file-count" title={t("logFileCount")}
                  description={<small id="log-file-count-hint" className={logFilesIsValid ? "setting-hint" : "setting-error"}>
                    {t(logFilesIsValid ? "logFileCountHint" : "logFileCountError")}
                  </small>}>
                  <input id="log-file-count" type="number" inputMode="numeric" min={1} max={10} step={1}
                    aria-invalid={!logFilesIsValid} aria-describedby="log-file-count-hint"
                    value={logFilesValue} onChange={event => setLogFilesValue(event.target.value)} />
                </SettingCard>
                <div className="log-path-setting setting-card-block">
                  <strong id="log-directory-label">{t("logDirectory")}</strong>
                  <output className="log-path" aria-labelledby="log-directory-label" data-testid="log-directory-draft">
                    {settingsDraft.logDirectory ?? t("logDirectoryDefault")}
                  </output>
                  <div className="log-folder-actions">
                    <button className="secondary-button" type="button" data-testid="choose-log-directory"
                      disabled={chooseLogDirectoryMutation.isPending || updateSettingsMutation.isPending}
                      onClick={() => chooseLogDirectoryMutation.mutate()}>{t("browse")}</button>
                    <button className="secondary-button" type="button" data-testid="default-log-directory"
                      disabled={settingsDraft.logDirectory === null || chooseLogDirectoryMutation.isPending || updateSettingsMutation.isPending}
                      onClick={() => setSettingsDraft({...settingsDraft, logDirectory: null})}>{t("useDefault")}</button>
                  </div>
                  <small className="setting-hint">{t("logDirectoryHint")}</small>
                  {chooseLogDirectoryMutation.isError && <small className="setting-error" role="alert">{t("logChooseDirectoryError")}</small>}
                </div>
                <details className="settings-technical-details" open={technicalDetailsOpen}
                  onToggle={event => setTechnicalDetailsOpen(event.currentTarget.open)}>
                <summary>{t("technicalDetails")}</summary>
                <div className="log-path-setting">
                  <strong id="active-log-label">{t("logActiveFile")}</strong>
                  <output className="log-path" aria-labelledby="active-log-label" data-testid="log-active-file">
                    {logging.isPending ? t("logStateLoading") : logging.data?.activeFilePath ?? t("logNoActiveFile")}
                  </output>
                  <button className="secondary-button" type="button" data-testid="open-log-directory"
                    disabled={!logging.data?.effectiveDirectory || openLogDirectoryMutation.isPending}
                    onClick={() => openLogDirectoryMutation.mutate()}>{t("openFolder")}</button>
                  {openLogDirectoryMutation.isError && <small className="setting-error" role="alert">{t("logOpenDirectoryError")}</small>}
                  {logging.isError && <small className="setting-error" role="alert">{t("logStateError")}</small>}
                </div>
                </details>
                <small className={logging.data?.restartRequired ? "logging-restart-required" : "setting-hint"}
                  role={logging.data?.restartRequired ? "status" : undefined}>
                  {t(logging.data?.restartRequired ? "logRestartRequired" : "logRestartHint")}
                </small>
                {logging.data?.lastError && <div className="log-error" role="alert">
                  <p>{t(logging.data.usingFallback ? "logFallback" : logging.data.activeFilePath ? "logWriteError" : "logDisabled")}</p>
                  <span>{logging.data.lastError}</span>
                </div>}
              </fieldset>
              </section>

              <section className="settings-panel" role="tabpanel" id="settings-panel-updatesData"
                aria-labelledby="settings-tab-updatesData" hidden={settingsSection !== "updatesData"}>
              <label className="setting-card check-setting check-setting-details">
                <input id="automatic-update-checks" type="checkbox"
                  checked={settingsDraft.automaticUpdateChecks}
                  onChange={event => setSettingsDraft({...settingsDraft, automaticUpdateChecks: event.target.checked})} />
                <span>
                  <strong>{t("automaticUpdateChecks")}</strong>
                  <small>{t("automaticUpdateChecksDescription")}</small>
                </span>
              </label>
              <fieldset className="settings-group setting-card-block app-updates">
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
                    <button className="primary-button" type="button" disabled={updateBusy || isChecking || clearDataBlocked || clearDataMutation.isPending}
                      onClick={confirmInstallUpdate}>{t("installUpdate")}</button>
                  </div>}
                  {updatePhase === "downloading" && <p role="status">{t("downloadingUpdate", {
                    progress: `${formatBytes(updates.data.downloadedBytes, i18n.language)}${updates.data.totalBytes === null ? "" : ` / ${formatBytes(updates.data.totalBytes, i18n.language)}`}`,
                  })}</p>}
                  {updatePhase === "installing" && <p role="status">{t("installingUpdate")}</p>}
                  {(updates.data.lastError || checkUpdatesMutation.isError || installUpdateMutation.isError) && <p className="setting-error" role="alert">{t("updateFailed")}</p>}
                </>}
              </fieldset>

              <div className="settings-group setting-card-block data-settings">
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
                  disabled={clearDataMutation.isPending || clearDataBlocked}
                  aria-describedby={clearDataBlocked ? "clear-data-busy-hint" : undefined}
                  onClick={confirmClearData}
                  type="button"
                >
                  <Trash2 size={16} aria-hidden="true" />
                  {clearDataMutation.isPending ? t("clearingData") : t("clearData")}
                </button>
                {clearDataBlocked && (
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
              </section>
            </div>
            </div>

            <footer className="settings-footer">
              <div className="settings-footer-main">
              <div className="settings-footer-status">
              <p className="settings-draft-status" role="status">
                {t(settingsDirty ? "settingsUnsaved" : "settingsUnchanged")}
              </p>
              {!settingsAreValid && (
                <button className="settings-validation-link" type="button" disabled={settingsLocked} onClick={focusInvalidSetting}>
                  {t(firstInvalidSetting[1])}
                </button>
              )}
              </div>
              <div className="dialog-actions">
                <button
                  className="secondary-button dialog-button"
                  disabled={updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
                  onClick={requestCloseSettings}
                  type="button"
                >
                  {t("cancel")}
                </button>
                <button
                  className="primary-button dialog-button"
                  disabled={!settingsDirty || !settingsAreValid || updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
                  onClick={saveSettings}
                  type="button"
                >
                  {updateSettingsMutation.isPending ? t("saving") : t("save")}
                </button>
              </div>
              </div>
              {updateSettingsMutation.isError && (
                <p className="dialog-error" role="alert">{t("saveSettingsError")}</p>
              )}
              {confirmSettingsClose && (
                <div className="settings-close-prompt" role="alert">
                  <p>{t("settingsClosePrompt")}</p>
                  <div className="settings-close-actions">
                    <button className="secondary-button" type="button"
                      disabled={updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
                      onClick={() => setConfirmSettingsClose(false)}>{t("settingsContinueEditing")}</button>
                    <button className="secondary-button" type="button"
                      disabled={updateSettingsMutation.isPending || chooseLogDirectoryMutation.isPending}
                      onClick={() => setSettingsOpen(false)}>{t("settingsDiscard")}</button>
                  </div>
                </div>
              )}
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
    timeStyle: "short",
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
