import { useState } from "react";

import { SettingsSelect } from "@/components/controls";
import { type BrowserProfile, useDefaultProfile, useProfiles } from "@/state/browserProfilePlaces";

/** The browser's profiles and the one a page is showing: `first`, or the default, until the user picks. */
export function useBrowserProfilePick(first?: string) {
  const profiles = useProfiles();
  const fallback = useDefaultProfile();
  const [picked, setPicked] = useState<string | null>(first ?? null);
  return { profiles, profile: picked ?? fallback.id, setProfile: setPicked };
}

/** Which profile's passwords or bookmarks a page shows. With one profile there is nothing to pick. */
export function BrowserPasswordsProfile({
  profiles,
  profile,
  onChange,
}: {
  profiles: readonly BrowserProfile[];
  profile: string | null;
  onChange: (profile: string) => void;
}) {
  if (profiles.length < 2 || profile === null) return null;
  return (
    <SettingsSelect
      label="Browser profile"
      value={profile}
      options={profiles.map(({ id, name }) => ({ value: id, label: name }))}
      onChange={onChange}
      className="w-36"
    />
  );
}
