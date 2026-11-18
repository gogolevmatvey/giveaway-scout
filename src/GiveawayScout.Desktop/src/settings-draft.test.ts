import { expect, it } from "vitest";
import type { AppSettings } from "./api";
import { intervalDraft, isIntegerInRange, settingsHaveChanges } from "./settings-draft";

const baseline: AppSettings = {
  checkIntervalMinutes: 60, sourceCheckIntervals: {freeSteamKeys: 60, giveeClub: 60, gamesForFarm: 60, zeros: 60}, httpTimeoutSeconds: 30, httpRetryCount: 2,
  giveeClubEnabled: true, gamesForFarmEnabled: true, zerosEnabled: false, freeSteamKeysEnabled: true,
  steamNotificationsEnabled: true, epicGamesNotificationsEnabled: true, gogNotificationsEnabled: true,
  notificationSoundEnabled: true, startAtLogin: false, autostartTarget: "gui", startInTray: true, keepRunningInTray: false,
  automaticUpdateChecks: true,
  languageCode: "en", verboseLogging: false, initialSyncMode: "silent",
  logMaxFileSizeMb: 2, logFileCount: 1, logDirectory: null,
};
const numbers = {intervals: intervalDraft(baseline.sourceCheckIntervals), timeout: "30", retries: "2", logSize: "2", logFiles: "1"};

it("an unopened or unchanged draft has no changes; numeric formatting is not a change", () => {
  expect(settingsHaveChanges(null, baseline, numbers)).toBe(false);
  expect(settingsHaveChanges(baseline, null, numbers)).toBe(false);
  expect(settingsHaveChanges(baseline, {...baseline, sourceCheckIntervals: {...baseline.sourceCheckIntervals}}, {...numbers, intervals: {...numbers.intervals, freeSteamKeys: "060"}})).toBe(false);
});

it.each(["timeout", "retries", "logSize", "logFiles"] as const)("detects edited and invalid %s input", field => {
  for (const value of ["90", "", "invalid"]) {
    expect(settingsHaveChanges(baseline, {...baseline}, {...numbers, [field]: value})).toBe(true);
  }
});

it.each(["freeSteamKeys", "giveeClub", "gamesForFarm", "zeros"] as const)("tracks an independent draft interval for %s, including disabled sources", source => {
  for (const value of ["90", "", "invalid"]) {
    expect(settingsHaveChanges(baseline, {...baseline}, {...numbers, intervals: {...numbers.intervals, [source]: value}})).toBe(true);
  }
  expect(settingsHaveChanges(baseline, {...baseline}, {...numbers, intervals: {...numbers.intervals, [source]: "060"}})).toBe(false);
  expect(baseline.sourceCheckIntervals[source]).toBe(60);
});

it("defaults all four interval fields to one hour and preserves distinct saved intervals", () => {
  expect(intervalDraft()).toEqual({freeSteamKeys: "60", giveeClub: "60", gamesForFarm: "60", zeros: "60"});
  expect(intervalDraft({freeSteamKeys: 15, giveeClub: 120, gamesForFarm: 45, zeros: 90}))
    .toEqual({freeSteamKeys: "15", giveeClub: "120", gamesForFarm: "45", zeros: "90"});
});

it.each([
  "giveeClubEnabled", "gamesForFarmEnabled", "zerosEnabled", "freeSteamKeysEnabled", "steamNotificationsEnabled",
  "epicGamesNotificationsEnabled", "gogNotificationsEnabled", "notificationSoundEnabled", "startAtLogin",
  "keepRunningInTray", "startInTray", "verboseLogging", "automaticUpdateChecks",
] as const)("detects and reverses changes to %s", key => {
  const draft = {...baseline, [key]: !baseline[key]};
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(true);
  draft[key] = baseline[key];
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(false);
});

it("detects language changes and does not modify the baseline", () => {
  const frozen = Object.freeze({...baseline});
  expect(settingsHaveChanges(frozen, {...baseline, languageCode: "ru"}, numbers)).toBe(true);
  expect(frozen.languageCode).toBe("en");
});

it("tracks changing the autostart target without changing the enabled setting", () => {
  const draft: AppSettings = {...baseline, autostartTarget: "cli"};
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(true);
  expect(draft.startAtLogin).toBe(baseline.startAtLogin);
  draft.autostartTarget = "gui";
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(false);
});

it("an empty retry input is not equivalent to the saved zero", () => {
  const noRetries = {...baseline, httpRetryCount: 0};
  expect(settingsHaveChanges(noRetries, {...noRetries}, {...numbers, retries: ""})).toBe(true);
});

it("folder edits are tracked, and returning to the default restores the original draft", () => {
  const draft = {...baseline, logDirectory: "C:/Logs" as string | null};
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(true);
  draft.logDirectory = null;
  expect(settingsHaveChanges(baseline, draft, numbers)).toBe(false);
});

it.each([
  [1, 100], [1, 10], [5, 1_440],
])("validates integers in the inclusive range %i–%i", (minimum, maximum) => {
  for (const value of [String(minimum), String(maximum), String(minimum).padStart(2, "0")]) {
    expect(isIntegerInRange(value, minimum, maximum)).toBe(true);
  }
  for (const value of ["", " ", "NaN", "Infinity", "2.5", "0", String(maximum + 1), "-1"]) {
    expect(isIntegerInRange(value, minimum, maximum)).toBe(false);
  }
});
