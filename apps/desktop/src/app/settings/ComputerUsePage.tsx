import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import { allowComputerAccess, useLiveComputerAccess } from "@/state/computerAccess";

/** The Computer use page's rows, for the page and for Settings search. */
export const COMPUTER_USE_ROWS = {
  accessibility: { label: "Control apps" },
  screenRecording: { label: "See the screen" },
} as const;

const INTRO =
  "Lets workers see and use apps on this Mac in the background. Your cursor and keyboard stay yours.";

const GRANTS: readonly ComputerGrant[] = ["accessibility", "screenRecording"];

/** What to do in System Settings after an Allow, and what to do when the switch is on but this still says no. */
export const GRANT_STEPS =
  "System Settings opens on the right list: Device Control and Data Access (called Accessibility before macOS 27) for Control apps, Screen & System Audio Recording for See the screen. Turn on Brigadier Computer Use there; this page updates by itself.";
export const STALE_ENTRY =
  "Already on there, but still not allowed here? It's from an older build: select Brigadier Computer Use, remove it with −, then press Allow… again.";
export const RESTARTING = "Brigadier Computer Use is restarting so it can see the screen.";

function GrantRow({
  grant,
  allowed,
  onAllow,
}: {
  grant: ComputerGrant;
  allowed: boolean;
  onAllow: (grant: ComputerGrant) => Promise<void>;
}) {
  const allow = useAction();
  return (
    <SettingsRow
      label={COMPUTER_USE_ROWS[grant].label}
      description={allowed ? "Allowed" : "Not allowed yet"}
      error={allow.error}
    >
      {!allowed && (
        <SettingsButton disabled={allow.busy} onClick={() => allow.run(() => onAllow(grant))}>
          Allow…
        </SettingsButton>
      )}
    </SettingsRow>
  );
}

/** The page's content for the permissions `access` reports; nothing where there is no computer use. */
export function ComputerUseBody({
  access,
  asked,
  onAllow,
}: {
  access: ComputerAccess | null;
  /** An Allow was clicked: the user is off to System Settings. */
  asked: boolean;
  onAllow: (grant: ComputerGrant) => Promise<void>;
}) {
  if (!access?.available) return null;
  const missing = GRANTS.some((grant) => !access[grant]);
  return (
    <SettingsPage title="Computer use" description={INTRO}>
      <SettingsSection>
        <SettingsCard>
          {GRANTS.map((grant) => (
            <GrantRow key={grant} grant={grant} allowed={access[grant]} onAllow={onAllow} />
          ))}
        </SettingsCard>
        {asked && missing && (
          <>
            <p className="text-foreground/65 text-label px-1">{GRANT_STEPS}</p>
            <p className="text-foreground/50 text-label px-1">{STALE_ENTRY}</p>
          </>
        )}
        {access.restarting && <p className="text-foreground/65 text-label px-1">{RESTARTING}</p>}
        {access.problem && (
          <p className="text-foreground/50 text-label px-1">{access.problem}</p>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}

export function ComputerUsePage() {
  // Kept current while shown: a switch turned on in System Settings shows here by itself.
  const access = useLiveComputerAccess();
  const [asked, setAsked] = useState(false);

  return (
    <ComputerUseBody
      access={access}
      asked={asked}
      onAllow={(grant) => {
        setAsked(true);
        return allowComputerAccess(grant);
      }}
    />
  );
}
