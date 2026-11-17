import type { i18n as I18n } from "i18next";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import type { SourceFailure } from "./api";
import { sourceFailureText, sourceName } from "./source-errors";

let instance: I18n;

beforeAll(async () => {
  vi.stubGlobal("navigator", { language: "ru-RU" });
  vi.stubGlobal("document", { documentElement: { lang: "" } });
  instance = (await import("./i18n")).default;
  await new Promise<void>((resolve) => {
    if (instance.isInitialized) resolve();
    else instance.once("initialized", () => resolve());
  });
});

afterAll(() => vi.unstubAllGlobals());

it.each([
  ["ru", { code: "zerosConnectionLimit" }, "одно подключение на внешний IP"],
  ["en", { code: "zerosConnectionLimit" }, "one connection per public IP"],
  ["ru", { code: "timeout" }, "не ответил вовремя"],
  ["en", { code: "timeout" }, "did not respond in time"],
  ["ru", { code: "connection" }, "Не удалось подключиться"],
  ["en", { code: "connection" }, "Could not connect"],
  ["ru", { code: "invalidFeed" }, "Не удалось распознать список раздач."],
  ["en", { code: "invalidFeed" }, "The giveaway list could not be recognized."],
  ["ru", { code: "http", status: 503 }, "ошибку HTTP 503"],
  ["en", { code: "http", status: 503 }, "returned HTTP 503"],
  ["ru", { code: "http", status: 429 }, "ограничил число запросов"],
  ["en", { code: "http", status: 429 }, "limited the number of requests"],
] as const)("source errors in %s use structured reason %o", (lng, reason, expected) => {
  const failure: SourceFailure = { source: "zeros", message: "RAW_DIAGNOSTIC <script>server text</script>", reason };
  const text = sourceFailureText(failure, instance.getFixedT(lng));
  expect(text).not.toContain("Zeros:");
  expect(text).toContain(expected);
  expect(text).not.toContain(failure.message);
});

it.each(["ru", "en"] as const)("unknown and incomplete failure summaries in %s do not interpret diagnostic text", lng => {
  for (const reason of [undefined, null, {code:"futureError"}, {code:"unknown"},
    ...[undefined, NaN, 99, 600, 429.5, "503"].map(status => ({code:"http",status}))]) {
    const failure = { source:"gamesForFarm", message:"RAW_DIAGNOSTIC",reason } as SourceFailure;
    expect(sourceFailureText(failure,instance.getFixedT(lng)))
      .toBe(instance.t("sourceErrorUnknown",{lng}));
  }
});

it.each(["freeSteamKeys", "FreeSteamKeys"])("displays the FreeSteamKeys name for %s records", source => {
  expect(sourceName(source)).toBe("FreeSteamKeys");
});

it.each(["ru", "en"] as const)("FreeSteamKeys failures in %s use the localized structured reason", lng => {
  const failure: SourceFailure = {
    source: "freeSteamKeys", message: "RAW_DIAGNOSTIC", reason: {code: "http", status: 429},
  };
  expect(sourceFailureText(failure, instance.getFixedT(lng)))
    .toBe(instance.t("sourceErrorRateLimit", {lng, status: 429}));
});

it.each(["ru", "en"] as const)("source summaries in %s do not replace errors with connection or retry advice", lng => {
  for (const reason of [{ code: "connection" }, { code: "timeout" }, { code: "invalidFeed" }, { code: "http", status: 503 }, { code: "http", status: 429 }] as const) {
    const failure: SourceFailure = { source: "gamesForFarm", message: "actual cause", reason };
    expect(sourceFailureText(failure, instance.getFixedT(lng)))
      .not.toMatch(/Проверьте соединение|Повторите|Возможно|Check your connection|Try again|may have/i);
  }
});

it.each(["ru", "en"] as const)("localized errors in %s leave the original diagnostic unchanged", lng => {
  const message = "https://freeanywhere.net/: error sending request: connection reset by peer (os error 10054)\r\n<script>server text</script>\u001b[31m\u0000";
  const failure: SourceFailure = Object.freeze({ source: "gamesForFarm", message, reason: Object.freeze({ code: "connection" }) });
  expect(sourceFailureText(failure, instance.getFixedT(lng))).toBe(instance.t("sourceErrorConnection", {lng}));
  expect(failure.message).toBe(message);
});

it.each([
  ["ru", 0, "Создано уведомлений: 0"],
  ["ru", 3, "Создано уведомлений: 3"],
  ["en", 0, "Notifications created: 0"],
  ["en", 3, "Notifications created: 3"],
] as const)("initial synchronization status in %s reports %i notifications", (lng, count, expected) => {
  expect(instance.t("checkBaselineReady", { lng, count })).toContain(expected);
});
