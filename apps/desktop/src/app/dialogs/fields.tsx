import { useId, type ReactNode } from "react";

import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** An error the daemon (or validation) reported, shown where it happened. */
export function ErrorLine({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p role="alert" className="text-destructive text-xs whitespace-pre-wrap">
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
        <span className="text-sm">{label}</span>
        {note && <span className="text-muted-foreground text-xs">{note}</span>}
      </label>
    </div>
  );
}
