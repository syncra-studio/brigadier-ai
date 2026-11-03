import { Button } from "@/components/controls";
import { ArrowRotateCw } from "@/components/icons";
import type { RegistryInfo } from "@/ipc/generated";
import { cn } from "@/lib/utils";

/** "3 hours ago": `n` of a unit ago. */
const ago = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;

/** "3 hours ago": how long since a check. */
function since(ms: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - ms) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return ago(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? ago(hours, "hour") : ago(Math.floor(hours / 24), "day");
}

/** "Oct 13": the ratings' date (a calendar day, so read in UTC). */
function day(date: string): string {
  const parsed = new Date(date);
  return Number.isNaN(parsed.getTime())
    ? date
    : parsed.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}

/** The page header's line: when the ratings were updated and checked. */
export function RatingsLine({ registry, now }: { registry: RegistryInfo | null; now: number }) {
  return (
    <span className="text-xs text-muted-foreground block pt-1">
      {registry ? `Ratings updated ${day(registry.updated)}` : "Ratings"}
      {registry?.checkedAtMs != null && ` · checked ${since(registry.checkedAtMs, now)}`}
    </span>
  );
}

/** The page header's Refresh models. */
export function RefreshModels({ label, busy, onRefresh }: { label: string; busy: boolean; onRefresh: () => void }) {
  return (
    <Button variant="soft" size="sm" data-setting={label} className="gap-1.5" disabled={busy} onClick={onRefresh}>
      <ArrowRotateCw aria-hidden className={cn("size-3", busy && "animate-spin motion-reduce:animate-none")} />
      {busy ? "Refreshing…" : label}
    </Button>
  );
}
