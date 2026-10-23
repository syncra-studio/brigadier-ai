import { forwardRef, type ReactNode, useId } from "react";

import { PickerButton } from "@/components/controls";
import { composerPill } from "@/components/conversation";
import { cn } from "@/lib/utils";

/** A rail trigger: the composer's pill, in the rail's bright text. */
export const RailPill = forwardRef<
  HTMLButtonElement,
  { icon: ReactNode; label: string; title?: string; children: ReactNode }
>(({ icon, label, title, children, ...props }, ref) => {
  const value = useId();
  return (
    <PickerButton
      ref={ref}
      label={label}
      shown={[value]}
      title={title}
      className={cn(composerPill, "text-foreground max-w-xs")}
      {...props}
    >
      {icon}
      <span id={value} className="truncate">{children}</span>
    </PickerButton>
  );
});
RailPill.displayName = "RailPill";

