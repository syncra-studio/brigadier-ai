import { useId, useState } from "react";

import { Button, Input } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/overlays";
import { type BrowserProfile, deleteProfile, sessionsOn, tabsOn, useDefaultProfile, useProfilePlaces } from "@/state/browserProfiles";
import { counted } from "@/state/browserImport";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/** Delete, confirmed by typing the name. Its sessions and tabs move to the default, and reload, signed in there. */
export function ProfileDeleteDialog({
  profile,
  open,
  onOpenChange,
  onDeleted,
}: {
  profile: BrowserProfile;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDeleted?: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" data-slot="profile-delete">
        {open && <Form profile={profile} onClose={() => onOpenChange(false)} onDeleted={onDeleted} />}
      </DialogContent>
    </Dialog>
  );
}

function Form({ profile, onClose, onDeleted }: { profile: BrowserProfile; onClose: () => void; onDeleted: (() => void) | undefined }) {
  const fallback = useDefaultProfile();
  const [typed, setTyped] = useState("");
  const field = useId();
  const places = useProfilePlaces((s) => s.sessions);
  const conversations = useApp((s) => s.conversations);
  const sessions = sessionsOn(profile.id, places, conversations);
  const tabs = tabsOn(profile.id);
  const target = fallback.name;
  const remove = () => {
    deleteProfile(profile.id).catch((cause: unknown) => {
      toast(`Its data couldn't be removed: ${cause instanceof Error ? cause.message : String(cause)}`, { tone: "error" });
    });
    onClose();
    onDeleted?.();
  };
  return (
    <>
      <DialogHeader>
        <DialogTitle>Delete {profile.name}?</DialogTitle>
        <DialogDescription>
          Its sign-ins, history, bookmarks and passwords are removed from Brigadier. This can't be undone.
        </DialogDescription>
      </DialogHeader>
      <p className="text-foreground/65 text-xs">
        {sessions + tabs === 0
          ? `Nothing is open on ${profile.name}.`
          : `${counted(sessions, "session", "sessions")} and ${counted(tabs, "open tab", "open tabs")} move to ${target}. Those pages reload, signed in as ${target}.`}
      </p>
      <div className="grid gap-1.5">
        <label htmlFor={field} className="text-label">Type <span className="font-medium">{profile.name}</span> to confirm</label>
        <Input id={field} value={typed} autoFocus onChange={(event) => setTyped(event.target.value)} />
      </div>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>Cancel</Button>
        <Button variant="destructive" disabled={typed !== profile.name} onClick={remove}>Delete profile</Button>
      </DialogFooter>
    </>
  );
}
