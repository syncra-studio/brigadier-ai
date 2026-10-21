import { useState } from "react";

import { BrowserDownloads } from "@/app/settings/BrowserDownloads";
import { BrowserHistory } from "@/app/settings/BrowserHistory";
import { BrowserOffNote } from "@/app/settings/BrowserOffNote";
import { BrowserSiteSettings } from "@/app/settings/BrowserSiteSettings";
import { BrowserSwitch } from "@/app/settings/BrowserSwitch";
import { ClearBrowsingDataDialog } from "@/app/settings/ClearBrowsingDataDialog";
import { LinkRow } from "@/app/settings/LinkRow";
import { LocationRow } from "@/app/settings/LocationRow";
import { SettingsPage, SettingsSection, SwitchSetting } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import {
  setClearingBrowsingData as setClearing,
  showBrowserSettingsView as setView,
  useBrowserSettingsView,
} from "@/state/browserSettings";
import { useApp } from "@/state/store";

/** The Browser page's rows, for the page and for Settings search. */
export const BROWSER_ROWS = {
  browser: {
    label: "Browser",
    description: "Use Brigadier's built-in browser for web pages, in a tab beside your session",
  },
  webLinks: {
    label: "Open web links in",
    description: "Where links in answers, search results and pull requests open",
  },
  localLinks: {
    label: "Open local dev URLs in",
    description: "Where localhost and other local development sites open, such as a running preview",
  },
  fullUrl: {
    label: "Show full URL",
    description: "Include the path, query and fragment in the address bar",
  },
  data: {
    label: "Browsing data",
    description: "Clear the built-in browser's history, cookies, site data, cache and download history",
  },
  history: {
    label: "Browsing history",
    description: "View and remove pages you visited in the built-in browser",
  },
  location: {
    label: "Location",
    description: "Where files you download in the built-in browser are saved",
  },
  askDownloads: {
    label: "Ask where to save downloads",
    description: "Choose the folder and name for each download",
  },
  downloads: {
    label: "Download history",
    description: "View the files you downloaded in the built-in browser",
  },
  sites: {
    label: "Site settings",
    description: "Which sites may use your camera and microphone in the built-in browser",
  },
} as const;

const PAGE_DESCRIPTION = "The built-in browser, where links open, and what it keeps.";
const OFF_NOTE = "The built-in browser is off: links open in your default browser, and agents are told it isn't there.";

/** Settings › Browser. Off, only the switch shows: the browser doesn't exist anywhere else. */
export function BrowserPage() {
  const on = useApp((s) => s.settings.browserEnabled);
  const embedded = useApp((s) => s.info?.platform !== "linux");
  const mac = useApp((s) => s.info?.platform === "macos");
  const shown = useApp((s) => s.selection);
  // A sub-view or the Clear dialog lasts while Settings shows what it was opened under (see
  // state/browserSettings.ts): Browser picked again in the sidebar opens the page itself.
  const opened = useBrowserSettingsView((s) => s.opened);
  const clearingUnder = useBrowserSettingsView((s) => s.clearing);
  const view = opened?.under === shown ? opened.view : "page";
  const clearing = clearingUnder !== null && clearingUnder === shown;
  const [cleared, setCleared] = useState(0);
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
          <BrowserSwitch on={on} row={BROWSER_ROWS.browser} />
        </SettingsCard>
        {!on && <BrowserOffNote>{OFF_NOTE}</BrowserOffNote>}
      </SettingsSection>
      {on && embedded && (
        <SettingsSection title="General">
          <SettingsCard>
            <LinkRow setting="webLinksIn" row={BROWSER_ROWS.webLinks} />
            <LinkRow setting="localLinksIn" row={BROWSER_ROWS.localLinks} />
            <SwitchSetting setting="browserFullUrl" row={BROWSER_ROWS.fullUrl} />
            <SettingsRow label={BROWSER_ROWS.data.label} description={BROWSER_ROWS.data.description}>
              <Button variant="soft" size="sm" onClick={() => setClearing(true)}>Clear browsing data</Button>
            </SettingsRow>
            <SettingsRow label={BROWSER_ROWS.history.label} description={BROWSER_ROWS.history.description}>
              <Button variant="soft" size="sm" onClick={() => setView("history")}>Manage</Button>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {on && embedded && (
        <SettingsSection title="Downloads">
          <SettingsCard>
            <LocationRow row={BROWSER_ROWS.location} />
            {mac && <SwitchSetting setting="browserAskDownloads" row={BROWSER_ROWS.askDownloads} />}
            <SettingsRow label={BROWSER_ROWS.downloads.label} description={BROWSER_ROWS.downloads.description}>
              <Button variant="soft" size="sm" onClick={() => setView("downloads")}>Manage</Button>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {on && mac && (
        <SettingsSection title="Browser permissions">
          <SettingsCard>
            <SettingsRow label={BROWSER_ROWS.sites.label} description={BROWSER_ROWS.sites.description}>
              <Button variant="soft" size="sm" onClick={() => setView("sites")}>Manage</Button>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      )}
      {clearDialog}
    </SettingsPage>
  );
}
