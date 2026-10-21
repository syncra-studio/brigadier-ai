import { useState } from "react";

import { siteSecurity } from "@/app/side-panel/browserAddress";
import { SECURITY } from "@/app/side-panel/siteInfoText";
import { SiteInfoPopover } from "@/app/side-panel/SiteInfoPopover";
import { TooltipIconButton } from "@/components/controls";
import { Popover, PopoverTrigger } from "@/components/overlays";
import { cn } from "@/lib/utils";
import { openBrowserSettings } from "@/state/browserSettings";
import { useApp } from "@/state/store";

/**
 * The address field's start: a lock for https (shown while the pointer is over the field), "Not
 * secure" for http, nothing on the New tab page. It opens what is known of the connection.
 */
export function SiteInfo({ url }: { url: string }) {
  const security = siteSecurity(url);
  const mac = useApp((s) => s.info?.platform === "macos");
  const [open, setOpen] = useState(false);
  if (!security) return null;
  const { label, Icon } = SECURITY[security];
  let host = url;
  try {
    host = new URL(url).host;
  } catch {
    // siteSecurity read it already.
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <TooltipIconButton
          tooltip={label}
          size="icon-md"
          data-slot="browser-site-info"
          data-security={security}
          className={cn(
            "shrink-0 rounded-full",
            security === "insecure"
              ? "text-destructive w-auto gap-1 px-2 text-xs"
              : "opacity-0 group-hover/address:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100",
          )}
        >
          <Icon />
          {security === "insecure" && <span aria-hidden>Not secure</span>}
        </TooltipIconButton>
      </PopoverTrigger>
      <SiteInfoPopover
        host={host}
        security={security}
        mac={mac}
        onSiteSettings={() => {
          setOpen(false);
          openBrowserSettings("sites");
        }}
      />
    </Popover>
  );
}
