import { Check, Minus, PlanPartial, Spinner, StopCircle, X } from "@/components/icons";
import { cn } from "@/lib/utils";

export type StepStatus = "pending" | "active" | "done" | "partial" | "failed" | "skipped" | "stopped";

/**
 * A step's mark: done, at work, failed, stopped, skipped, partly done, or still to come (a
 * ring). "sm" sits on a plan's text line; "xs" in a 16px square (a plan pill's hover list).
 */
export function StepGlyph({
  status,
  size = "sm",
  className,
}: {
  status: StepStatus;
  size?: "sm" | "xs";
  className?: string;
}) {
  const glyph = size === "sm" ? "size-icon-sm" : "size-icon-xs";
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center",
        size === "sm" ? "h-(--text-sm--line-height) w-icon-md" : "size-4",
        className,
      )}
    >
      {status === "done" ? (
        <Check aria-hidden className={cn("text-muted-foreground", glyph)} />
      ) : status === "active" ? (
        <Spinner aria-hidden className={cn("text-foreground animate-spin motion-reduce:animate-none", glyph)} />
      ) : status === "failed" ? (
        <X aria-hidden className={cn("text-destructive", glyph)} />
      ) : status === "stopped" ? (
        <StopCircle aria-hidden className={cn("text-muted-foreground", glyph)} />
      ) : status === "skipped" ? (
        <Minus aria-hidden className={cn("text-muted-foreground", glyph)} />
      ) : status === "partial" ? (
        <PlanPartial aria-hidden className={cn("text-muted-foreground", glyph)} />
      ) : (
        <span aria-hidden className="border-foreground/35 size-icon-xs rounded-full border" />
      )}
    </span>
  );
}
