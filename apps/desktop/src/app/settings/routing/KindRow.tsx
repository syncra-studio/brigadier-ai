import type { ComponentType, SVGProps } from "react";

import { NowPick } from "@/app/settings/routing/NowPick";
import { WhoPicks } from "@/app/settings/routing/WhoPicks";
import { OpenChevron } from "@/app/settings/routing/parts";
import { decides, orderSummary, rowRanking } from "@/app/settings/routing/words";
import { type ModelGroup, RowText } from "@/components/shared";
import {
  BookOpen,
  Chat,
  CheckCircle,
  Code,
  CommitIcon,
  Grid,
  Nodes,
  PullRequestMerged,
  PullRequestOpen,
  Telescope,
} from "@/components/icons";
import type { RoutePreview, TaskCategory } from "@/ipc/generated";
import { PLAIN_KIND_NAMES } from "@/lib/routing";
import { useApp } from "@/state/store";

/** Each kind of work's icon, as the design draws it. */
const KIND_ICONS: Record<TaskCategory, ComponentType<SVGProps<SVGSVGElement>>> = {
  scout: Telescope,
  research: BookOpen,
  implement: Code,
  review: PullRequestOpen,
  merge: PullRequestMerged,
  verify: CheckCircle,
  operate: Grid,
  chat: Chat,
  orchestrate: Nodes,
  gitText: CommitIcon,
};

/** A kind of work in Who does what: its icon, name and how it picks, what it runs on now; open, Who picks. */
export function KindRow({
  category,
  open,
  route,
  groups,
  now,
  onToggle,
  onSeeStrengths,
}: {
  category: TaskCategory;
  open: boolean;
  route: RoutePreview | null;
  groups: readonly ModelGroup[];
  now: number;
  onToggle: () => void;
  onSeeStrengths: () => void;
}) {
  const rankings = useApp((s) => s.settings.routingRankings);
  const own = rowRanking(rankings, category, null);
  const deciding = decides(own) ? own : null;
  const Icon = KIND_ICONS[category];
  const line = deciding
    ? `Your order · ${orderSummary(deciding, groups)}`
    : category === "chat"
      ? "The chat's own model, then Automatic"
      : "Automatic";
  return (
    <div data-slot="routing-kind" data-open={open || undefined} className="flex flex-col">
      <button type="button" aria-expanded={open} onClick={onToggle} className="flex w-full items-center gap-3.5 py-3.5 text-start">
        <Icon aria-hidden className="text-foreground/65 size-4 shrink-0" />
        <RowText truncate title={PLAIN_KIND_NAMES[category]} line={line} />
        <NowPick category={category} route={route} groups={groups} now={now} />
        <OpenChevron open={open} />
      </button>
      {open && (
        <WhoPicks category={category} own={own} deciding={deciding} route={route} groups={groups} onSeeStrengths={onSeeStrengths} />
      )}
    </div>
  );
}
