import { useId, type ReactNode } from "react";

import { Checkbox } from "@/components/controls";
import { cn } from "@/lib/utils";

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The project dialogs' card: its edge drawn inside it, and the menus' shadow, as drawn. */
export const PROJECT_DIALOG_CARD = "shadow-menu ring-inset";

/** The project dialogs' title: larger and bolder than the app's other dialogs, as drawn. */
export const PROJECT_DIALOG_TITLE = "font-sans text-lg leading-5.25 font-semibold";

/** An error the daemon (or validation) reported, shown where it happened. */
export function ErrorLine({ error, className }: { error: string | null; className?: string }) {
  if (!error) return null;
  return (
    <p role="alert" className={cn("text-destructive text-xs whitespace-pre-wrap", className)}>
      {error}
    </p>
  );
}

/** A checkbox with its label; `note` explains why it is disabled. */
export function CheckboxRow({
  label,
  note,
  checked,
  disabled,
  onCheckedChange,
}: {
  label: string;
  note?: ReactNode;
  checked: boolean;
  disabled?: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div className={cn("flex items-start gap-2.5", disabled && "opacity-60")}>
      <Checkbox
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(state) => onCheckedChange(state === true)}
        className="mt-0.5"
      />
      <label htmlFor={id} className="grid gap-0.5">
        <span className="text-label leading-4">{label}</span>
        {note && <span className="text-foreground/65 text-xs leading-3.75">{note}</span>}
      </label>
    </div>
  );
}
