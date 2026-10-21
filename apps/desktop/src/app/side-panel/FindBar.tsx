import type { RefObject } from "react";

import { ChevronDown, ChevronUp, Search, X } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import { findInPage, findLabel, NO_FIND, useBrowserFind } from "@/state/browserFind";

/**
 * The page's find bar, under the toolbar at its end (the page, drawn over the window, would hide
 * it anywhere over the page): the text, which match of how many, the previous and next match,
 * and close. Enter goes to the next match, Shift+Enter the previous, Esc closes.
 */
export function FindBar({ id, input, onClose }: { id: string; input: RefObject<HTMLInputElement | null>; onClose: () => void }) {
  const find = useBrowserFind((s) => s.tabs[id]) ?? NO_FIND;
  const label = findLabel(find);
  const step = (backwards: boolean) => {
    if (find.query) void findInPage(id, find.query, backwards ? "previous" : "next");
  };
  return (
    <div data-slot="browser-find-row" className="border-border flex shrink-0 justify-end border-b px-2 py-1">
      <div
        role="search"
        aria-label="Find in page"
        data-slot="browser-find"
        className="bg-popover text-popover-foreground shadow-browser-find rounded-menu flex h-browser-find-row w-browser-find max-w-full min-w-0 items-center gap-2 ps-4 pe-2"
      >
        <Search aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
        <input
          ref={input}
          value={find.query}
          placeholder="Find in page…"
          aria-label="Find in page"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          onChange={(event) => void findInPage(id, event.target.value, "type")}
          onFocus={(event) => event.target.select()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              step(event.shiftKey);
            } else if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              onClose();
            }
          }}
          className="text-browser placeholder:text-muted-foreground h-6 min-w-0 flex-1 bg-transparent outline-none"
        />
        <span data-slot="browser-find-count" aria-live="polite" className="text-muted-foreground shrink-0 text-xs whitespace-nowrap tabular-nums select-none">
          {label}
        </span>
        <TooltipIconButton tooltip="Previous match" shortcut="⇧↵" size="icon-xs" disabled={!find.found} onClick={() => step(true)}>
          <ChevronUp />
        </TooltipIconButton>
        <TooltipIconButton tooltip="Next match" shortcut="↵" size="icon-xs" disabled={!find.found} onClick={() => step(false)}>
          <ChevronDown />
        </TooltipIconButton>
        <span aria-hidden className="bg-border h-4 w-px shrink-0" />
        <TooltipIconButton tooltip="Close find" size="icon-xs" onClick={onClose}>
          <X />
        </TooltipIconButton>
      </div>
    </div>
  );
}
