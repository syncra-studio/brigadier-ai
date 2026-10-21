import type { ComponentProps } from "react";

import { SettingsRow } from "@/components/shared";
import { cn } from "@/lib/utils";

/**
 * A Settings row that stays usable while what it needs is off, dimmed (`dimmed`): the
 * notification rows while notifications are off, which apply once they're on.
 */
export function DimmedRow({ dimmed, ...props }: Omit<ComponentProps<typeof SettingsRow>, "className"> & { dimmed: boolean }) {
  return <SettingsRow {...props} className={cn(dimmed && "opacity-50")} />;
}
