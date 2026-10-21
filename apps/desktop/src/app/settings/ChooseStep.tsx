import { useState } from "react";

import { FlowHeader } from "@/app/settings/FlowHeader";
import { ProfilePick } from "@/app/settings/ProfilePick";
import { Button, CheckboxRow } from "@/components/controls";
import { DialogFooter } from "@/components/overlays";
import type { ImportSource } from "@/ipc/generated";
import { counted } from "@/state/browserImport";

/** Which profiles to copy, and whether history comes too. */
export function ChooseStep({
  source,
  onCancel,
  onImport,
}: {
  source: ImportSource;
  onCancel: () => void;
  onImport: (source: ImportSource, ids: string[], history: boolean) => void;
}) {
  const [ticked, setTicked] = useState<Set<string>>(() => new Set(source.profiles[0] ? [source.profiles[0].id] : []));
  const [history, setHistory] = useState(true);
  const chosen = source.profiles.filter((profile) => ticked.has(profile.id));
  const cookies = chosen.reduce((sum, profile) => sum + (profile.cookies ?? 0), 0);
  const tick = (id: string, on: boolean) =>
    setTicked((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });
  const note =
    source.family === "chromium"
      ? `${source.name} must stay closed during the import. macOS may ask for Keychain access to its encryption key; choose Allow (Always Allow avoids asking again).`
      : source.family === "gecko"
        ? `${source.name} must stay closed during the import.`
        : null;
  return (
    <>
      <FlowHeader
        source={source}
        title={`Import from ${source.name}`}
        description="Which profiles should be copied into Brigadier's browser?"
      />
      <div data-slot="import-profiles" className="border-divider bg-card rounded-settings flex flex-col overflow-hidden border">
        {source.profiles.map((profile) => (
          <ProfilePick
            key={profile.id}
            name={profile.name}
            cookies={profile.cookies}
            checked={ticked.has(profile.id)}
            onCheckedChange={(on) => tick(profile.id, on)}
          />
        ))}
      </div>
      <CheckboxRow
        label="Also bring browsing history"
        note="The pages you visited, to search and revisit."
        checked={history}
        onCheckedChange={setHistory}
      />
      {note && <p className="text-foreground/65 text-xs">{note}</p>}
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          disabled={chosen.length === 0}
          onClick={() => onImport(source, chosen.map((profile) => profile.id), history)}
        >
          Import {counted(cookies, "cookie", "cookies")}
        </Button>
      </DialogFooter>
    </>
  );
}
