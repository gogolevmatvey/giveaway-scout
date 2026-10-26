import type { AppSettings } from "./api";

export function settingsHaveChanges(
  baseline: AppSettings | null,
  draft: AppSettings | null,
  numbers: { interval: string; timeout: string; retries: string },
): boolean {
  if (!baseline || !draft) return false;
  if (Object.values(numbers).some(value => value.trim() === "")) return true;
  const candidate: AppSettings = {
    ...draft,
    checkIntervalMinutes: Number(numbers.interval),
    httpTimeoutSeconds: Number(numbers.timeout),
    httpRetryCount: Number(numbers.retries),
  };
  return (Object.keys(baseline) as (keyof AppSettings)[])
    .some((key) => baseline[key] !== candidate[key]);
}
