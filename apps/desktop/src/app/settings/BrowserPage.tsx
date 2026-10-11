import { Globe } from "@openai/apps-sdk-ui/components/Icon";

import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { BrowserDownloads } from "@/app/settings/BrowserDownloads";
import { BrowserHistory } from "@/app/settings/BrowserHistory";
import { BrowserImportDialog } from "@/app/settings/BrowserImportDialog";
import { BrowserSiteSettings } from "@/app/settings/BrowserSiteSettings";
import { ClearBrowsingDataDialog } from "@/app/settings/ClearBrowsingDataDialog";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
  SwitchSetting,
} from "@/app/settings/parts";
import { pickFolder } from "@/ipc/client";
import type { LinkTarget } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The Browser page's rows, for the page and for Settings search. */
export const BROWSER_ROWS = {
  browser: {
    label: "Browser",
    description: "Use Brigadier's built-in browser for web pages, in a tab beside your session.",
  },
  webLinks: {
    label: "Open web links in",
    description: "Where links in answers, search results and pull requests open.",
  },
  localLinks: {
    label: "Open local dev URLs in",
    description: "Where localhost and other local development sites open, such as a running preview.",
  },
  fullUrl: {
    label: "Show full URL",
    description: "Include the path, query, and fragment in the address bar.",
  },
  data: {
    label: "Browsing data",
    description: "Clear the built-in browser's history, cookies, site data, cache and download history.",
  },
  history: {
    label: "Browsing history",
    description: "View and remove pages you visited in the built-in browser.",
  },
  location: {
    label: "Location",
    description: "Where files you download in the built-in browser are saved.",
  },
  askDownloads: {
    label: "Ask where to save downloads",
    description: "Choose the folder and name for each download.",
  },
  downloads: {
    label: "Download history",
    description: "View the files you downloaded in the built-in browser.",
  },
  import: {
    label: "Import from your browser",
    description: "Bring over cookies and browsing history from Safari, Chrome, Arc, Firefox and other browsers.",
  },
  sites: {
    label: "Site settings",
    description: "Which sites may use your camera and microphone in the built-in browser.",
  },
} as const;

const PAGE_DESCRIPTION = "The built-in browser, where links open, and what it keeps.";
const OFF_NOTE = "The built-in browser is off: links open in your default browser, and agents are told it isn't there.";

const TARGETS = [
  { value: "brigadier", label: "Brigadier's browser" },
  { value: "system", label: "Your default browser" },
] as const satisfies readonly { value: LinkTarget; label: string }[];

