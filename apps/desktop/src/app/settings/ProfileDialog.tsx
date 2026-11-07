import { type FormEvent, useState } from "react";

import { ProfileSwatches } from "@/app/settings/ProfileMark";
import { Button, Input } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/overlays";
import { openSettings } from "@/state/actions";
import { type BrowserProfile, changeProfile, createProfile, nextHue, useProfiles } from "@/state/browserProfiles";

/** New profile, or (with `editing`) Rename: a name and a colour. */
export function ProfileDialog({
  open,
  onOpenChange,
  editing,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing?: BrowserProfile;
  onCreated?: (id: string) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" data-slot="profile-dialog">
        {open && <Form editing={editing} onClose={() => onOpenChange(false)} onCreated={onCreated} />}
      </DialogContent>
    </Dialog>
  );
}

function Form({ editing, onClose, onCreated }: { editing: BrowserProfile | undefined; onClose: () => void; onCreated: ((id: string) => void) | undefined }) {
  const profiles = useProfiles();
  const [name, setName] = useState(editing?.name ?? "");
  const [hue, setHue] = useState(editing?.hue ?? nextHue(profiles));
  const taken = profiles.some((profile) => profile.id !== editing?.id && profile.name.toLowerCase() === name.trim().toLowerCase());
  const ready = name.trim().length > 0 && !taken;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!ready) return;
    if (editing) changeProfile(editing.id, { name: name.trim(), hue });
    else {
      const id = createProfile(name, hue);
      onCreated?.(id);
    }
    onClose();
  };
  return (
    <form onSubmit={submit} className="contents">
      <DialogHeader>
        <DialogTitle>{editing ? `Rename ${editing.name}` : "New profile"}</DialogTitle>
        <DialogDescription>
          {editing ? "Its sign-ins, history, bookmarks and passwords stay as they are." : "A separate set of sign-ins, history, bookmarks and passwords. It starts empty."}
        </DialogDescription>
      </DialogHeader>
      <div className="grid gap-3">
        <label className="grid gap-1.5">
          <span className="text-label font-medium">Name</span>
          <Input value={name} autoFocus maxLength={40} onChange={(event) => setName(event.target.value)} />
          {taken && <span className="text-destructive text-xs">Another profile has this name.</span>}
        </label>
        <div className="grid gap-2">
          <span className="text-label font-medium">Colour</span>
          <div className="px-0.5"><ProfileSwatches value={hue} onChange={setHue} /></div>
        </div>
      </div>
      <DialogFooter className={editing ? undefined : "sm:justify-between"}>
        {!editing && (
          <Button
            type="button"
            variant="link"
            className="text-link px-0 font-normal"
            onClick={() => {
              onClose();
              openSettings("browserImport");
            }}
          >
            Import from a browser instead…
          </Button>
        )}
        <div className="flex gap-2">
          <Button type="button" variant="ghost" onClick={onClose}>Cancel</Button>
          <Button type="submit" disabled={!ready}>{editing ? "Save" : "Create profile"}</Button>
        </div>
      </DialogFooter>
    </form>
  );
}
