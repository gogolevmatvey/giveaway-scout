import { describe, expect, it } from "vitest";
import type { NotificationRecord } from "./api";
import { detectedSources, notificationDetectorNames, notificationOriginName, sourceCatalog } from "./history";

const merged: NotificationRecord = {
  id: "merged", trackingKey: "fsk:1", campaignId: "givee:123", source: "freeSteamKeys",
  originSource: "giveeClub", originName: "Givee Club", detectedSources: ["freeSteamKeys", "giveeClub"],
  store: "steam", title: "Приключение ONE", kind: "giveaway", url: "https://givee.club/ru/event/123",
  receivedAtUtc: "2026-11-07T07:00:00Z", readAtUtc: null, deliveryState: "pending", deliveryAttempts: 0, lastDeliveryError: null,
};
const other: NotificationRecord = { ...merged, id: "other", title: "Different ONE", source: "gamesForFarm", originSource: "gamesForFarm", originName: "Games for Farm", detectedSources: ["gamesForFarm"], store: "unknown", readAtUtc: "2026-11-07T08:00:00Z" };

describe("campaign history", () => {
  it("lists sources in the product priority order", () => {
    expect(sourceCatalog.map(source => source.source)).toEqual(["freeSteamKeys", "giveeClub", "gamesForFarm", "zeros"]);
  });
  it("places the origin first and preserves all attribution without repeating a single origin", () => {
    expect(notificationOriginName(merged)).toBe("Givee Club");
    expect(notificationDetectorNames(merged)).toBe("FreeSteamKeys, Givee Club");
    expect(notificationDetectorNames(other)).toBeNull();
    expect(notificationOriginName({ ...merged, originSource: null, originName: "Gleam" })).toBe("Gleam");
  });
  it("supports older PascalCase source-only notification records", () => {
    const legacy = { ...merged, source: "FreeSteamKeys", originName: undefined, originSource: undefined, detectedSources: undefined };
    expect(detectedSources(legacy)).toEqual(["freeSteamKeys"]);
    expect(notificationDetectorNames(legacy)).toBeNull();
    expect(notificationOriginName(legacy)).toBe("FreeSteamKeys");
  });
});
