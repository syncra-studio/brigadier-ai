import { useState } from "react";

import { BrowserImportDialog } from "@/app/settings/BrowserImportDialog";
import { SettingsButton, SettingsCard, SettingsPage, SettingsRow, SettingsSection } from "@/app/settings/parts";
import { openSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/** The Import Browser Data page's rows, for the page and for Settings search. */
export const BROWSER_IMPORT_ROWS = {
  import: {
    label: "Import from your browser",
    description: "Bring over cookies and browsing history from Safari, Chrome, Arc, Firefox and other browsers.",
  },
} as const;

const PAGE_DESCRIPTION = "Bring what another browser keeps into Brigadier's built-in browser.";

/**
 * Settings › Import Browser Data: Import… copies another browser's cookies and history into the
 * built-in browser, once. While the browser is off there is nothing to import into, so the
 * button waits, with a line that says why and leads to Settings › Browser.
 */
export function BrowserImportPage() {
  const on = useApp((s) => s.settings.browserEnabled);
  const [importing, setImporting] = useState(false);
  return (
    <SettingsPage title="Import Browser Data" description={PAGE_DESCRIPTION}>
      <SettingsSection>
        <SettingsCard>
          <SettingsRow label={BROWSER_IMPORT_ROWS.import.label} description={BROWSER_IMPORT_ROWS.import.description}>
            <SettingsButton disabled={!on} onClick={() => setImporting(true)}>
              Import…
            </SettingsButton>
          </SettingsRow>
        </SettingsCard>
        {!on && (
          <p data-slot="browser-off" className="text-foreground/65 px-1 text-xs">
            The built-in browser is off, so there is nothing to import into. Turn it on in{" "}
            <button type="button" className="text-link hover:underline" onClick={() => openSettings("browser")}>
              Settings › Browser
            </button>
            .
          </p>
        )}
      </SettingsSection>
      {on && <BrowserImportDialog open={importing} onOpenChange={setImporting} />}
    </SettingsPage>
  );
}
