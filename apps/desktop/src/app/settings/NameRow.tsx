import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { Input } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The name the app calls the user by: saved as it's left or on Enter; Escape undoes the typing. */
export function NameRow() {
  const id = useId();
  const name = useApp((s) => s.settings.userName);
  // What is typed, while it differs from the setting.
  const [typed, setTyped] = useState<string | null>(null);
  const save = useAction();

  const commit = () => {
    if (typed === null) return;
    const value = typed.trim();
    setTyped(null);
    if (value !== name) save.run(() => setSetting("userName", value));
  };

  return (
    <SettingsRow
      label={GENERAL_ROWS.name.label}
      description={GENERAL_ROWS.name.description}
      htmlFor={id}
      error={save.error}
    >
      <Input
        id={id}
        autoComplete="given-name"
        maxLength={80}
        placeholder="Your name"
        value={typed ?? name}
        className="border-input bg-popover h-control-md rounded-control text-label w-48 px-2.5"
        onChange={(event) => setTyped(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") {
            // Esc here undoes the typing rather than leaving Settings.
            event.stopPropagation();
            setTyped(null);
          }
        }}
      />
    </SettingsRow>
  );
}
