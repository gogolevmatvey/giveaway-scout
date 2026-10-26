import type { i18n as I18n } from "i18next";
import { afterAll, beforeAll, expect, it, vi } from "vitest";
import type { SourceFailure } from "./api";
import { sourceFailureText } from "./source-errors";

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
  ["ru", { code: "invalidFeed" }, "изменил формат данных"],
  ["en", { code: "invalidFeed" }, "changed its data format"],
  ["ru", { code: "http", status: 503 }, "ошибку HTTP 503"],
  ["en", { code: "http", status: 503 }, "returned HTTP 503"],
  ["ru", { code: "http", status: 429 }, "ограничил число запросов"],
  ["en", { code: "http", status: 429 }, "limited the number of requests"],
] as const)("source errors in %s use structured reason %o", (lng, reason, expected) => {
  const failure: SourceFailure = { source: "zeros", message: "RAW_DIAGNOSTIC <script>server text</script>", reason };
  const text = sourceFailureText(failure, instance.getFixedT(lng));
  expect(text).toContain(`Zeros: `);
  expect(text).toContain(expected);
  expect(text).not.toContain(failure.message);
});

it.each(["ru", "en"] as const)("unknown and legacy failures in %s never expose raw diagnostics", lng => {
  for (const reason of [undefined, {code:"futureError"}, {code:"unknown"}, {code:"http",status:NaN}]) {
    const failure = { source:"gamesForFarm", message:"RAW_DIAGNOSTIC",reason } as SourceFailure;
    expect(sourceFailureText(failure,instance.getFixedT(lng)))
      .toBe(`Games for Farm: ${instance.t("sourceErrorUnknown",{lng})}`);
  }
});

it.each([
  ["ru", 0, "Создано уведомлений: 0"],
  ["ru", 3, "Создано уведомлений: 3"],
  ["en", 0, "Notifications created: 0"],
  ["en", 3, "Notifications created: 3"],
] as const)("initial synchronization status in %s reports %i notifications", (lng, count, expected) => {
  expect(instance.t("checkBaselineReady", { lng, count })).toContain(expected);
});
