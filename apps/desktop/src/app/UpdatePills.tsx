import { Check, Copy, DownloadSimple, X } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useState } from "react";

import { BarItem } from "@/app/BarItem";
import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { BrigadierGlyph } from "@/components/glyphs/brand-glyph";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Spinner } from "@/components/glyphs/spinner";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
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
 * The bottom bar's updates, there only while something has a newer version: "⬇ Brigadier" for
 * the app and "⬇" with the agents' marks for their CLIs. Each opens its versions, to update or
 * skip.
 */
export function UpdatePills() {
  const view = useUpdates((s) => s.view);
  const skipped = useUpdates((s) => s.skipped);
  const seen = useUpdates((s) => s.seen);
  const shown = shownUpdates(view, skipped, seen);
  const app = shown.filter((item) => item.target === "app");
  const clis = shown.filter((item) => item.target !== "app");
  const outdated = new Set(clis.map((item) => item.target));
  return (
    <>
      <BarItem show={app.length > 0}>
        <UpdatePill items={app}>
          <span>Brigadier</span>
        </UpdatePill>
      </BarItem>
      <BarItem show={clis.length > 0}>
        <UpdatePill items={clis}>
          {CLIS.map((cli) => (
            <BarItem key={cli} show={outdated.has(cli)}>
              <ProviderGlyph provider={cli} className="size-icon-sm mx-0.5 shrink-0" />
            </BarItem>
          ))}
        </UpdatePill>
      </BarItem>
    </>
  );
}

/** "Codex update available", "Claude Code and Codex updates available", "Updating Codex". */
function pillLabel(items: readonly UpdateItem[]): string {
  const names = items.map((item) => UPDATE_NAMES[item.target]).join(" and ");
  if (items.some((item) => item.progress.type === "updating")) return `Updating ${names}`;
  return `${names} ${items.length > 1 ? "updates" : "update"} available`;
}

function UpdatePill({ items, children }: { items: UpdateItem[]; children: ReactNode }) {
  // Kept while the pill leaves, so it doesn't empty as it shrinks.
  const [last, setLast] = useState(items);
  if (items.length > 0 && JSON.stringify(items) !== JSON.stringify(last)) setLast(items);
  const rows = items.length > 0 ? items : last;
  const busy = rows.some((item) => item.progress.type === "updating");
  const label = pillLabel(rows);
  return (
    <Popover
      onOpenChange={(open) => {
        if (!open) noteUpdatesSeen();
      }}
    >
      <Tooltip>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-label={label}
              className="border-border text-muted-foreground hover:text-foreground hover:bg-foreground/8 data-[state=open]:text-foreground data-[state=open]:bg-foreground/8 focus-visible:ring-ring/50 mx-0.5 flex h-6 shrink-0 items-center gap-1 rounded-full border ps-1.5 pe-2 text-xs outline-none transition-colors duration-150 focus-visible:ring-2"
            >
              {busy ? (
                <Spinner aria-hidden className="size-icon-sm animate-spin" />
              ) : (
                <DownloadSimple aria-hidden className="size-icon-sm" />
              )}
              {children}
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent side="top">{label}</TooltipContent>
      </Tooltip>
      <PopoverContent side="top" align="end" className="flex w-sm flex-col gap-1 p-1.5">
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
  );
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
