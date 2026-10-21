import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { NameRow } from "@/app/settings/NameRow";
import { SettingsSection } from "@/app/settings/parts";
import { SidebarClosedChoice } from "@/app/settings/SidebarClosedChoice";
import { useSidebar } from "@/components/layout";
import { SettingsCard, SettingsRow } from "@/components/shared";

/** General › Appearance: your name, and what the sidebar leaves when it is closed. */
export function AppearanceSection() {
  const { collapseMode, setCollapseMode } = useSidebar();
  return (
    <SettingsSection title="Appearance">
      <SettingsCard>
        <NameRow />
        <SettingsRow
          label={GENERAL_ROWS.sidebarClosed.label}
          description={GENERAL_ROWS.sidebarClosed.description}
        >
          <SidebarClosedChoice
            label={GENERAL_ROWS.sidebarClosed.label}
            value={collapseMode}
            onChange={setCollapseMode}
          />
        </SettingsRow>
      </SettingsCard>
    </SettingsSection>
  );
}
