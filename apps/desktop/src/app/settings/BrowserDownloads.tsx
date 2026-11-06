import { useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { DownloadRow } from "@/app/settings/DownloadRow";
import { ProfilePicker } from "@/app/settings/ProfilePicker";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button, ErrorLine } from "@/components/controls";
import { Download } from "@/components/icons";
import { EmptyState, SettingsCard, SettingsSearch } from "@/components/shared";
import { useNow } from "@/hooks/use-now";
import { browserDownloads, byDay, cancelDownload, type DownloadRecord, removeDownloads } from "@/state/browserData";
import { useBrowsers } from "@/state/browsers";
import { useDefaultProfile } from "@/state/browserProfilePlaces";
import { useApp } from "@/state/store";

/** Settings › Browser › Download history: the files the browser downloaded, by day. */
export function BrowserDownloads({ profile: opened, onBack }: { profile?: string | undefined; onBack: () => void }) {
  const fallback = useDefaultProfile();
  const [profile, setProfile] = useState(opened ?? fallback.id);
  const told = useBrowsers((s) => s.downloads);
  const [query, setQuery] = useState("");
  const [downloads, setDownloads] = useState<DownloadRecord[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [changed, setChanged] = useState(0);
  const mac = useApp((s) => s.info?.platform === "macos");
  const now = useNow(60_000);

  useEffect(() => {
    let current = true;
    const timer = setTimeout(() => {
      browserDownloads(profile, query)
        .then((found) => current && setDownloads(found))
        .catch((cause: unknown) => current && setError(errorText(cause)));
    }, query ? 150 : 0);
    return () => {
      current = false;
      clearTimeout(timer);
    };
    // `changed` reads the list again after a removal, `told` as the shell says a download moved.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [profile, query, changed, told]);

  const remove = (ids: number[] | null) => {
    removeDownloads(profile, ids)
      .then(() => setChanged((count) => count + 1))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const shown = downloads ?? [];
  const days = byDay(shown, now);
  return (
    <SettingsPage title="Download history" back={{ label: "Browser", onBack }}>
      <ProfilePicker value={profile} onChange={setProfile} />
      <SettingsSearch label="Search download history" value={query} onChange={setQuery} />
      <SettingsSection
        title="All-time history"
        actions={
          <Button variant="soft" size="sm" disabled={!shown.some((d) => d.state !== "running")} onClick={() => remove(null)}>
            Clear all
          </Button>
        }
      >
        <ErrorLine error={error} />
        {downloads && days.length === 0 && (
          <SettingsCard>
            <EmptyState
              variant="centered"
              icon={Download}
              title={query ? "No downloads match" : "No downloads yet"}
              detail={!query && "Files you download in the built-in browser show here."}
            />
          </SettingsCard>
        )}
        {days.map((day) => (
          <SettingsSection key={day.key} title={day.label}>
            <SettingsCard inset>
              <ul className="divide-divider divide-y">
                {day.items.map((download) => (
                  <DownloadRow
                    key={download.id}
                    download={download}
                    mac={mac}
                    onRemove={() => remove([download.id])}
                    onCancel={() => void cancelDownload(profile, download.id).catch((cause: unknown) => setError(errorText(cause)))}
                    onError={setError}
                  />
                ))}
              </ul>
            </SettingsCard>
          </SettingsSection>
        ))}
      </SettingsSection>
    </SettingsPage>
  );
}
