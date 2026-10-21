import type { FC } from "react";

import { failed } from "@/app/conversation/activity/previewProofs";
import { buttonVariants } from "@/components/controls";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/overlays";
import { ArrowUpRight, ChevronDown } from "@/components/icons";
import { openUrl } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { openLink } from "@/state/links";
import { openBrowserTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/** The design's split pill: Open opens the page where Settings › Browser sends it; ▾ picks where. */
export const OpenPill: FC<{ url: string; conversationId: string | null }> = ({ url, conversationId }) => {
  const browser = useApp((s) => s.settings.browserEnabled && s.info?.platform !== "linux");
  const inApp = browser && conversationId !== null;
  // A Browser tab already on the page comes to the front (#425).
  const open = () => void openLink(url, conversationId ? { agent: true, conversationId } : { agent: true }).catch(failed);
  const half = "hover:bg-foreground/10 flex h-full items-center";
  return (
    <span
      data-slot="preview-open"
      className={cn(buttonVariants({ variant: "outline", size: "sm" }), "rounded-nav border-border bg-foreground/5 text-label gap-0 overflow-hidden border px-0 font-normal")}
    >
      <button type="button" onClick={open} title={url} className={cn(half, "gap-1.5 ps-3 pe-2")}>
        <ArrowUpRight aria-hidden className="size-icon-sm" />
        Open
      </button>
      <span aria-hidden className="bg-divider h-3.5 w-px" />
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label="Open in" className={cn(half, "text-foreground/65 ps-1.5 pe-2")}>
            <ChevronDown aria-hidden className="size-icon-sm" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {inApp && <DropdownMenuItem onSelect={() => conversationId && openBrowserTab(conversationId, url, true)}>Brigadier browser</DropdownMenuItem>}
          <DropdownMenuItem onSelect={() => void openUrl(url).catch(failed)}>Default browser</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
};
