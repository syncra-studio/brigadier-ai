import { useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/shell/dialogs/fields";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { SiteRow } from "@/app/settings/SiteRow";
import { EmptyState, SettingsCard } from "@/components/shared";
import type { BrowserSite, SitePermission } from "@/ipc/generated";
import { browserSites, setSite } from "@/state/browserData";

/**
 * Settings › Browser › Site settings: every site that asked the built-in browser for the camera
 * or microphone, and what each may use. Ask (the first time) shows the system's own question.
 */
export function BrowserSiteSettings({ onBack }: { onBack: () => void }) {
  const [sites, setSites] = useState<BrowserSite[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    browserSites().then(setSites, (cause: unknown) => setError(errorText(cause)));
  }, []);

  const choose = (origin: string, device: "camera" | "microphone", choice: SitePermission) => {
    setError(null);
    setSite(origin, { [device]: choice })
      .then(() =>
        setSites((shown) => shown?.map((site) => (site.origin === origin ? { ...site, [device]: choice } : site)) ?? null),
      )
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  return (
    <SettingsPage
      title="Site settings"
      description="Which sites may use your camera and microphone in the built-in browser."
      back={{ label: "Browser", onBack }}
    >
      <SettingsSection title="Camera and microphone">
        <ErrorLine error={error} />
        {sites && sites.length === 0 ? (
          <SettingsCard>
            <EmptyState
              variant="card"
              title="Sites that ask for your camera or microphone show here. Until you choose, each asks you first."
            />
          </SettingsCard>
        ) : (
          <SettingsCard>
            <ul className="divide-divider divide-y">
              {(sites ?? []).map((site) => (
                <SiteRow key={site.origin} site={site} onChoose={(device, choice) => choose(site.origin, device, choice)} />
              ))}
            </ul>
          </SettingsCard>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}
