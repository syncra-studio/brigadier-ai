import { useId, type ReactNode } from "react";

import { Checkbox } from "@/components/controls/checkbox";
import { cn } from "@/lib/utils";

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
