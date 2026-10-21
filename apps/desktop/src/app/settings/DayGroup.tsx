import type { ReactNode } from "react";

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/layout";
import { ChevronDown } from "@/components/icons";
import { SettingsCard } from "@/components/shared";

/** One day of Browsing history: a card that folds, the day over its rows (`children`). */
export function DayGroup({ label, children }: { label: string; children: ReactNode }) {
  return (
    <SettingsCard>
      <Collapsible defaultOpen>
        <CollapsibleTrigger className="group flex min-h-11 w-full items-center justify-between text-start text-sm font-medium">
          {label}
          <ChevronDown
            aria-hidden
            className="text-foreground/65 size-icon-sm transition-transform group-data-[state=open]:rotate-180"
          />
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ul className="divide-divider border-divider divide-y border-t">
            {children}
          </ul>
        </CollapsibleContent>
      </Collapsible>
    </SettingsCard>
  );
}
