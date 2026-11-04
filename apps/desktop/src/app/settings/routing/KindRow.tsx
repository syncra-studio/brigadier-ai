import type { ComponentType, SVGProps } from "react";

import { NowPick } from "@/app/settings/routing/NowPick";
import { ROLE_KINDS } from "@/app/settings/routing/roles";
import { SuggestionLine } from "@/app/settings/routing/SuggestionLine";
import { WhoPicks } from "@/app/settings/routing/WhoPicks";
import { OpenChevron } from "@/app/settings/routing/parts";
import { useListedModels } from "@/app/settings/routing/pickers";
import { refusingRule, rowChoice, startsOn } from "@/app/settings/routing/ruleWords";
import { rowModelName, suggestionsFor, workName, type WorkRow } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import { Code, Cursor, Grid, Lifesaver, Nodes, PullRequestMerged, PullRequestOpen, Telescope, Text } from "@/components/icons";
import type { ModelRules } from "@/ipc/generated";
import { joinWords } from "@/lib/routing";
import { useApp } from "@/state/store";

/** The shipped kinds of work's icons, as the design draws kinds of work; a kind of your own gets the grid. */
const KIND_ICONS: Record<string, ComponentType<SVGProps<SVGSVGElement>>> = {
  "Architecture and broad changes": Nodes,
  "Bounded implementation": Code,
  "Mechanical changes and search": Telescope,
  "Adversarial review": PullRequestOpen,
  "Computer use": Cursor,
  Fusion: PullRequestMerged,
  Fallback: Lifesaver,
  "Conversation titles": Text,
  "Rule reading": Text,
};

/**
 * A kind of work in Who does what: its icon, name and who picks (Automatic with the reason the
 * ratings give, or your order), the model it starts on now (the first your rules allow); under
 * your order, a better-rated model when the ratings found one (one a role for a kind of roles);
 * open, Who picks.
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
  const role = ROLE_KINDS[row.work];
  const suggestions = suggestionsFor(rules, row);
  const models = useListedModels();
  const availability = useApp((s) => s.settings);
  const name = (found: WorkRow["rows"][number]) => rowModelName(groups, found.agent, found.model);
  const ruled = row.rows.some((found) => !refusingRule(rules.rules, rowChoice(found, models), row.work));
  // The first your rules let start whose agent and model are on in Providers, as a start picks it.
  const now = startsOn(row.rows, rules.rules, models, availability, row.work);
  const who = row.mine.length > 0 ? `Your order · ${row.mine.map(name).join(", then ")}` : "Automatic";
  // Every model it names kept out by a rule: it can't start until one is let in or another picked.
  const line = role ? `${row.mine.length > 0 ? "Your order" : "Automatic"} · ${role.line}` : !ruled
    ? `${who} · Your rules keep ${joinWords(row.rows.map(name))} out, so it can't start`
    : !now
      ? `${who} · ${joinWords(row.rows.map(name))} ${row.rows.length === 1 ? "is" : "are"} off in Providers, so it can't start`
      : row.mine.length > 0
      ? who
      : `${who} · ${row.shipped[0]?.reason ?? ""}`;
  return (
    <div data-slot="routing-kind" data-work={row.work} data-open={open || undefined} className="flex flex-col">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-3.5 py-3.5 text-start">
        <Icon aria-hidden className="text-foreground/65 size-4 shrink-0" />
        <RowText truncate title={workName(row.work)} line={line} />
        {now && !role && <NowPick row={now} groups={groups} />}
        <OpenChevron open={open} />
      </button>
      {suggestions.map((suggestion) => (
        <SuggestionLine key={`${suggestion.work}/${suggestion.row.agent}`} suggestion={suggestion} row={row} rules={rules} groups={groups} />
      ))}
      {open && <WhoPicks row={row} rules={rules} groups={groups} />}
    </div>
  );
}
