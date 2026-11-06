import { useState } from "react";

import { FlowHeader } from "@/app/settings/FlowHeader";
import { Button, Checkbox, CheckboxRow, SettingsSelect } from "@/components/controls";
import { DialogFooter } from "@/components/overlays";
import { counted, type ImportChoice, type ImportWhat, type ListedSource } from "@/state/browserImport";
import { type BrowserProfile, tabsOn, useProfiles } from "@/state/browserProfiles";
import { useApp } from "@/state/store";

const DAY = 86_400_000;

function when(atMs: number | undefined): string {
  if (atMs === undefined) return "Not imported yet";
  const days = Math.floor((Date.now() - atMs) / DAY);
  return days <= 0 ? "Imported today" : days === 1 ? "Imported yesterday" : `Imported ${days} days ago`;
}

/**
 * Which profiles to bring, each as a new profile or replacing one, and what of them. A profile
 * already imported from here defaults to replacing what it became; `into` is the profile the page
 * was asked to import into, which the browser's first profile then replaces.
 */
export function ProfileImportPick({
  source,
  into,
  onCancel,
  onImport,
}: {
  source: ListedSource;
  into: string | null;
  onCancel: () => void;
  onImport: (source: ListedSource, choices: ImportChoice[], what: ImportWhat) => void;
}) {
  const profiles = useProfiles();
  const mac = useApp((s) => s.info?.platform === "macos");
  const before = (id: string): BrowserProfile | undefined => profiles.find((profile) => profile.id === source.imports[id]?.into);
  const [ticked, setTicked] = useState<Set<string>>(
    () => new Set(source.profiles.filter((each, index) => index === 0 || !before(each.id)).map((each) => each.id)),
  );
  const [targets, setTargets] = useState<Record<string, string>>(() =>
    Object.fromEntries(source.profiles.map((each, index) => [each.id, index === 0 && into ? into : before(each.id)?.id ?? "new"])),
  );
  const [picked, setWhat] = useState<ImportWhat>({ signIns: true, history: true, bookmarks: true, passwords: true });
  const { passwords, signIns } = source;
  const what = { ...picked, signIns: signIns && picked.signIns, passwords: passwords && picked.passwords };
  const chosen = source.profiles.filter((each) => ticked.has(each.id));
  const part = (key: keyof ImportWhat) => ({
    checked: what[key],
    onCheckedChange: (on: boolean) => setWhat((current) => ({ ...current, [key]: on })),
  });
  const replacing = new Set(chosen.map((each) => targets[each.id]).filter((target) => target !== "new"));
  const nothing = !Object.values(what).some(Boolean);
  // Two profiles can't replace the same one.
  const twice = replacing.size !== chosen.filter((each) => targets[each.id] !== "new").length;
  // A profile being imported into is let go of first, so its open pages load again.
  const reloading = profiles
    .filter((profile) => replacing.has(profile.id))
    .map((profile) => ({ name: profile.name, tabs: tabsOn(profile.id) }))
    .filter(({ tabs }) => tabs > 0);
  const options = [
    { value: "new", label: "New profile" },
    ...profiles.map((profile) => ({ value: profile.id, label: `Replace ${profile.name}` })),
  ];
  return (
    <>
      <FlowHeader source={source} title={`Import from ${source.name}`} description="Each profile you tick stays separate in Brigadier." />
      <div data-slot="import-profiles" className="border-divider bg-card rounded-settings flex flex-col overflow-hidden border">
        {source.profiles.map((each) => {
          const on = ticked.has(each.id);
          return (
            <div key={each.id} data-slot="import-profile" className="not-last:border-divider flex items-center gap-3 px-4 py-3 not-last:border-b">
              <Checkbox
                aria-label={`Import ${each.name}`}
                checked={on}
                onCheckedChange={(next) =>
                  setTicked((current) => {
                    const updated = new Set(current);
                    if (next === true) updated.add(each.id);
                    else updated.delete(each.id);
                    return updated;
                  })
                }
              />
              <span className="grid min-w-0 flex-1 gap-0.5">
                <span className="text-label break-words">{each.name}</span>
                <span className="text-foreground/65 text-xs">{when(before(each.id) && source.imports[each.id]?.atMs)}</span>
              </span>
              <SettingsSelect
                label={`Import ${each.name} as`}
                value={targets[each.id] ?? "new"}
                onChange={(target) => setTargets((current) => ({ ...current, [each.id]: target }))}
                disabled={!on}
                className="w-40"
                options={options}
              />
            </div>
          );
        })}
      </div>
      <p className={twice ? "text-destructive text-xs" : "text-foreground/65 text-xs"}>
        {twice
          ? "Two profiles can't replace the same one."
          : !signIns
            ? "Replacing combines a profile's bookmarks and history with these."
            : passwords
              ? "Replacing swaps a profile's sign-ins and passwords for these. Its bookmarks and history are combined."
              : "Replacing swaps a profile's sign-ins for these. Its bookmarks and history are combined."}
      </p>
      {reloading.length > 0 && (
        <p data-slot="import-reload" className="text-foreground/65 text-xs">
          {reloading.map(({ name, tabs }) => `${counted(tabs, "open tab", "open tabs")} on ${name} will reload.`).join(" ")}
        </p>
      )}
      <div className="grid gap-2">
        <span className="text-label font-medium">What to bring</span>
        <div className="grid grid-cols-2 gap-x-6 gap-y-2.5">
          {signIns && <CheckboxRow label="Sign-ins" note="Stay signed in on your sites" {...part("signIns")} />}
          <CheckboxRow label="Browsing history" note="Pages you visited" {...part("history")} />
          <CheckboxRow label="Bookmarks" note="With their folders" {...part("bookmarks")} />
          {passwords && <CheckboxRow label="Passwords" note="Filled for you, never shown to agents" {...part("passwords")} />}
        </div>
      </div>
      {!signIns && (
        <p data-slot="import-no-sign-ins" className="text-foreground/65 text-xs">
          {source.name}'s sign-ins and passwords stay behind: they are locked with a key Brigadier doesn't touch. You'll sign in again here.
        </p>
      )}
      {source.emptyProfiles.length > 0 && (
        <p data-slot="import-skipped" className="text-foreground/65 text-xs">
          Skipped {new Intl.ListFormat("en", { type: "conjunction" }).format(source.emptyProfiles)}: nothing to bring.
        </p>
      )}
      {source.firstImport && (
        <p data-slot="import-agents" className="text-foreground/65 text-xs">
          Agents working in a profile use its sign-ins. To keep agents out of a site, keep it in another profile.
        </p>
      )}
      <p className="text-foreground/65 text-xs">
        Close {source.name} first. Your computer may ask you to allow access to its saved data.{" "}
        {mac
          ? "Passkeys aren't copied: they stay on your device, and on this Mac they can't be used here yet."
          : "Passkeys aren't copied: they stay on your device and work here when a site asks."}
      </p>
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={onCancel}>Cancel</Button>
        <Button
          type="button"
          disabled={chosen.length === 0 || nothing || twice}
          onClick={() => onImport(source, chosen.map((each) => ({ id: each.id, name: each.name, into: targets[each.id] ?? "new" })), what)}
        >
          Import {counted(chosen.length, "profile", "profiles")}
        </Button>
      </DialogFooter>
    </>
  );
}
