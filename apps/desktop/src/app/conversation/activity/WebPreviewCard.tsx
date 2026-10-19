import { ChevronDown, Globe } from "@openai/apps-sdk-ui/components/Icon";

import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { openUrl } from "@/ipc/client";
import { openBrowserTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

function failed(cause: unknown): void {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/**
 * The page a turn left running, under its answer: a globe tile, "Web preview" and its address. A
 * click opens it in a Browser tab of the session (the default browser where there is none, or
 * the built-in browser is off); "Open in" picks one.
 */
export function WebPreviewCard({ url, conversationId }: { url: string; conversationId: string | null }) {
  const browser = useApp((s) => s.settings.browserEnabled && s.info?.platform !== "linux");
  const inApp = browser && conversationId !== null;
  const inBrowser = () => {
    if (conversationId) openBrowserTab(conversationId, url, true);
  };
  const inDefault = () => void openUrl(url).catch(failed);
  const address = url.replace(/^https?:\/\//, "").replace(/\/$/, "");
  return (
    <div data-slot="web-preview" className="border-foreground/10 rounded-shell hover:bg-foreground/3 relative flex items-center gap-3 border p-3 transition-colors">
      <button type="button" aria-label={`Open ${address}`} onClick={inApp ? inBrowser : inDefault} className="rounded-shell absolute inset-0" />
      <span aria-hidden className="bg-background text-link flex size-12 shrink-0 items-center justify-center rounded-lg [&>svg]:size-6">
        <Globe />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="text-sm font-medium">Web preview</span>
        <span title={url} className="text-muted-foreground truncate text-sm">{address}</span>
      </span>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm" className="relative">
            Open in
            <ChevronDown />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {inApp && <DropdownMenuItem onSelect={inBrowser}>Brigadier browser</DropdownMenuItem>}
          <DropdownMenuItem onSelect={inDefault}>Default browser</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
