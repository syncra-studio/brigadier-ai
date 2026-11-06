import { useEffect, useState } from "react";

import { ProfileClearDialog } from "@/app/settings/ProfileClearDialog";
import { ProfileDeleteDialog } from "@/app/settings/ProfileDeleteDialog";
import { ProfileDisc, ProfileSwatches } from "@/app/settings/ProfileMark";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button, Input, Switch } from "@/components/controls";
import { SettingsCard, SettingsRow } from "@/components/shared";
import { select } from "@/state/actions";
import { counted } from "@/state/browserImport";
import {
  canDelete,
  changeProfile,
  makeDefaultProfile,
  profileCounts,
  type ProfileCounts,
  sessionsOn,
  useDefaultProfile,
  useProfilePlaces,
  useProfiles,
} from "@/state/browserProfiles";
import { useApp } from "@/state/store";

/** Settings › Browser › one profile: its name and colour, what it holds, its site permissions, delete. */
export function ProfileDetailPage({ id, onBack }: { id: string; onBack: () => void }) {
  const profiles = useProfiles();
  const fallback = useDefaultProfile();
  const places = useProfilePlaces((s) => s.sessions);
  const conversations = useApp((s) => s.conversations);
  const profile = profiles.find((each) => each.id === id);
  const [counts, setCounts] = useState<ProfileCounts | null>(null);
  const [read, setRead] = useState(0);
  const [clearing, setClearing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [name, setName] = useState(profile?.name ?? "");
  useEffect(() => {
    let current = true;
    profileCounts(id).then(
      (found) => current && setCounts(found),
      () => current && setCounts(null),
    );
    return () => {
      current = false;
    };
    // `read` reads the counts again after a clear.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [id, read]);
  useEffect(() => {
    if (!profile) onBack();
  }, [profile, onBack]);
  if (!profile) return null;
  const isDefault = fallback.id === id;
  const only = !canDelete(id, profiles, fallback.id);
  const sessions = sessionsOn(id, places, conversations);
  const on = sessions === 0 ? "No sessions are on it." : sessions === 1 ? "1 session is on it." : `${sessions} sessions are on it.`;
  const saveName = () => {
    const trimmed = name.trim();
    const taken = profiles.some((each) => each.id !== id && each.name.toLowerCase() === trimmed.toLowerCase());
    if (!trimmed || taken) setName(profile.name);
    else if (trimmed !== profile.name) changeProfile(id, { name: trimmed });
  };
  return (
    <SettingsPage
      title={profile.name}
      mark={<ProfileDisc profile={profile} size="xl" />}
      description={`${isDefault ? "The default profile. " : ""}${on}`}
      back={{ label: "Browser", onBack }}
    >
      <SettingsSection>
        <SettingsCard>
          <SettingsRow label="Name">
            <Input
              aria-label="Name"
              value={name}
              maxLength={40}
              className="h-7 w-52"
              onChange={(event) => setName(event.target.value)}
              onBlur={saveName}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
            />
          </SettingsRow>
          <SettingsRow label="Colour"><ProfileSwatches value={profile.hue} onChange={(hue) => changeProfile(id, { hue })} /></SettingsRow>
          <SettingsRow label="Default for new sessions" description="One profile is always the default. To change it, turn this on for another profile.">
            <Switch checked={isDefault} disabled={isDefault} aria-label="Default for new sessions" onCheckedChange={(checked) => checked && makeDefaultProfile(id)} />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
      <SettingsSection title="Data in this profile">
        <SettingsCard>
          <SettingsRow
            label="Sign-ins"
            description={counts
              ? `${counted(counts.signIns, "site", "sites")} with cookies, not a count of accounts.`
              : undefined}
          />
          <SettingsRow label="Browsing history" description="The pages this profile visited.">
            <Button
              variant="soft"
              size="sm"
              aria-label="Manage browsing history"
              onClick={() => void select({ type: "settings", page: "browser", detail: { view: "history", profile: id } })}
            >
              Manage
            </Button>
          </SettingsRow>
          <SettingsRow label="Download history" description="The files this profile downloaded.">
            <Button
              variant="soft"
              size="sm"
              aria-label="Manage download history"
              onClick={() => void select({ type: "settings", page: "browser", detail: { view: "downloads", profile: id } })}
            >
              Manage
            </Button>
          </SettingsRow>
          <SettingsRow label="Bookmarks" />
          <SettingsRow label="Saved passwords" description="Agents never see these." />
          <SettingsRow label="Site settings" description="Which sites may use your camera and microphone in this profile.">
            <Button
              variant="soft"
              size="sm"
              aria-label="Manage site settings"
              onClick={() => void select({ type: "settings", page: "browser", detail: { view: "sites", profile: id } })}
            >
              Manage
            </Button>
          </SettingsRow>
          <SettingsRow label="Browsing data" description="Clear this profile's history, sign-ins, site data, cache and download history, by time range. Saved passwords are kept, and other profiles aren't touched.">
            <Button variant="soft" size="sm" onClick={() => setClearing(true)}>Clear browsing data</Button>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
      <SettingsSection>
        <SettingsCard>
          <SettingsRow
            label="Delete this profile"
            description={only
              ? `The ${profiles.length === 1 ? "last" : "default"} profile can't be deleted. You can clear its data instead.`
              : "Removes it and its data from Brigadier."}
          >
            <Button variant="soft-destructive" size="sm" disabled={only} onClick={() => setDeleting(true)}>Delete profile…</Button>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
      <ProfileClearDialog profile={profile} open={clearing} onOpenChange={setClearing} onCleared={() => setRead((times) => times + 1)} />
      <ProfileDeleteDialog profile={profile} open={deleting} onOpenChange={setDeleting} onDeleted={onBack} />
    </SettingsPage>
  );
}
