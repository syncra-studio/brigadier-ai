import type { ReactNode } from "react";

import { ROW, ROW_ACTIVE } from "@/app/conversation/railMenu";
import { Check } from "@/components/icons";
import { MenuRow } from "@/components/overlays";
import { cn } from "@/lib/utils";

/** A picker's row: its icon and name, ✓ on the one in use, lit while highlighted. */
export function PickerRow({
  icon,
  title,
  selected,
  highlighted,
  onPointerMove,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  selected: boolean;
  highlighted: boolean;
  onPointerMove: () => void;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="option"
      aria-selected={selected}
      onPointerMove={onPointerMove}
      onClick={onClick}
      className={cn(ROW, highlighted && ROW_ACTIVE)}
    >
      <MenuRow icon={icon} title={title} />
      {selected && <Check />}
    </button>
  );
}
