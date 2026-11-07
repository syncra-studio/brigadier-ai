import { SettingsSelect } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import { useProfiles } from "@/state/browserProfilePlaces";

/** Which profile's data a Settings page shows: history, downloads and clearing are each profile's own. */
export function ProfilePicker({ value, onChange }: { value: string; onChange: (profile: string) => void }) {
  const profiles = useProfiles();
  return (
    <SettingsCard>
      <SettingsRow label="Profile">
        <SettingsSelect
          label="Profile"
          value={value}
          onChange={onChange}
          options={profiles.map((each) => ({ value: each.id, label: each.name }))}
        />
      </SettingsRow>
    </SettingsCard>
  );
}
