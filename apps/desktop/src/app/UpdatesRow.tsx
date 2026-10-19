import { Check, Copy, DownloadSimple, X } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { BarItem, ITEM_MS } from "@/app/BarItem";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { FootButton, footMenuPlacement } from "@/app/sidebar/nav";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useSidebar } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { UpdateItem, UpdateTarget } from "@/ipc/generated";
import { pillClick, shownUpdates, skipUpdate, startUpdate, useUpdates } from "@/state/updates";

const UPDATE_NAMES: Record<UpdateTarget, string> = {
  app: "Brigadier",
  claude: PROVIDER_LABELS.claude,
  codex: PROVIDER_LABELS.codex,
};

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
          <FootButton
            tip={direct === "menu" ? rowLabel(shown) : `Update ${UPDATE_NAMES[direct.target]} to ${direct.latest}`}
            icon={
              busy ? (
                <Spinner aria-hidden className="animate-spin" />
              ) : finished ? (
                <Check aria-hidden />
              ) : (
                <DownloadSimple aria-hidden />
              )
            }
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
            end={
              expanded && (
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
              )
            }
          />
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

function UpdateRow({ item }: { item: UpdateItem }) {
  const asked = useUpdates((s) => s.errors[item.target]);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const name = UPDATE_NAMES[item.target];
  const { progress, action } = item;
  const shownError = asked ?? (progress.type === "failed" ? progress.error : null);
  const take = () => void startUpdate(item);
  return (
    <div className="flex flex-col gap-1.5 rounded-md px-1.5 py-1.5">
      <div className="flex items-center gap-2.5">
        {item.target === "app" ? (
          <BrigadierGlyph aria-hidden className="size-icon-md shrink-0" />
        ) : (
          <ProviderGlyph provider={item.target} className="size-icon-md shrink-0" />
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm">{name}</span>
          <span className="text-muted-foreground text-xs tabular-nums">
            {progress.type === "updated"
              ? `Updated to ${item.current}`
              : `${item.current} → ${item.latest}`}
          </span>
        </div>
        {progress.type === "updating" ? (
          <span className="text-muted-foreground flex items-center gap-1.5 text-xs">
            <Spinner className="size-icon-sm animate-spin" />
            Updating…
          </span>
        ) : progress.type === "updated" ? (
          <Check aria-label="Updated" className="text-muted-foreground size-icon-sm shrink-0" />
        ) : (
          <>
            {action.type !== "manual" && (
              <Button size="sm" variant="outline" onClick={take}>
                {action.type === "download"
                  ? "Download"
                  : progress.type === "failed"
                    ? "Try again"
                    : "Update"}
              </Button>
            )}
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  size="icon-sm"
                  variant="ghost"
                  aria-label={`Skip ${name} ${item.latest}`}
                  onClick={() => skipUpdate(item)}
                >
                  <X />
                </Button>
              </TooltipTrigger>
              <TooltipContent side="top">Skip this version</TooltipContent>
            </Tooltip>
          </>
        )}
      </div>
      {action.type === "manual" && progress.type === "available" && (
        <div className="flex flex-col gap-1 ps-7">
          <span className="text-muted-foreground text-xs">Run this in a terminal to update:</span>
          <div className="bg-muted/50 flex items-center gap-1 rounded-sm ps-2">
            <code className="min-w-0 flex-1 truncate font-mono text-xs">{action.command}</code>
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={isCopied ? "Copied" : "Copy the command"}
              onClick={() => copyToClipboard(action.command)}
            >
              {isCopied ? <Check /> : <Copy />}
            </Button>
          </div>
        </div>
      )}
      {shownError && (
        <p role="alert" className="text-warning ps-7 text-xs whitespace-pre-line">
          {shownError}
        </p>
      )}
    </div>
  );
}
