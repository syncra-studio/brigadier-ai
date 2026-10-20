import type { ReactNode } from "react";

import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { X } from "@/components/icons";
import { cn } from "@/lib/utils";

type BannerProps = {
  /** `error` is red and announced at once; `info` and `warning` sit on a light wash. */
  tone?: "info" | "warning" | "error";
  text: ReactNode;
  /** Its buttons, after the words. */
  children?: ReactNode;
  /** Shows a × that calls it. */
  onDismiss?: () => void;
  className?: string;
  "data-slot"?: string;
};

/** A strip across the top of a pane: what happened, and what to do about it. */
export function Banner({ tone = "info", text, children, onDismiss, className, ...props }: BannerProps) {
  return (
    <div
      data-slot="banner"
      {...props}
      role={tone === "error" ? "alert" : "status"}
      data-tone={tone}
      className={cn(
        "flex shrink-0 items-center gap-2 border-b px-4 py-2 text-sm",
        tone === "error" ? "bg-destructive/10 text-destructive border-destructive/20" : "bg-foreground/5 border-border",
        className,
      )}
    >
      {/* Words and buttons wrap as one; the × keeps to the first line's end. */}
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
        <span data-slot="banner-text" className="min-w-0 flex-1 basis-48 wrap-break-word">{text}</span>
        {children}
      </div>
      {onDismiss && (
        <TooltipIconButton tooltip="Dismiss" size="icon-xs" onClick={onDismiss}>
          <X />
        </TooltipIconButton>
      )}
    </div>
  );
}
