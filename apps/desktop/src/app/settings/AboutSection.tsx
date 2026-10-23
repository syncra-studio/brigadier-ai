import { useState } from "react";

import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { LicensesDialog } from "@/app/settings/LicensesDialog";
import { SettingsSection } from "@/app/settings/parts";
import { VersionValue } from "@/app/settings/VersionValue";
import { Button } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import { checkForUpdates } from "@/ipc/client";
import { useAppUpdate } from "@/state/appUpdate";
import { reopenOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";

/** Settings › About's "Check now": checked just now, and what it found (if it finished), or that it couldn't. */
type CheckState = { checking: boolean; found: boolean; failed?: boolean };

/** General › About: the version, the build, updates, the licenses, and running setup again. */
export function AboutSection() {
  const version = useApp((s) => s.info?.version);
  const build = useApp((s) => s.info?.build);
  const channel = useApp((s) => s.info?.updateChannel);
  const [licenses, setLicenses] = useState(false);
  const [checked, setChecked] = useState<CheckState | null>(null);
  const checkNow = () => {
    setChecked({ checking: true, found: false });
    void checkForUpdates().then(
      (update) => {
        if (update) useAppUpdate.setState({ update });
        setChecked({ checking: false, found: update !== null });
      },
      // Offline, or the feed didn't answer: say so, and Check now works again.
      () => setChecked({ checking: false, found: false, failed: true }),
    );
  };
  return (
    <SettingsSection title="About">
      <SettingsCard>
        <SettingsRow label={GENERAL_ROWS.version.label} description={GENERAL_ROWS.version.description}>
          <VersionValue version={version} />
        </SettingsRow>
        {(build?.sha || build?.date) && (
          <SettingsRow label={GENERAL_ROWS.build.label} description={GENERAL_ROWS.build.description}>
            <span className="text-muted-foreground text-label tabular-nums">
              {[build?.date, build?.sha].filter(Boolean).join(" · ")}
            </span>
          </SettingsRow>
        )}
        {channel && (
          <SettingsRow
            label={GENERAL_ROWS.updates.label}
            description={
              checked?.checking
                ? "Checking…"
                : checked?.failed
                  ? "Couldn't check for updates. Try again later."
                  : checked
                  ? checked.found
                    ? "An update was found."
                    : "Up to date."
                  : `${channel} channel`
            }
          >
            <Button variant="soft" size="sm" onClick={checkNow} disabled={checked?.checking}>
              Check now
            </Button>
          </SettingsRow>
        )}
        <SettingsRow label={GENERAL_ROWS.licenses.label} description={GENERAL_ROWS.licenses.description}>
          <Button variant="soft" size="sm" onClick={() => setLicenses(true)}>View</Button>
        </SettingsRow>
        <SettingsRow label={GENERAL_ROWS.setup.label} description={GENERAL_ROWS.setup.description}>
          <Button variant="soft" size="sm" onClick={() => reopenOnboarding()}>Run setup</Button>
        </SettingsRow>
      </SettingsCard>
      <LicensesDialog open={licenses} onOpenChange={setLicenses} />
    </SettingsSection>
  );
}
