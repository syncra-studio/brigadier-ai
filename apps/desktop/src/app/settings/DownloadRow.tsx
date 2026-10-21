import { errorText } from "@/app/shell/dialogs/fields";
import { Button } from "@/components/controls";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { DotsHorizontal, Download, Folder, Trash } from "@/components/icons";
import { revealPath } from "@/ipc/client";
import type { BrowserDownload } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { siteOf } from "@/state/browserData";

const STATES: Record<BrowserDownload["state"], string | null> = {
  running: "Downloading",
  done: null,
  failed: "Failed",
};

/**
 * One file in Download history: its name over its state and site, the time, and a menu to reveal
 * it (once done) or remove it from the history. `onError` takes what revealing failed with.
 */
export function DownloadRow({
  download,
  mac,
  onRemove,
  onError,
}: {
  download: BrowserDownload;
  mac: boolean;
  onRemove: () => void;
  onError: (error: string) => void;
}) {
  const name = download.path.split(/[\\/]/).pop() || download.path;
  const state = STATES[download.state];
  return (
    <li className="flex min-h-12 items-center gap-3 py-1.5">
      <Download aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="text-label truncate" title={download.path}>{name}</span>
        <span className="text-foreground/65 truncate text-xs">
          {state ? `${state} · ` : ""}
          {siteOf(download.url)}
        </span>
      </span>
      <span className="text-foreground/65 shrink-0 text-xs tabular-nums">
        {formatTime(download.atMs)}
      </span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={`More for ${name}`}
            className="text-foreground/65 shrink-0"
          >
            <DotsHorizontal />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {download.state === "done" && (
            <DropdownMenuItem
              onSelect={() =>
                void revealPath(download.path).catch((cause: unknown) => onError(errorText(cause)))
              }
            >
              <Folder />
              {mac ? "Reveal in Finder" : "Show in folder"}
            </DropdownMenuItem>
          )}
          <DropdownMenuItem variant="destructive" onSelect={onRemove}>
            <Trash />
            Remove from history
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
}
