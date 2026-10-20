import { type ReactNode, useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { Button, Checkbox, Segmented } from "@/components/controls";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays";
import { Download, Globe, Images, SettingsCog, Trash } from "@/components/icons";
import { SettingsCard } from "@/components/shared";
import type { BrowserClear } from "@/ipc/generated";
import { clearBrowsingData, type ClearRange, rangeStart, siteDataCount } from "@/state/browserData";

const RANGES = [
  { value: "hour", label: "Last hour" },
  { value: "day", label: "Last 24 hours" },
  { value: "week", label: "Last 7 days" },
  { value: "month", label: "Last 4 weeks" },
  { value: "all", label: "All time" },
] as const satisfies readonly { value: ClearRange; label: string }[];

type Kind = "history" | "cookies" | "cache" | "downloads" | "sites";

/** Picked when the dialog opens: everything but the sites' camera and microphone choices. */
const PICKED: Record<Kind, boolean> = { history: true, cookies: true, cache: true, downloads: true, sites: false };

function sitesLine(count: number | null | undefined): string {
  if (count === undefined || count === null) return "Signs you out of most sites.";
  if (count === 0) return "No site keeps any now.";
  return `Kept for ${count} ${count === 1 ? "site" : "sites"} now. Signs you out of most of them.`;
}

/**
 * Clear browsing data: a time range and what to remove from Brigadier's browser. Cookies and the
 * cache are WebKit's, so they show only where the browser can clear them (`siteData`).
 */
export function ClearBrowsingDataDialog({
  open,
  onOpenChange,
  siteData,
  onCleared,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  siteData: boolean;
  onCleared?: () => void;
}) {
  const [clearing, setClearing] = useState(false);
  return (
    <Dialog open={open} onOpenChange={(next) => !clearing && onOpenChange(next)}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>Clear browsing data</DialogTitle>
          <DialogDescription className="sr-only">
            Choose a time range and what to remove from Brigadier's browser.
          </DialogDescription>
        </DialogHeader>
        {/* Made each time it opens, so it starts from the usual picks. */}
        {open && (
          <ClearForm
            siteData={siteData}
            clearing={clearing}
            setClearing={setClearing}
            onDone={() => {
              onCleared?.();
              onOpenChange(false);
            }}
            onCancel={() => onOpenChange(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

function ClearForm({
  siteData,
  clearing,
  setClearing,
  onDone,
  onCancel,
}: {
  siteData: boolean;
  clearing: boolean;
  setClearing: (clearing: boolean) => void;
  onDone: () => void;
  onCancel: () => void;
}) {
  const [range, setRange] = useState<ClearRange>("hour");
  const [picked, setPicked] = useState(PICKED);
  const [sites, setSites] = useState<number | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (siteData) siteDataCount().then(setSites, () => setSites(null));
  }, [siteData]);

  const kinds: { kind: Kind; icon: ReactNode; label: string; detail: string }[] = [
    { kind: "history", icon: <Globe />, label: "Browsing history", detail: "The pages you visited." },
    ...(siteData
      ? [
          { kind: "cookies" as const, icon: <Trash />, label: "Cookies and site data", detail: sitesLine(sites) },
          { kind: "cache" as const, icon: <Images />, label: "Cached images and files", detail: "Pages may load more slowly the next time." },
        ]
      : []),
    { kind: "downloads", icon: <Download />, label: "Download history", detail: "The list only; downloaded files stay." },
    { kind: "sites", icon: <SettingsCog />, label: "Site settings", detail: "Which sites may use your camera and microphone." },
  ];
  const any = kinds.some(({ kind }) => picked[kind]);

  const clear = () => {
    const request: BrowserClear = {
      sinceMs: rangeStart(range, Date.now()),
      history: picked.history,
      cookies: siteData && picked.cookies,
      cache: siteData && picked.cache,
      downloads: picked.downloads,
      sites: picked.sites,
    };
    setClearing(true);
    setError(null);
    clearBrowsingData(request)
      .then(() => {
        setClearing(false);
        onDone();
      })
      .catch((cause: unknown) => {
        setClearing(false);
        setError(errorText(cause));
      });
  };

  return (
    <>
      <Segmented label="Time range" value={range} options={RANGES} onChange={setRange} />
      <SettingsCard>
        {kinds.map(({ kind, icon, label, detail }) => (
          <label key={kind} className="flex min-h-14 cursor-pointer items-center gap-3 py-2">
            <span aria-hidden className="text-foreground/80 [&_svg]:size-icon-md flex shrink-0">
              {icon}
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span className="text-label">{label}</span>
              <span className="text-foreground/65 text-xs">{detail}</span>
            </span>
            <Checkbox
              checked={picked[kind]}
              onCheckedChange={(checked) => setPicked((shown) => ({ ...shown, [kind]: checked === true }))}
            />
          </label>
        ))}
      </SettingsCard>
      <ErrorLine error={error} />
      <DialogFooter>
        <Button type="button" variant="ghost" disabled={clearing} onClick={onCancel}>
          Cancel
        </Button>
        <Button type="button" disabled={clearing || !any} onClick={clear}>
          {clearing ? "Deleting…" : "Delete data"}
        </Button>
      </DialogFooter>
    </>
  );
}
