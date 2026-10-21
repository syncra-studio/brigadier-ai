import type { FC } from "react";

import type { CiState } from "@/app/conversation/activity/slotView";
import { Check, Spinner, X } from "@/components/icons";

/** The CI pill's icon: a spinner while it checks, then how it ended. */
export const CiGlyph: FC<{ state: CiState }> = ({ state }) => {
  switch (state) {
    case "checking":
      return <Spinner className="size-icon-xs animate-spin motion-reduce:animate-none" />;
    case "passed":
      return <Check className="text-success size-icon-xs" />;
    case "failed":
      return <X className="text-destructive size-icon-xs" />;
    case "ended":
      return <span className="border-foreground/35 size-icon-xs rounded-full border" />;
  }
};
