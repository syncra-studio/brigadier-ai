import { UsageBar } from "@/components/shared/usage-bar";
import { levelTone, USAGE_DANGER_FROM, type UsageLevel } from "@/lib/quota";
import { cn } from "@/lib/utils";

/** Used % past which a provider's usage turns amber in the list (the user's design). */
const USAGE_AMBER_PAST = 75;

/** A share's colour in the list: amber past 75% used, red at a reached limit or from 90%. */
function shareTone(percent: number, limitReached = false): UsageLevel {
  if (limitReached || percent >= USAGE_DANGER_FROM) return "danger";
  return percent > USAGE_AMBER_PAST ? "warning" : "normal";
}

/** A limit's thin bar and its share, amber past 75% used, in usage's shared colours. */
export function Share({ percent, limitReached = false }: { percent: number; limitReached?: boolean }) {
  const level = shareTone(percent, limitReached);
  const look = levelTone(level, percent === 0 ? { text: "text-muted-foreground" } : {});
  return (
    <>
      <UsageBar value={percent} fill={look.fill} className="w-11" />
      <span
        data-slot="usage-value"
        data-level={level}
        className={cn("w-8.5 shrink-0 text-end text-xs font-medium tabular-nums", look.text)}
      >
        {percent}%
      </span>
    </>
  );
}
