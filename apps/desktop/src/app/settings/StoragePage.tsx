import { useEffect, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import { formatBytes } from "@/lib/format";
import { openStorage, openUninstall, scanStorage } from "@/state/storage";

/** The Storage page's rows, for the page and for Settings search. */
export const STORAGE_ROWS = {
  storage: {
    label: "Storage",
    description: "What Brigadier keeps on this computer, and what of it can be cleaned.",
  },
  uninstall: {
    label: "Uninstall Brigadier",
    description:
      "Removes what Brigadier created on this computer, then the app. Your repositories, your own branches and your files are not touched.",
  },
} as const;

export function StoragePage() {
  const [summary, setSummary] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    scanStorage().then(
      (report) =>
        current &&
        setSummary(
          report.cleanableBytes > 0
            ? `Brigadier uses ${formatBytes(report.totalBytes)} · ${formatBytes(report.cleanableBytes)} can be cleaned.`
            : `Brigadier uses ${formatBytes(report.totalBytes)} · nothing to clean.`,
        ),
      (cause: unknown) => current && setSummary(`Couldn't measure: ${errorText(cause)}`),
    );
    return () => {
      current = false;
    };
  }, []);

  return (
    <SettingsPage title="Storage" wide>
      <SettingsSection>
        <SettingsCard>
          <SettingsRow
            label={STORAGE_ROWS.storage.label}
            description={summary ?? STORAGE_ROWS.storage.description}
          >
            <SettingsButton onClick={() => openStorage()}>Manage…</SettingsButton>
          </SettingsRow>
          <SettingsRow
            label={STORAGE_ROWS.uninstall.label}
            description={STORAGE_ROWS.uninstall.description}
          >
            <SettingsButton destructive onClick={() => openUninstall()}>
              Uninstall…
            </SettingsButton>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}
