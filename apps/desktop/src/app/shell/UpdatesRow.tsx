import { useEffect, useState } from "react";

import { TooltipIconButton } from "@/components/controls";
import { BarItem, ITEM_MS } from "@/app/shell/BarItem";
import { UPDATE_NAMES, UpdateRow } from "@/app/shell/UpdateRow";
import { footMenuPlacement } from "@/app/shell/sidebar/nav";
import { BrigadierGlyph, Check, DownloadSimple, ProviderGlyph, Spinner } from "@/components/icons";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/overlays";
import { useSidebar } from "@/components/layout";
import type { UpdateItem } from "@/ipc/generated";
import { pillClick, shownUpdates, startUpdate, useUpdates } from "@/state/updates";

const CLIS = ["claude", "codex"] as const;

/** The updates pill in the expanded row: outlined, round-ended, filled only on hover. */
export const FOOT_CHIP = "border-border rounded-full border px-2";

/**
 * Updates at the sidebar's foot, there only while something has a newer version: a chip of a
 * download icon and the outdated app's and agents' marks in the expanded sidebar's row, or a
 * download icon with a dot on the collapsed strip. When the one update is an agent's, a click
 * runs it; otherwise it opens their versions, to update or skip. An update that finishes shows
 * its check a moment, then leaves by itself: its row shrinks away, the list closes after its
 * last row, and the chip after the list.
 */
export function UpdatesRow() {
  const view = useUpdates((s) => s.view);
  const skipped = useUpdates((s) => s.skipped);
  const seen = useUpdates((s) => s.seen);
  const items = shownUpdates(view, skipped, seen);
  const { open: expanded } = useSidebar();
  const [open, setOpen] = useState(false);
  // Kept while the chip leaves, so it doesn't empty as it shrinks.
  const [last, setLast] = useState(items);
  if (items.length > 0 && JSON.stringify(items) !== JSON.stringify(last)) setLast(items);
  const shown = items.length > 0 ? items : last;
  // Every row the list has had, so a row that leaves shrinks away rather than vanishing.
  const all = view?.items ?? [];
  const [lastAll, setLastAll] = useState(all);
  if (all.length > 0 && JSON.stringify(all) !== JSON.stringify(lastAll)) setLastAll(all);
  const rows = all.length > 0 ? all : lastAll;
  const outdated = new Set(shown.map((item) => item.target));
  const busy = shown.some((item) => item.progress.type === "updating");
  const finished = shown.length > 0 && shown.every((item) => item.progress.type === "updated");
  const direct = pillClick(items);
  const empty = items.length === 0;
  // The list closes once its last row has shrunk away.
  useEffect(() => {
    if (!empty || !open) return;
    const timer = window.setTimeout(() => setOpen(false), ITEM_MS);
    return () => window.clearTimeout(timer);
  }, [empty, open]);
  return (
    <BarItem show={!empty || open} axis={expanded ? "x" : "y"}>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <TooltipIconButton
            look="strip"
            side={expanded ? "top" : "right"}
            tooltip={direct === "menu" ? rowLabel(shown) : `Update ${UPDATE_NAMES[direct.target]} to ${direct.latest}`}
            dot={expanded || busy || finished ? null : "bg-foreground"}
            className={expanded ? FOOT_CHIP : undefined}
            onClick={(event) => {
              if (direct === "menu") return;
              // Runs before the trigger's own click, which then leaves the list closed.
              event.preventDefault();
              void startUpdate(direct).then((started) => {
                if (!started) setOpen(true);
              });
            }}
          >
            {busy ? (
              <Spinner aria-hidden className="animate-spin" />
            ) : finished ? (
              <Check aria-hidden />
            ) : (
              <DownloadSimple aria-hidden />
            )}
            {expanded && (
              <span className="flex items-center">
                <BarItem show={outdated.has("app")}>
                  <BrigadierGlyph aria-hidden className="size-icon-sm mx-0.5 shrink-0" />
                </BarItem>
                {CLIS.map((cli) => (
                  <BarItem key={cli} show={outdated.has(cli)}>
                    <ProviderGlyph provider={cli} className="size-icon-sm mx-0.5 shrink-0" />
                  </BarItem>
                ))}
              </span>
            )}
          </TooltipIconButton>
        </PopoverTrigger>
        <PopoverContent
          {...footMenuPlacement(expanded)}
          // Focus the list itself, not its first button (whose tip would open with it).
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            if (event.currentTarget instanceof HTMLElement) event.currentTarget.focus();
          }}
          className="flex w-sm flex-col p-1.5"
        >
          {rows.map((item) => (
            <BarItem key={item.target} axis="y" show={items.includes(item)}>
              <UpdateRow item={item} />
            </BarItem>
          ))}
          <BarItem
            axis="y"
            show={items.some((item) => item.target !== "app" && item.progress.type !== "available")}
          >
            <p className="text-muted-foreground px-1.5 pt-1 pb-1 text-xs">
              Sessions already running keep the version they started with.
            </p>
          </BarItem>
        </PopoverContent>
      </Popover>
    </BarItem>
  );
}

/**
 * "Codex update available", "Brigadier and Codex updates available", "Brigadier, Claude Code
 * and Codex updates available", "Updating Codex".
 */
function rowLabel(items: readonly UpdateItem[]): string {
  const all = items.map((item) => UPDATE_NAMES[item.target]);
  const names =
    all.length > 1 ? `${all.slice(0, -1).join(", ")} and ${all.at(-1)}` : (all[0] ?? "");
  if (items.some((item) => item.progress.type === "updating")) return `Updating ${names}`;
  if (items.every((item) => item.progress.type === "updated")) return `${names} updated`;
  return `${names} ${items.length > 1 ? "updates" : "update"} available`;
}
