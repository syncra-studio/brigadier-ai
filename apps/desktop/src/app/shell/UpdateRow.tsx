import { Button } from "@/components/controls";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { BrigadierGlyph, Check, Copy, ProviderGlyph, Spinner, X } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { UpdateItem, UpdateTarget } from "@/ipc/generated";
import { skipUpdate, startUpdate, useUpdates } from "@/state/updates";

export const UPDATE_NAMES: Record<UpdateTarget, string> = {
  app: "Brigadier",
  claude: PROVIDER_LABELS.claude,
  codex: PROVIDER_LABELS.codex,
};

/**
 * One update in the updates list: the app's or agent's mark, name and versions, then Update (or
 * Download), its progress or its check, and Skip; a manual update shows its command to copy.
 */
export function UpdateRow({ item }: { item: UpdateItem }) {
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
