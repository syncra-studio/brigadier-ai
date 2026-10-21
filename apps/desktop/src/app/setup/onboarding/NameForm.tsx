import { type FormEvent, useId, useState } from "react";

import { errorText } from "@/app/shell/dialogs/fields";
import { ContinueButton } from "@/app/setup/onboarding/ContinueButton";
import { useFocusOnMount } from "@/app/setup/onboarding/parts";
import { BrigadierGlyph } from "@/components/icons";
import { Input } from "@/components/controls";
import { DialogTitle } from "@/components/overlays";

/**
 * "What should we call you?" under the mark: one borderless field, focused as the step opens;
 * Continue (or Return) saves it once it holds more than spaces.
 */
export function NameForm({ name, onName }: { name: string; onName: (name: string) => Promise<void> }) {
  const id = useId();
  const [value, setValue] = useState(name);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const trimmed = value.trim();
  const input = useFocusOnMount<HTMLInputElement>();

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onName(trimmed);
    } catch (cause) {
      setError(errorText(cause));
      setBusy(false);
    }
  };

  return (
    <div className="welcome-name">
      <form id={id} className="welcome-name-body" onSubmit={(event) => void submit(event)}>
        <div className="welcome-mark" aria-hidden>
          <BrigadierGlyph />
        </div>
        <DialogTitle asChild>
          <h1 className="welcome-heading text-balance">What should we call you?</h1>
        </DialogTitle>
        <label htmlFor={`${id}-name`} className="sr-only">
          Your name
        </label>
        <Input
          ref={input}
          id={`${id}-name`}
          autoComplete="given-name"
          placeholder="Your name"
          maxLength={80}
          value={value}
          disabled={busy}
          aria-invalid={error !== null || undefined}
          onChange={(event) => setValue(event.target.value)}
          // Borderless: the words alone, centred under the question.
          className="placeholder:text-muted-foreground/70 focus-visible:bg-transparent disabled:bg-transparent h-auto max-w-xs rounded-none bg-transparent p-0 text-center text-lg outline-none"
        />
        {error && (
          <p role="alert" className="text-destructive mt-3 text-sm">
            {error}
          </p>
        )}
      </form>
      <div className="welcome-continue">
        <ContinueButton ready={trimmed !== ""} busy={busy} form={id} />
      </div>
    </div>
  );
}
