import { useState, type FormEvent } from "react";

import { Button } from "@/components/controls/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/overlays/dialog";
import { Input } from "@/components/controls/input";

export type NameDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  label: string;
  initialValue?: string;
  confirmLabel: string;
  onSubmit: (value: string) => Promise<void>;
};

/** A small form asking for one name: new project, rename a session or chat. */
export function NameDialog(props: NameDialogProps) {
  return (
    <Dialog open={props.open} onOpenChange={props.onOpenChange}>
      <DialogContent>
        {/* Remounted per opening so the field starts from the initial value. */}
        {props.open && <NameForm {...props} />}
      </DialogContent>
    </Dialog>
  );
}

function NameForm({
  onOpenChange,
  title,
  description,
  label,
  initialValue = "",
  confirmLabel,
  onSubmit,
}: NameDialogProps) {
  const [value, setValue] = useState(initialValue);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const name = value.trim();
    if (!name) {
      setError(`${label} can't be empty.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSubmit(name);
      onOpenChange(false);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form onSubmit={(event) => void submit(event)} className="grid gap-4">
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        {description && <DialogDescription>{description}</DialogDescription>}
      </DialogHeader>
      <div className="grid gap-1.5">
        <label htmlFor="name-dialog-input" className="text-muted-foreground text-xs">
          {label}
        </label>
        <Input
          id="name-dialog-input"
          value={value}
          autoFocus
          maxLength={200}
          aria-invalid={error !== null}
          onChange={(event) => setValue(event.target.value)}
        />
        {error && (
          <p role="alert" className="text-destructive text-xs">
            {error}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="submit" disabled={busy}>
          {confirmLabel}
        </Button>
      </DialogFooter>
    </form>
  );
}
