import { SECURITY } from "@/app/side-panel/siteInfoText";
import { PopoverContent } from "@/components/overlays";
import { cn } from "@/lib/utils";

/** What is known of the page's connection: its host, whether it is secure, and Site settings (macOS). */
export function SiteInfoPopover({ host, security, mac, onSiteSettings }: {
  host: string;
  security: keyof typeof SECURITY;
  mac: boolean;
  onSiteSettings: () => void;
}) {
  const { Icon, title, detail } = SECURITY[security];
  return (
    <PopoverContent align="start" aria-label="Site information" className="w-browser-site-info p-3">
      <p className="text-browser truncate font-medium">{host}</p>
      <div className="mt-3 flex items-start gap-2">
        <Icon aria-hidden className={cn("size-icon-md mt-0.5 shrink-0", security === "insecure" && "text-destructive")} />
        <div className="min-w-0">
          <p className="text-browser">{title}</p>
          <p className="text-muted-foreground mt-0.5 text-xs">{detail}</p>
        </div>
      </div>
      {mac && (
        <>
          <div className="bg-border -mx-3 my-3 h-px" />
          <button
            type="button"
            onClick={onSiteSettings}
            className="hover:bg-foreground/8 text-browser h-browser-menu-item -mx-1 flex w-full items-center rounded-capsule px-1"
          >
            Site settings
          </button>
        </>
      )}
    </PopoverContent>
  );
}
