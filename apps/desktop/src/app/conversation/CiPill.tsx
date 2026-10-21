import { type FC, useState } from "react";

import { ciPill, ciRunWords, type CiWatchView, shortSha } from "@/app/conversation/activity/slotView";
import { StatusSlotToggle } from "@/app/conversation/StatusSlotToggle";
import { CiGlyph } from "@/components/conversation";
import { openUrl } from "@/ipc/client";

/**
 * "Checking CI · 2 of 5 jobs done", then "CI passed" or "CI failed · test": every push being
 * watched, each run's jobs or how it ended, and its link, inside.
 */
export const CiPill: FC<{ watches: readonly CiWatchView[] }> = ({ watches }) => {
  const [open, setOpen] = useState(false);
  const pill = ciPill(watches);
  return (
    <div data-slot="ci-pill" data-ci={pill.state} className="flex flex-col px-1 py-1">
      <div className="flex min-h-row items-center gap-1">
        <StatusSlotToggle open={open} onToggle={() => setOpen(!open)} icon={<CiGlyph state={pill.state} />}>
          {pill.words}
        </StatusSlotToggle>
      </div>
      {open && (
        <ul className="max-h-queue-max flex flex-col gap-1 overflow-y-auto px-2 pb-1">
          {watches.map((watch) => (
            <li key={`${watch.repo} ${watch.branch} ${watch.sha}`} className="flex flex-col">
              <span className="text-foreground/80 min-h-row-sm flex min-w-0 items-center gap-1.5 ps-6 text-sm">
                <span className="truncate">{watch.repo}</span>
                <span aria-hidden className="text-foreground/40">·</span>
                <span className="truncate">{watch.branch}</span>
                <span aria-hidden className="text-foreground/40">·</span>
                <span className="text-foreground/50 font-mono text-xs">{shortSha(watch.sha)}</span>
              </span>
              {watch.runs.length === 0 ? (
                <span className="text-foreground/50 ps-6 text-sm">{watch.result === null ? "Waiting for CI to start…" : watch.result === "stopped" ? "Stopped watching CI" : "No CI runs"}</span>
              ) : (
                watch.runs.map((run) => (
                  <button
                    key={run.url}
                    type="button"
                    onClick={() => void openUrl(run.url).catch(() => {})}
                    className="hover:bg-foreground/5 rounded-control min-h-row-sm text-foreground/60 hover:text-foreground flex min-w-0 items-center gap-1.5 ps-6 pe-2 text-start text-sm"
                  >
                    <span className="min-w-0 flex-1 truncate">{run.workflow}</span>
                    <span className="shrink-0 tabular-nums">{ciRunWords(run)}</span>
                  </button>
                ))
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};
