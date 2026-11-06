import { useCallback, useEffect, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { BrowserPasswordsAdd } from "@/app/settings/BrowserPasswordsAdd";
import { BrowserPasswordsProfile, useBrowserProfilePick } from "@/app/settings/BrowserPasswordsProfile";
import { BrowserPasswordsRow } from "@/app/settings/BrowserPasswordsRow";
import { SettingsPage, SettingsSection } from "@/app/settings/parts";
import { Button, ErrorLine } from "@/components/controls";
import { Plus } from "@/components/icons";
import { ConfirmDialog } from "@/components/overlays";
import { EmptyState, SettingsCard, SettingsSearch } from "@/components/shared";
import {
  allowSite,
  findPasswords,
  neverSites,
  passwordSite,
  removePassword,
  type SavedPassword,
  savedPasswords,
} from "@/state/browserPasswords";

/**
 * Settings › Browser › Saved passwords: one profile's passwords, by site. Shows a password only
 * on request, after the system's check; adds and removes; lists the sites that never offer to save.
 */
export function BrowserPasswords({ profile: first, onBack }: { profile?: string | undefined; onBack: () => void }) {
  const { profiles, profile, setProfile } = useBrowserProfilePick(first);
  const [read, setRead] = useState<{ from: string; passwords: SavedPassword[] } | null>(null);
  const [never, setNever] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<SavedPassword | null>(null);
  const [busy, setBusy] = useState(false);

  // What another profile's read left isn't shown: the list is empty until this one's comes.
  const passwords = read?.from === profile ? read.passwords : null;
  const setPasswords = (change: (shown: SavedPassword[]) => SavedPassword[]) =>
    setRead((last) => last && { from: last.from, passwords: change(last.passwords) });

  const load = useCallback((from: string) => {
    savedPasswords(from).then(
      (found) => setRead({ from, passwords: found }),
      (cause: unknown) => setError(errorText(cause)),
    );
    neverSites(from).then(setNever, (cause: unknown) => setError(errorText(cause)));
  }, []);

  useEffect(() => {
    if (profile) load(profile);
  }, [profile, load]);

  const remove = () => {
    if (!profile || !removing) return;
    setBusy(true);
    removePassword(profile, removing.id)
      .then(() => {
        setPasswords((shown) => shown.filter((entry) => entry.id !== removing.id));
        setRemoving(null);
      })
      .catch((cause: unknown) => setError(errorText(cause)))
      .finally(() => setBusy(false));
  };

  const allow = (site: string) => {
    if (!profile) return;
    allowSite(profile, site)
      .then(() => setNever((shown) => shown.filter((other) => other !== site)))
      .catch((cause: unknown) => setError(errorText(cause)));
  };

  const found = findPasswords(passwords ?? [], query);
  return (
    <SettingsPage
      title="Saved passwords"
      description="Passwords Brigadier's browser fills for you. Agents never see them, and a tab an agent is driving doesn't fill them."
      back={{ label: "Browser", onBack }}
    >
      <div className="flex items-center gap-2">
        <SettingsSearch label="Search saved passwords" value={query} onChange={setQuery} />
        <BrowserPasswordsProfile profiles={profiles} profile={profile} onChange={setProfile} />
        <Button variant="soft" disabled={!profile} onClick={() => setAdding(true)}>
          <Plus />
          Add password
        </Button>
      </div>
      <SettingsSection title="Passwords" aside={passwords ? String(passwords.length) : undefined}>
        <ErrorLine error={error} />
        {passwords && found.length === 0 ? (
          <SettingsCard>
            <EmptyState
              variant="card"
              title={query ? "No passwords match." : "Sign in on a site in Brigadier's browser and it offers to save your password here."}
            />
          </SettingsCard>
        ) : (
          <SettingsCard inset>
            <ul className="divide-divider divide-y">
              {profile &&
                found.map((entry) => (
                  <BrowserPasswordsRow
                    key={entry.id}
                    profile={profile}
                    entry={entry}
                    onRemove={() => setRemoving(entry)}
                    onError={setError}
                  />
                ))}
            </ul>
          </SettingsCard>
        )}
      </SettingsSection>
      {never.length > 0 && (
        <SettingsSection title="Never offered on">
          <SettingsCard inset>
            <ul className="divide-divider divide-y">
              {never.map((site) => (
                <li key={site} className="flex min-h-11 items-center gap-3 py-1.5">
                  <span className="text-label min-w-0 flex-1 truncate" title={site}>{passwordSite(site)}</span>
                  <Button variant="ghost" size="sm" onClick={() => allow(site)}>Offer again</Button>
                </li>
              ))}
            </ul>
          </SettingsCard>
        </SettingsSection>
      )}
      {profile && (
        <BrowserPasswordsAdd
          profile={profile}
          open={adding}
          onOpenChange={setAdding}
          onAdded={() => {
            setAdding(false);
            load(profile);
          }}
        />
      )}
      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title={removing && `Remove the password for ${passwordSite(removing.site)}?`}
        description={removing ? `${removing.username || "This password"} won't fill any more. This can't be undone.` : ""}
        confirm="Remove"
        destructive
        busy={busy}
        onConfirm={remove}
      />
    </SettingsPage>
  );
}
