import type { ReactNode } from "react";

import { ErrorLine } from "@/app/shell/dialogs/fields";
import { cn } from "@/lib/utils";

/**
 * One setting: its label and description on the start, its control on the end. `htmlFor` ties
 * the label to a control with that id; without it, give the control its own label. `error` is
 * what the last change of it failed with. `setting` is the name search and links find it by,
 * for a label that isn't plain text or changes as it is used.
 */
export function SettingsRow({
  label,
  setting,
  description,
  htmlFor,
  error,
  children,
  className,
}: {
  label: ReactNode;
  setting?: string;
  description?: ReactNode;
  htmlFor?: string;
  error?: string | null;
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div
      data-slot="settings-row"
      // Settings search scrolls to the row it found by this.
      data-setting={setting ?? (typeof label === "string" ? label : undefined)}
      className={cn("@container flex items-center justify-between gap-6 px-4 py-3", className)}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        {htmlFor ? (
          <label htmlFor={htmlFor} className="text-label font-medium break-words">
            {label}
          </label>
        ) : (
          <div className="text-label font-medium break-words">{label}</div>
        )}
        {description && (
          <div className="text-foreground/65 text-xs break-words">{description}</div>
        )}
        <ErrorLine error={error ?? null} />
      </div>
      {children && (
        <div className="flex max-w-full min-w-settings-control shrink-0 items-center justify-end gap-2">
          {children}
        </div>
      )}
    </div>
  );
}
