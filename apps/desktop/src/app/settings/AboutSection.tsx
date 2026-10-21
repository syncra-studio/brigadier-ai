import { useState } from "react";

import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { LicensesDialog } from "@/app/settings/LicensesDialog";
import { SettingsSection } from "@/app/settings/parts";
import { VersionValue } from "@/app/settings/VersionValue";
import { Button } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import { reopenOnboarding } from "@/state/onboarding";
import { useApp } from "@/state/store";

/** General › About: the version, the licenses, and running setup again. */
export function AboutSection() {
  const version = useApp((s) => s.info?.version);
  const [licenses, setLicenses] = useState(false);
  return (
    <SettingsSection title="About">
      <SettingsCard>
        <SettingsRow label={GENERAL_ROWS.version.label} description={GENERAL_ROWS.version.description}>
          <VersionValue version={version} />
        </SettingsRow>
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
