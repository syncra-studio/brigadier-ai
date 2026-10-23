import type { ComponentType, SVGProps } from "react";

import { NowPick } from "@/app/settings/routing/NowPick";
import { WhoPicks } from "@/app/settings/routing/WhoPicks";
import { OpenChevron } from "@/app/settings/routing/parts";
import type { WorkRow } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Code, Grid, Nodes, PullRequestOpen, Telescope } from "@/components/icons";
import type { ModelRules } from "@/ipc/generated";

/** The shipped kinds of work's icons, as the design draws kinds of work; a kind of your own gets the grid. */
const KIND_ICONS: Record<string, ComponentType<SVGProps<SVGSVGElement>>> = {
  "Architecture and broad changes": Nodes,
  "Bounded implementation": Code,
  "Mechanical changes and search": Telescope,
  "Independent review": PullRequestOpen,
};

/** A kind of work in Who does what: its icon, name and whose row it follows, the model it names; open, Who picks. */
export function KindRow({
  row,
  rules,
  open,
  groups,
  onToggle,
}: {
  row: WorkRow;
  rules: ModelRules;
  open: boolean;
  groups: readonly ModelGroup[];
  onToggle: () => void;
}) {
  const Icon = KIND_ICONS[row.work] ?? Grid;
  const line = `${row.mine ? "Yours" : "Shipped"} · ${row.row.reason}`;
  return (
    <div data-slot="routing-kind" data-open={open || undefined} className="flex flex-col">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-3.5 py-3.5 text-start">
        <Icon aria-hidden className="text-foreground/65 size-4 shrink-0" />
        <RowText truncate title={row.work} line={line} />
        <NowPick row={row.row} groups={groups} />
        <OpenChevron open={open} />
      </button>
      {open && <WhoPicks row={row} rules={rules} groups={groups} />}
    </div>
  );
}
