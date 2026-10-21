import { useEffect, useRef, useState } from "react";

import type { ResolvedDraft } from "@/app/conversation/draftSetup";
import { BranchPopover, ProjectCombobox, WorkInMenu } from "@/app/conversation/RailPickers";
import { ComposerRailItem } from "@/components/conversation";

/** Whether a sideways-scrolling row has more past its right edge. */
function overflowsEnd(element: HTMLElement): boolean {
  return element.scrollLeft + element.clientWidth < element.scrollWidth - 1;
}

/**
 * The rail's utility bar on a new chat: project, where the session works and its branch.
 * It scrolls sideways, fading at the edge, when the composer is too narrow.
 */
export function UtilityBar({ resolved }: { resolved: ResolvedDraft }) {
  const [overflowing, setOverflowing] = useState(false);
  const bar = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const element = bar.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setOverflowing(overflowsEnd(element)));
    // The bar's own width and its row's, which grows with the pills' text.
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, []);
  return (
    <ComposerRailItem variant="controls" label="Composer utility bar">
      <div
        ref={bar}
        onScroll={(event) => setOverflowing(overflowsEnd(event.currentTarget))}
        data-overflowing={overflowing || undefined}
        className="data-overflowing:mask-fade-end hide-scrollbar overflow-x-auto px-1 py-1.25"
      >
        <div className="flex w-max items-center gap-2">
          <ProjectCombobox project={resolved.project} />
          {resolved.kind === "session" && resolved.repoPath && (
            <>
              <WorkInMenu resolved={resolved} />
              <BranchPopover resolved={resolved} />
            </>
          )}
        </div>
      </div>
    </ComposerRailItem>
  );
}
