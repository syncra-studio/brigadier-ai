import { Check, Copy, DownloadSimple, X } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { BarItem } from "@/app/BarItem";
import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { FootRow, footMenuPlacement } from "@/app/sidebar/nav";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useSidebar } from "@/components/ui/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { UpdateItem, UpdateTarget } from "@/ipc/generated";
import { noteUpdatesSeen, shownUpdates, skipUpdate, takeUpdate, useUpdates } from "@/state/updates";

const UPDATE_NAMES: Record<UpdateTarget, string> = {
  app: "Brigadier",
  claude: PROVIDER_LABELS.claude,
  codex: PROVIDER_LABELS.codex,
};

const CLIS = ["claude", "codex"] as const;

/**
 * Updates at the sidebar's foot, there only while something has a newer version: "Updates
 * available" with the outdated app's and agents' marks, or a download icon with a dot on the
 * collapsed strip. It opens their versions, to update or skip.
 */
export function UpdatesRow() {
  const view = useUpdates((s) => s.view);
  const skipped = useUpdates((s) => s.skipped);
  const seen = useUpdates((s) => s.seen);
  const items = shownUpdates(view, skipped, seen);
  const { open: expanded } = useSidebar();
  // Kept while the row leaves, so it doesn't empty as it shrinks.
  const [last, setLast] = useState(items);
  if (items.length > 0 && JSON.stringify(items) !== JSON.stringify(last)) setLast(items);
  const rows = items.length > 0 ? items : last;
  const outdated = new Set(rows.map((item) => item.target));
  const busy = rows.some((item) => item.progress.type === "updating");
  return (
    <BarItem show={items.length > 0} axis="y">
      <Popover
        onOpenChange={(opened) => {
          if (!opened) noteUpdatesSeen();
        }}
      >
        <PopoverTrigger asChild>
          <FootRow
            label={busy ? "Updating…" : "Updates available"}
            tip={rowLabel(rows)}
            icon={
              busy ? (
                <Spinner aria-hidden className="animate-spin" />
              ) : (
                <DownloadSimple aria-hidden />
              )
            }
            dot={expanded || busy ? null : "bg-foreground"}
            end={
              <>
                <BarItem show={outdated.has("app")}>
                  <BrigadierGlyph aria-hidden className="size-icon-sm mx-0.5 shrink-0" />
                </BarItem>
                {CLIS.map((cli) => (
                  <BarItem key={cli} show={outdated.has(cli)}>
                    <ProviderGlyph provider={cli} className="size-icon-sm mx-0.5 shrink-0" />
                  </BarItem>
                ))}
              </>
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
          className="flex w-sm flex-col gap-1 p-1.5"
        >
          {rows.map((item) => (
            <UpdateRow key={item.target} item={item} />
          ))}
          {rows.some((item) => item.target !== "app" && item.progress.type !== "available") && (
            <p className="text-muted-foreground px-1.5 pb-1 text-xs">
              Sessions already running keep the version they started with.
            </p>
          )}
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
  return `${names} ${items.length > 1 ? "updates" : "update"} available`;
}

function UpdateRow({ item }: { item: UpdateItem }) {
  const [error, setError] = useState<string | null>(null);
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const name = UPDATE_NAMES[item.target];
  const { progress, action } = item;
  const shownError = error ?? (progress.type === "failed" ? progress.error : null);
  const take = () => {
    setError(null);
    takeUpdate(item).catch((cause: unknown) => setError(errorText(cause)));
  };
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
          <span className="text-muted-foreground flex items-center gap-1 text-xs">
            <Check className="size-icon-sm" />
            Done
          </span>
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
