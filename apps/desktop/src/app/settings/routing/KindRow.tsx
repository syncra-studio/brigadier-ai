import type { ComponentType, SVGProps } from "react";

import { NowPick } from "@/app/settings/routing/NowPick";
import { WhoPicks } from "@/app/settings/routing/WhoPicks";
import { OpenChevron } from "@/app/settings/routing/parts";
import { useListedModels } from "@/app/settings/routing/pickers";
import { refusingRule, rowChoice } from "@/app/settings/routing/ruleWords";
import { rowModelName, type WorkRow } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Code, Grid, Nodes, PullRequestOpen, Telescope } from "@/components/icons";
import type { ModelRules } from "@/ipc/generated";
import { joinWords } from "@/lib/routing";

/** The shipped kinds of work's icons, as the design draws kinds of work; a kind of your own gets the grid. */
const KIND_ICONS: Record<string, ComponentType<SVGProps<SVGSVGElement>>> = {
  "Architecture and broad changes": Nodes,
  "Bounded implementation": Code,
  "Mechanical changes and search": Telescope,
  "Independent review": PullRequestOpen,
};

/**
 * A kind of work in Who does what: its icon, name and who picks (Automatic with the shipped
 * reason, or your order), the model it starts on now (the first your rules allow); open, Who picks.
 */
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
  const models = useListedModels();
  const name = (found: WorkRow["rows"][number]) => rowModelName(groups, found.agent, found.model);
  const now = row.rows.find((found) => !refusingRule(rules.rules, rowChoice(found, models), row.work));
  const who = row.mine.length > 0 ? `Your order · ${row.mine.map(name).join(", then ")}` : "Automatic";
  // Every model it names kept out by a rule: it can't start until one is let in or another picked.
  const line = !now
    ? `${who} · Your rules keep ${joinWords(row.rows.map(name))} out, so it can't start`
    : row.mine.length > 0
      ? who
      : `${who} · ${row.shipped[0]?.reason ?? ""}`;
  return (
    <div data-slot="routing-kind" data-work={row.work} data-open={open || undefined} className="flex flex-col">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-3.5 py-3.5 text-start">
        <Icon aria-hidden className="text-foreground/65 size-4 shrink-0" />
        <RowText truncate title={row.work} line={line} />
        {now && <NowPick row={now} groups={groups} />}
        <OpenChevron open={open} />
      </button>
      {open && <WhoPicks row={row} rules={rules} groups={groups} />}
    </div>
  );
}
