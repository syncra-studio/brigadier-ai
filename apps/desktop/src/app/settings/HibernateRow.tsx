import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { GENERAL_ROWS } from "@/app/settings/generalRows";
import { Input } from "@/components/controls";
import { SettingsRow } from "@/components/shared";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The range Hibernate after accepts: a minute to a week. */
const HIBERNATE_MINUTES = { min: 1, max: 10_080 } as const;

/** Minutes before an idle conversation hibernates: saved when the field is left or on Enter. */
export function HibernateRow() {
  const id = useId();
  const minutes = useApp((s) => s.settings.hibernateAfterMinutes);
  // What is typed, while it differs from the setting; else the setting (which a change from
  // elsewhere, or a failed save going back, updates).
  const [typed, setTyped] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const save = useAction();
  const text = typed ?? String(minutes);

  const commit = () => {
    if (typed === null) return;
    const value = Number(typed);
    if (
      !Number.isInteger(value) ||
      value < HIBERNATE_MINUTES.min ||
      value > HIBERNATE_MINUTES.max
    ) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    setTyped(null);
    if (value !== minutes) save.run(() => setSetting("hibernateAfterMinutes", value));
  };

  return (
    <SettingsRow
      label={GENERAL_ROWS.hibernate.label}
      description={GENERAL_ROWS.hibernate.description}
      htmlFor={id}
      error={invalid ? "A whole number of minutes, from 1 to 10,080 (a week)" : save.error}
    >
      <Input
        id={id}
        type="number"
        inputMode="numeric"
        min={HIBERNATE_MINUTES.min}
        max={HIBERNATE_MINUTES.max}
        step={1}
        value={text}
        aria-invalid={invalid || undefined}
        className="border-input bg-popover h-control-md rounded-control text-label w-18 [appearance:textfield] px-2.5 tabular-nums [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
        onChange={(event) => setTyped(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") {
            // Esc here undoes the typing rather than leaving Settings.
            event.stopPropagation();
            setTyped(null);
            setInvalid(false);
          }
        }}
      />
      <span className="text-muted-foreground text-label">min</span>
    </SettingsRow>
  );
}
