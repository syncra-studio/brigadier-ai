import { useId, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { Button, ErrorLine, Input } from "@/components/controls";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/overlays";
import { addPassword, siteAddress } from "@/state/browserPasswords";

/** Adds a password by hand. `onAdded` follows once the browser's store has it. */
export function BrowserPasswordsAdd({
  profile,
  open,
  onOpenChange,
  onAdded,
}: {
  profile: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add password</DialogTitle>
          <DialogDescription>It fills when you sign in on this site in Brigadier's browser.</DialogDescription>
        </DialogHeader>
        {/* Made each time it opens: what was typed last time is gone. */}
        {open && <AddForm profile={profile} onCancel={() => onOpenChange(false)} onAdded={onAdded} />}
      </DialogContent>
    </Dialog>
  );
}

function AddForm({ profile, onCancel, onAdded }: { profile: string; onCancel: () => void; onAdded: () => void }) {
  const ids = useId();
  const [site, setSite] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const address = siteAddress(site);
  const submit = () => {
    if (!address || !password) return;
    setBusy(true);
    addPassword(profile, address, username.trim(), password)
      .then(onAdded)
      .catch((cause: unknown) => {
        setError(errorText(cause));
        setBusy(false);
      });
  };
  const fields = [
    { id: `${ids}-site`, label: "Site", value: site, set: setSite, type: "text", hint: "github.com" },
    { id: `${ids}-user`, label: "Username", value: username, set: setUsername, type: "text", hint: "you@example.com" },
    { id: `${ids}-pass`, label: "Password", value: password, set: setPassword, type: "password", hint: "" },
  ];
  return (
    <form
      className="flex flex-col gap-3"
      autoComplete="off"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {fields.map((field) => (
        <label key={field.id} htmlFor={field.id} className="flex flex-col gap-1">
          <span className="text-label">{field.label}</span>
          <Input
            id={field.id}
            type={field.type}
            value={field.value}
            placeholder={field.hint}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => field.set(event.target.value)}
          />
        </label>
      ))}
      <ErrorLine error={site && !address ? "That isn't a web address." : error} />
      <DialogFooter>
        <Button type="button" variant="ghost" disabled={busy} onClick={onCancel}>Cancel</Button>
        <Button type="submit" disabled={busy || !address || !password}>Add password</Button>
      </DialogFooter>
    </form>
  );
}
