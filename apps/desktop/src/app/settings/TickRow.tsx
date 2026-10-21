import type { ReactNode } from "react";

import { Checkbox } from "@/components/controls";

/** One kind of data in Clear browsing data: its icon, label and line, and a tick box. */
export function TickRow({
  icon,
  label,
  detail,
  checked,
  onCheckedChange,
}: {
  icon: ReactNode;
  label: string;
  detail: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex min-h-14 cursor-pointer items-center gap-3 py-2">
      <span aria-hidden className="text-foreground/80 [&_svg]:size-icon-md flex shrink-0">
        {icon}
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-label">{label}</span>
        <span className="text-foreground/65 text-xs">{detail}</span>
      </span>
      <Checkbox checked={checked} onCheckedChange={(next) => onCheckedChange(next === true)} />
    </label>
  );
}
