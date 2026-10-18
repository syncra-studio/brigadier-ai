import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

/*
 * The pill buttons of the dialogs drawn to the user's design (Trust this folder?, Add project):
 * 30 high, 13px medium. `primary` is the light one on the right, `secondary` a raised pill with
 * an edge, `quiet` text only (Cancel, Clone from URL).
 */
const LOOKS = {
  primary: "bg-settings-text text-settings-on-light hover:opacity-90 px-3.5",
  secondary: "bg-settings-raised border-settings-ring text-settings-text hover:bg-settings-border border px-3.5",
  quiet: "text-settings-text-secondary hover:text-settings-text hover:bg-settings-raised px-3",
} as const;

export function DialogPill({
  look,
  className,
  ...props
}: ComponentProps<"button"> & { look: keyof typeof LOOKS }) {
  return (
    <button
      type="button"
      {...props}
      className={cn(
        "rounded-capsule text-settings-small flex h-7.5 shrink-0 items-center gap-1.75 font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-settings-accent disabled:pointer-events-none disabled:opacity-50",
        LOOKS[look],
        className,
      )}
    />
  );
}
