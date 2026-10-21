import { UsageBar } from "@/components/shared";
import type { ImportProgress } from "@/ipc/generated";

/** How far the cookies have come; indeterminate until they are counted. */
export function ImportBar({ progress }: { progress: ImportProgress | null }) {
  const share = progress?.type === "cookies" && progress.total > 0 ? progress.done / progress.total : progress?.type === "history" ? 1 : null;
  return (
    <UsageBar
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={share === null ? undefined : Math.round(share * 100)}
      value={share === null ? null : share * 100}
      indeterminate={share === null}
      fill="bg-foreground/60"
      size="md"
    />
  );
}
