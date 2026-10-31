import { expect, it } from "vitest";
import type { AppSettings } from "./api";
import { settingsHaveChanges } from "./settings-draft";

const baseline: AppSettings = {
  checkIntervalMinutes: 60, httpTimeoutSeconds: 30, httpRetryCount: 2,
  giveeClubEnabled: true, gamesForFarmEnabled: true, zerosEnabled: false,
  steamNotificationsEnabled: true, epicGamesNotificationsEnabled: true, gogNotificationsEnabled: true,
  notificationSoundEnabled: true, startAtLogin: false, startInTray: true, keepRunningInTray: false,
  languageCode: "en", verboseLogging: false, initialSyncMode: "silent",
};
const numbers = {interval: "60", timeout: "30", retries: "2"};

it("an unopened or unchanged draft has no changes; numeric formatting is not a change", () => {
  expect(settingsHaveChanges(null, baseline, numbers)).toBe(false);
  expect(settingsHaveChanges(baseline, null, numbers)).toBe(false);
  expect(settingsHaveChanges(baseline, {...baseline}, {...numbers, interval: "060"})).toBe(false);
});

it.each(["interval", "timeout", "retries"] as const)("detects edited and invalid %s input", field => {
  for (const value of ["90", "", "invalid"]) {
    expect(settingsHaveChanges(baseline, {...baseline}, {...numbers, [field]: value})).toBe(true);
  }
});

it.each([
  "giveeClubEnabled", "gamesForFarmEnabled", "zerosEnabled", "steamNotificationsEnabled",
  "epicGamesNotificationsEnabled", "gogNotificationsEnabled", "notificationSoundEnabled", "startAtLogin",
  "keepRunningInTray", "startInTray", "verboseLogging",
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

it("an empty retry input is not equivalent to the saved zero", () => {
  const noRetries = {...baseline, httpRetryCount: 0};
  expect(settingsHaveChanges(noRetries, {...noRetries}, {...numbers, retries: ""})).toBe(true);
});
