import type { TFunction } from "i18next";
import type { SourceFailure } from "./api";

/** Translate the stable category without guessing from transport diagnostics. */
export function sourceFailureText(failure: SourceFailure, t: TFunction): string {
  let description: string;
  switch (failure.reason?.code) {
    case "timeout": description = t("sourceErrorTimeout"); break;
    case "connection": description = t("sourceErrorConnection"); break;
    case "invalidFeed": description = t("sourceErrorInvalidFeed"); break;
    case "zerosConnectionLimit": description = t("sourceErrorZerosConnectionLimit"); break;
    case "http": {
      const status = failure.reason.status;
      description = Number.isInteger(status) && status >= 100 && status <= 599
        ? t(status === 429 ? "sourceErrorRateLimit" : "sourceErrorHttp", { status })
        : t("sourceErrorUnknown");
      break;
    }
    default: description = t("sourceErrorUnknown");
  }
  return description;
}

export function sourceName(source: string): string {
  if (source === "giveeClub" || source === "GiveeClub") return "Givee Club";
  if (source === "gamesForFarm" || source === "GamesForFarm") return "Games for Farm";
  if (source === "zeros" || source === "Zeros") return "Zeros";
  if (source === "freeSteamKeys" || source === "FreeSteamKeys") return "FreeSteamKeys";
  return source;
}
