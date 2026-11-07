import { useEffect, useState } from "react";

import { ErrorLine, SettingsSelect } from "@/components/controls";
import { errorText } from "@/app/shell/dialogs/fields";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { SiteRow } from "@/app/settings/SiteRow";
import { EmptyState, SettingsCard, SettingsRow } from "@/components/shared";
import type { BrowserSite, SitePermission } from "@/ipc/generated";
import { browserSites, setSite } from "@/state/browserData";
import { useDefaultProfile, useProfiles } from "@/state/browserProfilePlaces";

/**
 * Settings › Browser › Site settings: every site that asked for the camera, microphone or location in a
 * profile, and what each may use there. Another profile's choices for the same site are its own.
 */
export function BrowserSiteSettings({ profile: opened, onBack }: { profile?: string | undefined; onBack: () => void }) {
  const profiles = useProfiles();
  const fallback = useDefaultProfile();
  const [profile, setProfile] = useState(opened ?? fallback.id);
  const [sites, setSites] = useState<BrowserSite[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    browserSites(profile).then(
      (found) => current && setSites(found),
      (cause: unknown) => current && setError(errorText(cause)),
    );
    return () => {
      current = false;
    };
  }, [profile]);

  const choose = (origin: string, device: "camera" | "microphone" | "location", choice: SitePermission) => {
    setError(null);
    setSite(profile, origin, { [device]: choice })
      .then(() =>
        setSites((shown) => shown?.map((site) => (site.origin === origin ? { ...site, [device]: choice } : site)) ?? null),
      )
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  return (
    <SettingsPage
      title="Site settings"
      description="Which sites may use your camera, microphone and location, for each profile."
      back={{ label: "Browser", onBack }}
    >
      <SettingsSection>
        <SettingsCard>
          <SettingsRow label="Profile">
            <SettingsSelect
              label="Profile"
              value={profile}
              onChange={setProfile}
              options={profiles.map((each) => ({ value: each.id, label: each.name }))}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Camera, microphone and location">
        <ErrorLine error={error} />
        {sites && sites.length === 0 ? (
          <SettingsCard>
            <EmptyState
              variant="card"
              title="Sites that ask for your camera, microphone or location show here. Until you choose, each asks you first."
            />
          </SettingsCard>
        ) : (
          <SettingsCard inset>
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
