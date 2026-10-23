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

/** Model strength's foot: when the ratings were updated and checked, and Refresh models. */
export function RatingsFooter({
  label,
  registry,
  now,
  running,
  busy,
  disabled,
  onRefresh,
}: {
  /** Refresh models' name. */
  label: string;
  registry: RegistryInfo | null;
  now: number;
  running: boolean;
  /** Refresh models is running. */
  busy: boolean;
  disabled: boolean;
  onRefresh: () => void;
}) {
  return (
    <div className="flex items-center gap-3.5 py-3">
      <span className="text-xs text-muted-foreground min-w-0 flex-1">
        {registry ? `Ratings updated ${day(registry.updated)}` : "Ratings"}
        {registry?.checkedAtMs != null && ` · checked ${since(registry.checkedAtMs, now)}`}
      </span>
      <Button
        variant="link-blue"
        size="inline"
        data-setting={label}
        className="text-xs gap-1.5 font-medium"
        disabled={disabled}
        onClick={onRefresh}
      >
        <ArrowRotateCw
          aria-hidden
          className={cn("size-3", (running || busy) && "animate-spin motion-reduce:animate-none")}
        />
        {running ? "Refreshing…" : label}
      </Button>
    </div>
  );
}
