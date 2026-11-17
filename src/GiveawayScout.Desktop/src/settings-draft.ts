import type { AppSettings, GiveawaySource, SourceCheckIntervals } from "./api";
import { sourceCatalog } from "./history";

export type SourceIntervalDraft = Record<GiveawaySource, string>;

export function intervalDraft(intervals?: SourceCheckIntervals): SourceIntervalDraft {
  return Object.fromEntries(sourceCatalog.map(({ source }) => [source, String(intervals?.[source] ?? 60)])) as SourceIntervalDraft;
}

export function settingsHaveChanges(
  baseline: AppSettings | null,
  draft: AppSettings | null,
  numbers: { intervals: SourceIntervalDraft; timeout: string; retries: string; logSize: string; logFiles: string },
): boolean {
  if (!baseline || !draft) return false;
  if ([...Object.values(numbers.intervals), numbers.timeout, numbers.retries, numbers.logSize, numbers.logFiles]
    .some(value => value.trim() === "")) return true;
  const candidate: AppSettings = {
    ...draft,
    sourceCheckIntervals: Object.fromEntries(sourceCatalog.map(({ source }) => [source, Number(numbers.intervals[source])])) as SourceCheckIntervals,
    httpTimeoutSeconds: Number(numbers.timeout),
    httpRetryCount: Number(numbers.retries),
    logMaxFileSizeMb: Number(numbers.logSize),
    logFileCount: Number(numbers.logFiles),
  };
  return (Object.keys(baseline) as (keyof AppSettings)[])
    .some((key) => key === "sourceCheckIntervals"
      ? sourceCatalog.some(({ source }) => baseline.sourceCheckIntervals[source] !== candidate.sourceCheckIntervals[source])
      : baseline[key] !== candidate[key]);
}

export function isIntegerInRange(value: string, minimum: number, maximum: number): boolean {
  const number = Number(value);
  return value.trim() !== "" && Number.isInteger(number) && number >= minimum && number <= maximum;
}
