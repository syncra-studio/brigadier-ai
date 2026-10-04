import { useId, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  Segmented,
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSelect,
  SettingsSwitch,
} from "@/app/settings/parts";
import { Input } from "@/components/ui/input";
import type { Density } from "@/ipc/generated";
import { setDensity } from "@/state/actions";
import {
  keepAwakeOptions,
  lidClosedHint,
  setKeepAwake,
  setKeepAwakeLidClosed,
  useKeepAwake,
} from "@/state/keepAwake";
import { reopenOnboarding } from "@/state/onboarding";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The General page's rows, for the page and for Settings search. */
export const GENERAL_ROWS = {
  density: {
    label: "Density",
    description: "Compact makes controls smaller and spacing tighter across the app.",
  },
  keepAwake: {
    label: "Keep the computer awake",
    description: "Whether this computer may sleep while Brigadier is open.",
  },
  lidClosed: {
    label: "Keep going with the lid closed",
    description: "Applied whenever the computer is kept awake.",
  },
  hibernate: {
    label: "Hibernate after",
    description:
      "An idle conversation stops its CLI processes and cleans up; it continues where it left off.",
  },
  setup: {
    label: "Run setup again",
    description: "Sign in to agents, pick projects and choose defaults, as on first launch.",
  },
} as const;

const DENSITIES = [
  { value: "compact", label: "Compact" },
  { value: "normal", label: "Normal" },
] as const satisfies readonly { value: Density; label: string }[];

export function GeneralPage() {
  const density = useApp((s) => s.settings.density);
  const keepAwake = useApp((s) => s.settings.keepAwake);
  const lidClosed = useApp((s) => s.settings.keepAwakeLidClosed);
  const status = useKeepAwake((s) => s.status);
  const settingUp = useKeepAwake((s) => s.settingUp);
  const densityAction = useAction();
  const keepAwakeAction = useAction();
  const lidAction = useAction();
  const options = keepAwakeOptions(status?.screenOn ?? false);
  const chosen = options.find((option) => option.value === keepAwake);

  return (
    <SettingsPage title="General">
      <SettingsSection title="Appearance">
        <SettingsCard>
          <SettingsRow
            label={GENERAL_ROWS.density.label}
            description={GENERAL_ROWS.density.description}
            error={densityAction.error}
          >
            <Segmented
              label={GENERAL_ROWS.density.label}
              value={density}
              options={DENSITIES}
              onChange={(value) => densityAction.run(() => setDensity(value))}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Power">
        <SettingsCard>
          <SettingsRow
            label={GENERAL_ROWS.keepAwake.label}
            description={
              status?.forRun
                ? "An overnight run keeps the computer awake, screen on, until it ends. Then this applies again."
                : chosen
                  ? `${chosen.hint}.`
                  : GENERAL_ROWS.keepAwake.description
            }
            error={keepAwakeAction.error}
          >
            <SettingsSelect
              label={GENERAL_ROWS.keepAwake.label}
              value={keepAwake}
              options={options.map((option) => ({
                value: option.value,
                label: option.label,
                hint: `${option.hint}.`,
              }))}
              onChange={(value) => keepAwakeAction.run(() => setKeepAwake(value))}
            />
          </SettingsRow>
          {status?.lidClosed !== "unsupported" && (
            <SettingsRow
              label={GENERAL_ROWS.lidClosed.label}
              description={lidClosedHint(status, settingUp)}
              error={lidAction.error}
            >
              <SettingsSwitch
                label={GENERAL_ROWS.lidClosed.label}
                checked={lidClosed}
                disabled={settingUp}
                onCheckedChange={(on) => lidAction.run(() => setKeepAwakeLidClosed(on))}
              />
            </SettingsRow>
          )}
          <HibernateRow />
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Setup">
        <SettingsCard>
          <SettingsRow label={GENERAL_ROWS.setup.label} description={GENERAL_ROWS.setup.description}>
            <SettingsButton onClick={() => reopenOnboarding()}>Run setup</SettingsButton>
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>
    </SettingsPage>
  );
}

/** The range Hibernate after accepts: a minute to a week. */
const HIBERNATE_MINUTES = { min: 1, max: 10_080 } as const;

/** Minutes before an idle conversation hibernates: saved when the field is left or on Enter. */
function HibernateRow() {
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
      error={invalid ? "A whole number of minutes, from 1 to 10,080 (a week)." : save.error}
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
