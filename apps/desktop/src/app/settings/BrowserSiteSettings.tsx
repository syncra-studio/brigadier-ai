import { useEffect, useState } from "react";

import { ErrorLine, errorText } from "@/app/dialogs/fields";
import { SettingsCard, SettingsPage, SettingsSection, SettingsSelect } from "@/app/settings/parts";
import { Camera, Mic } from "@/components/icons";
import type { BrowserSite, SitePermission } from "@/ipc/generated";
import { browserSites, setSite } from "@/state/browserData";

const CHOICES = [
  { value: "ask", label: "Ask" },
  { value: "allow", label: "Allow" },
  { value: "block", label: "Block" },
] as const satisfies readonly { value: SitePermission; label: string }[];

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
            <p className="text-foreground/65 text-label py-4">
              Sites that ask for your camera or microphone show here. Until you choose, each asks you first.
            </p>
          </SettingsCard>
        ) : (
          <SettingsCard>
            <ul className="divide-divider divide-y">
              {(sites ?? []).map((site) => (
                <li key={site.origin} className="flex min-h-14 flex-wrap items-center gap-x-4 gap-y-2 py-2">
                  <span className="text-label min-w-0 flex-1 truncate" title={site.origin}>
                    {site.origin.replace(/^https:\/\//, "")}
                  </span>
                  <span className="flex items-center gap-2">
                    <Camera aria-hidden className="text-foreground/65 size-icon-sm" />
                    <SettingsSelect
                      label={`Camera for ${site.origin}`}
                      value={site.camera}
                      options={CHOICES}
                      onChange={(choice) => choose(site.origin, "camera", choice)}
                    />
                  </span>
                  <span className="flex items-center gap-2">
                    <Mic aria-hidden className="text-foreground/65 size-icon-sm" />
                    <SettingsSelect
                      label={`Microphone for ${site.origin}`}
                      value={site.microphone}
                      options={CHOICES}
                      onChange={(choice) => choose(site.origin, "microphone", choice)}
                    />
                  </span>
                </li>
              ))}
            </ul>
          </SettingsCard>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}
