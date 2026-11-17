import type { GiveawaySource, NotificationRecord } from "./api";
import { sourceName } from "./source-errors";

export const sourceCatalog = [
  { source: "freeSteamKeys", name: "FreeSteamKeys", descriptionKey: "freeSteamKeysDescription", settingKey: "freeSteamKeysEnabled" },
  { source: "giveeClub", name: "Givee Club", descriptionKey: "giveeClubDescription", settingKey: "giveeClubEnabled" },
  { source: "gamesForFarm", name: "Games for Farm", descriptionKey: "gamesForFarmDescription", settingKey: "gamesForFarmEnabled" },
  { source: "zeros", name: "Zeros", descriptionKey: "zerosDescription", settingKey: "zerosEnabled" },
] as const;

export function normalizeSource(source: string): GiveawaySource | null {
  return sourceCatalog.find(item => item.source.toLowerCase() === source.toLowerCase())?.source ?? null;
}

export function detectedSources(notification: NotificationRecord): GiveawaySource[] {
  const legacySource = normalizeSource(notification.source);
  return notification.detectedSources?.length ? notification.detectedSources : legacySource ? [legacySource] : [];
}

export function notificationOriginName(notification: NotificationRecord): string {
  return notification.originName || sourceName(notification.originSource || notification.source);
}

export function notificationDetectorNames(notification: NotificationRecord): string | null {
  const origin = notification.originSource ?? normalizeSource(notificationOriginName(notification).replace(/ /g, ""));
  const detectors = detectedSources(notification);
  // A source finding its own campaign needs no repeated attribution.
  return detectors.length === 1 && detectors[0] === origin ? null : detectors.map(sourceName).join(", ");
}
