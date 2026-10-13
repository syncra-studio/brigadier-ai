import { Camera, Download, Mic, X } from "@openai/apps-sdk-ui/components/Icon";
import { type RefObject, useEffect, useState } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { revealPath } from "@/ipc/client";
import type { BrowserDownload } from "@/ipc/generated";
import { browserDownloads, openDownload, siteOf } from "@/state/browserData";
import { openBrowserSettings } from "@/state/browserSettings";
import { answerPermission, type BrowserAsking, useBrowsers } from "@/state/browsers";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/**
 * The page's question for the camera or microphone, from the address field's start: allow it
 * while the tab stays on the site, this time only, or never (saved for the site). It stays until
 * it is answered or closed (which refuses this time), and leaves the page live around it.
 */
export function PermissionPrompt({
  id,
  asking,
  anchor,
}: {
  id: string;
  asking: BrowserAsking | null | undefined;
  anchor: RefObject<HTMLDivElement | null>;
}) {
  let site = asking?.origin ?? "";
  try {
    site = new URL(site).host;
  } catch {
    // The origin as the shell named it.
  }
  const uses = [
    ...(asking?.camera ? [{ Icon: Camera, text: "Use your camera" }] : []),
    ...(asking?.microphone ? [{ Icon: Mic, text: "Use your microphone" }] : []),
  ];
  return (
    <Popover open={!!asking} onOpenChange={(open) => { if (!open) answerPermission(id, "dismiss"); }}>
      <PopoverAnchor virtualRef={anchor as RefObject<HTMLDivElement>} />
      <PopoverContent
        align="start"
        aria-label={`Website permission request from ${site}`}
        data-slot="browser-permission"
        // Not a menu: the page stays live around it.
        data-above-page
        className="w-browser-site-info p-3"
        onOpenAutoFocus={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()}
      >
        <div className="flex items-start gap-2">
          <p className="text-browser min-w-0 flex-1 pt-1 font-medium break-words">{site} wants to</p>
          <TooltipIconButton tooltip="Close permission request" size="icon-xs" onClick={() => answerPermission(id, "dismiss")}>
            <X />
          </TooltipIconButton>
        </div>
        <ul className="mt-2 flex flex-col gap-1.5">
          {uses.map(({ Icon, text }) => (
            <li key={text} className="text-browser flex items-center gap-2">
              <Icon aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
              {text}
            </li>
          ))}
        </ul>
        <div className="mt-3 flex flex-col gap-1.5">
          <Button size="sm" variant="secondary" onClick={() => answerPermission(id, "visiting")}>Allow while visiting the site</Button>
          <Button size="sm" variant="secondary" onClick={() => answerPermission(id, "once")}>Allow this time</Button>
          <Button size="sm" variant="secondary" onClick={() => answerPermission(id, "never")}>Never allow</Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}

const STATES: Record<BrowserDownload["state"], string | null> = {
  running: "Downloading",
  done: null,
  failed: "Failed",
};

/** How many of the newest downloads the popover lists. */
const NEWEST = 5;

function failed(cause: unknown) {
  toast(cause instanceof Error ? cause.message : String(cause), { tone: "error" });
}

/**
 * The toolbar's Downloads, once the browser has downloaded something: the newest files, each to
 * open or show in its folder, and all of them in Settings.
 */
export function DownloadsButton() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const finished = useBrowsers((s) => s.downloads);
  const [open, setOpen] = useState(false);
  const [downloads, setDownloads] = useState<BrowserDownload[]>([]);
  const [read, setRead] = useState(0);
  useEffect(() => {
    let current = true;
    browserDownloads("").then(
      (found) => current && setDownloads(Array.isArray(found) ? found.slice(0, NEWEST) : []),
      () => {},
    );
    return () => {
      current = false;
    };
    // Read again as a download finishes and as the popover opens.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [finished, read]);
  if (downloads.length === 0) return null;
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) setRead((count) => count + 1);
      }}
    >
      <PopoverTrigger asChild>
        <TooltipIconButton tooltip="Downloads" size="icon-lg" className="shrink-0 rounded-full @max-md/browser-toolbar:hidden">
          <Download />
        </TooltipIconButton>
      </PopoverTrigger>
      <PopoverContent align="end" aria-label="Downloads" data-slot="browser-downloads" className="w-browser-downloads p-1">
        <ul>
          {downloads.map((download) => {
            const name = download.path.split(/[\\/]/).pop() || download.path;
            const state = STATES[download.state];
            return (
              <li key={download.id} className="flex items-center gap-2 rounded-capsule py-1.5 ps-2 pe-1">
                <Download aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="text-browser truncate" title={download.path}>{name}</span>
                  <span className="text-muted-foreground truncate text-xs">
                    {state ? `${state} · ` : ""}
                    {siteOf(download.url)}
                  </span>
                </span>
                {download.state === "done" && (
                  <>
                    <Button size="xs" variant="ghost" onClick={() => openDownload(download.id).catch(failed)}>Open</Button>
                    <Button size="xs" variant="ghost" onClick={() => revealPath(download.path).catch(failed)}>
                      {mac ? "Show in Finder" : "Show in folder"}
                    </Button>
                  </>
                )}
              </li>
            );
          })}
        </ul>
        <div className="bg-border mx-1 my-1 h-px" />
        <button
          type="button"
          onClick={() => {
            setOpen(false);
            openBrowserSettings("downloads");
          }}
          className="hover:bg-foreground/8 text-browser h-browser-menu-item flex w-full items-center rounded-capsule px-2"
        >
          All downloads
        </button>
      </PopoverContent>
    </Popover>
  );
}
