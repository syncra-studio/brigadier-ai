import { type FC, useContext } from "react";

import { ReviewContext } from "@/app/side-panel/reviewContext";
import { useCheckoutRoot } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { ExternalLink, FolderOpen, Folders } from "@/components/icons";
import { openFolder, revealPath } from "@/ipc/client";
import type { ReviewFile } from "@/ipc/generated";
import { openFileTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/** A changed file's "Open in" menu: a file tab, its default app, or the file manager. */
export const OpenInMenu: FC<{ file: ReviewFile }> = ({ file }) => {
  const conversationId = useContext(ReviewContext);
  const root = useCheckoutRoot();
  const mac = useApp((s) => s.info?.platform === "macos");
  const gone = file.status === "deleted";
  const absolute = root ? `${root.replace(/\/$/, "")}/${file.path}` : null;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton tooltip="Open in" size="icon-xs" disabled={gone}>
          <ExternalLink />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={() => openFileTab(conversationId, file.path)}>
          <Folders />
          File tab
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!absolute}
          onSelect={() => absolute && openFolder(absolute).catch(failed)}
        >
          <ExternalLink />
          Default app
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={!absolute}
          onSelect={() => absolute && revealPath(absolute).catch(failed)}
        >
          <FolderOpen />
          {mac ? "Finder" : "File manager"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};