/** Settings › Browser. Off, only the switch shows: the browser doesn't exist anywhere else. */
export function BrowserPage() {
  const on = useApp((s) => s.settings.browserEnabled);
  const embedded = useApp((s) => s.info?.platform !== "linux");
  const mac = useApp((s) => s.info?.platform === "macos");
  const [view, setView] = useState<"page" | "history" | "downloads" | "sites">("page");
  const [clearing, setClearing] = useState(false);
  const [cleared, setCleared] = useState(0);
  const [importing, setImporting] = useState(false);
  const clearDialog = (
    <ClearBrowsingDataDialog
      open={clearing}
      onOpenChange={setClearing}
      siteData={mac}
      onCleared={() => setCleared((count) => count + 1)}
    />
  );
  if (on && mac && view === "sites") return <BrowserSiteSettings onBack={() => setView("page")} />;
  if (on && embedded && view === "downloads") return <BrowserDownloads onBack={() => setView("page")} />;
  if (on && embedded && view === "history") {
    return (
      <>
        <BrowserHistory onBack={() => setView("page")} onClear={() => setClearing(true)} cleared={cleared} />
        {clearDialog}
      </>
    );
  }
  return (
    <SettingsPage title="Browser" description={PAGE_DESCRIPTION}>
      <SettingsSection>
        <SettingsCard>
          <BrowserSwitch on={on} />
        </SettingsCard>
        {!on && <p className="text-foreground/65 px-1 text-xs">{OFF_NOTE}</p>}
      </SettingsSection>
      {on && embedded && (
        <SettingsSection
          title="General"
          actions={mac && <SettingsButton onClick={() => setImporting(true)}>Import…</SettingsButton>}
        >
          <SettingsCard>
            <LinkRow setting="webLinksIn" row={BROWSER_ROWS.webLinks} />
            <LinkRow setting="localLinksIn" row={BROWSER_ROWS.localLinks} />
            <SwitchSetting setting="browserFullUrl" row={BROWSER_ROWS.fullUrl} />
            <SettingsRow label={BROWSER_ROWS.data.label} description={BROWSER_ROWS.data.description}>
              <SettingsButton onClick={() => setClearing(true)}>Clear browsing data</SettingsButton>
            </SettingsRow>
            <SettingsRow label={BROWSER_ROWS.history.label} description={BROWSER_ROWS.history.description}>
              <SettingsButton onClick={() => setView("history")}>Manage</SettingsButton>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {on && embedded && (
        <SettingsSection title="Downloads">
          <SettingsCard>
            <LocationRow />
            {mac && <SwitchSetting setting="browserAskDownloads" row={BROWSER_ROWS.askDownloads} />}
            <SettingsRow label={BROWSER_ROWS.downloads.label} description={BROWSER_ROWS.downloads.description}>
              <SettingsButton onClick={() => setView("downloads")}>Manage</SettingsButton>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {on && mac && (
        <SettingsSection title="Browser permissions">
          <SettingsCard>
            <SettingsRow label={BROWSER_ROWS.sites.label} description={BROWSER_ROWS.sites.description}>
              <SettingsButton onClick={() => setView("sites")}>Manage</SettingsButton>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {clearDialog}
      {mac && <BrowserImportDialog open={importing} onOpenChange={setImporting} />}
    </SettingsPage>
  );
}

/** Downloads' folder: the system's Downloads folder until another is chosen. */
function LocationRow() {
  const dir = useApp((s) => s.settings.browserDownloadsDir);
  const save = useAction();
  const change = () =>
    save.run(async () => {
      const picked = await pickFolder(dir || undefined);
      if (picked) await setSetting("browserDownloadsDir", picked);
    });
  return (
    <SettingsRow
      label={BROWSER_ROWS.location.label}
      description={dir ? <span title={dir}>{shortPath(dir)}</span> : "Your Downloads folder"}
      error={save.error}
    >
      <div className="flex items-center gap-2">
        {dir && (
          <SettingsButton onClick={() => save.run(() => setSetting("browserDownloadsDir", ""))}>
            Use Downloads
          </SettingsButton>
        )}
        <SettingsButton onClick={change}>Change…</SettingsButton>
      </div>
    </SettingsRow>
  );
}

function BrowserSwitch({ on }: { on: boolean }) {
  const save = useAction();
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-3">
          <Globe aria-hidden className="size-icon-lg text-foreground/80 shrink-0" />
          {BROWSER_ROWS.browser.label}
        </span>
      }
      description={<span className="ps-9 block">{BROWSER_ROWS.browser.description}</span>}
      error={save.error}
      className="py-4"
    >
      <SettingsSwitch
        label={BROWSER_ROWS.browser.label}
        checked={on}
        onCheckedChange={(next) => save.run(() => setSetting("browserEnabled", next))}
      />
    </SettingsRow>
  );
}

function LinkRow({
  setting,
  row,
}: {
  setting: "webLinksIn" | "localLinksIn";
  row: { label: string; description: string };
}) {
  const value = useApp((s) => s.settings[setting]);
  const save = useAction();
  return (
    <SettingsRow label={row.label} description={row.description} error={save.error}>
      <SettingsSelect<LinkTarget>
        label={row.label}
        value={value}
        options={TARGETS}
        onChange={(next) => save.run(() => setSetting(setting, next))}
      />
    </SettingsRow>
  );
}
