import type { FC } from "react";

import { UsageBar } from "@/components/shared";
import { usageTone } from "@/lib/quota";

/** A usage window's bar: the share left, filled; the share used, faint. */
/** What is left of a window, coloured by how much is used (from 80% and from 90%). */
export const LimitBar: FC<{ left: number }> = ({ left }) => (
  <UsageBar
    value={left}
    fill={usageTone(100 - left, { normal: { fill: "bg-foreground" } }).fill}
    size="md"
    className="w-28"
  />
);
