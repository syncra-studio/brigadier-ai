import { type RefObject, useEffect, useMemo, useState } from "react";

import { addressSuggestions, type OpenTab, type Suggestion } from "./browserSuggestions";

import { Globe, Search, X } from "@/components/icons";
import { Popover, PopoverAnchor, PopoverContent } from "@/components/overlays";
import type { BrowserVisit } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { browserHistory, removeHistory } from "@/state/browserData";
import { useBrowsers } from "@/state/browsers";
import { useSessionTabs } from "@/state/sessionTabs";

/** The session's other Browser tabs, with what their pages show now. */
function useOpenTabs(self: string): OpenTab[] {
  const session = useSessionTabs((s) => Object.values(s.sessions).find((each) => each.tabs.some((tab) => tab.id === self)));
  const pages = useBrowsers((s) => s.pages);
  return useMemo(
    () =>
      (session?.tabs ?? []).flatMap((tab) =>
        tab.kind === "browser"
          ? [{ id: tab.id, url: pages[tab.id]?.url || tab.url, title: pages[tab.id]?.title || tab.title }]
          : [],
      ),
    [session, pages],
  );
}

/** What the address field suggests for `typed` (null: not typing): see `addressSuggestions`. */
export function useAddressSuggestions(self: string, typed: string | null) {
  const tabs = useOpenTabs(self);
  const [visits, setVisits] = useState<BrowserVisit[]>([]);
  const [changed, setChanged] = useState(0);
  const query = typed?.trim() ?? "";
  useEffect(() => {
    if (!query) return;
    let current = true;
    const timer = setTimeout(() => {
      browserHistory(query, "all", null, 24).then(
        (found) => current && setVisits(Array.isArray(found) ? found : []),
        () => current && setVisits([]),
      );
    }, 80);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // `changed` reads the history again after a removal.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [query, changed]);
  const rows = useMemo(() => addressSuggestions(query, tabs, query ? visits : [], self), [query, tabs, visits, self]);
  const remove = (id: number) => {
    setVisits((shown) => shown.filter((visit) => visit.id !== id));
    removeHistory([id]).then(
      () => setChanged((count) => count + 1),
      () => setChanged((count) => count + 1),
    );
  };
  return { rows, remove };
}

const label = (row: Suggestion) =>
  row.kind === "go" ? (row.search ? `Search the web for “${row.text}”` : row.url) : row.title || row.url;

/**
 * The address field's suggestions, under the field while it is typed in. The keyboard stays in
 * the field: ↑ and ↓ pick a row, Enter takes it, a click takes one, and × removes a page from
 * the history.
 */
export function AddressSuggestions({
  anchor,
  field,
  rows,
  open,
  highlight,
  onPick,
  onHighlight,
  onRemove,
  onClose,
}: {
  anchor: RefObject<HTMLDivElement | null>;
  field: RefObject<HTMLInputElement | null>;
  rows: Suggestion[];
  open: boolean;
  highlight: number;
  onPick: (row: Suggestion) => void;
  onHighlight: (index: number) => void;
  onRemove: (id: number) => void;
  onClose: () => void;
}) {
  return (
    <Popover open={open && rows.length > 0} onOpenChange={(next) => { if (!next) onClose(); }}>
      <PopoverAnchor virtualRef={anchor as RefObject<HTMLDivElement>} />
      <PopoverContent
        align="start"
        data-slot="browser-suggestions"
        className="w-(--radix-popover-trigger-width) p-1"
        onOpenAutoFocus={(event) => event.preventDefault()}
        onCloseAutoFocus={(event) => event.preventDefault()}
        // A click in the field types on.
        onInteractOutside={(event) => {
          if (event.target instanceof Node && anchor.current?.contains(event.target)) event.preventDefault();
        }}
      >
        <div id="browser-suggestions" role="listbox" aria-label="Suggestions">
          {rows.map((row, index) => {
            const Icon = row.kind === "go" && row.search ? Search : Globe;
            return (
              <div
                key={row.kind === "visit" ? `visit-${row.id}` : row.kind === "tab" ? `tab-${row.id}` : "go"}
                id={`browser-suggestion-${index}`}
                role="option"
                tabIndex={-1}
                aria-selected={index === highlight}
                data-kind={row.kind}
                // The keyboard stays in the field.
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => { if (index !== highlight) onHighlight(index); }}
                onClick={() => onPick(row)}
                onKeyDown={(event) => event.key === "Enter" && onPick(row)}
                className={cn(
                  "group/suggestion text-browser h-browser-menu-item flex cursor-default items-center gap-2 rounded-capsule ps-2 pe-1",
                  index === highlight && "bg-foreground/8",
                )}
              >
                <Icon aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
                <span className="min-w-0 flex-1 truncate">
                  {label(row)}
                  {row.kind !== "go" && row.title && (
                    <span className="text-muted-foreground"> · {row.url.replace(/^https?:\/\/(www\.)?/, "")}</span>
                  )}
                </span>
                {row.kind === "tab" && <span className="text-muted-foreground shrink-0 text-xs">Switch to tab</span>}
                {row.kind === "visit" && (
                  <button
                    type="button"
                    aria-label={`Remove ${row.title || row.url} from history`}
                    onMouseDown={(event) => event.preventDefault()}
                    onClick={(event) => {
                      event.stopPropagation();
                      onRemove(row.id);
                      field.current?.focus();
                    }}
                    className="hover:bg-foreground/8 text-muted-foreground flex size-6 shrink-0 items-center justify-center rounded-full opacity-0 group-hover/suggestion:opacity-100 group-aria-selected/suggestion:opacity-100 focus-visible:opacity-100"
                  >
                    <X className="size-icon-sm" />
                  </button>
                )}
              </div>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}
