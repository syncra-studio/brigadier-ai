import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import {
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
} from "@/app/settings/parts";
import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import {
  allowComputerAccess,
  readComputerAccess,
  useComputerAccess,
} from "@/state/computerAccess";

/** The Computer use page's rows, for the page and for Settings search. */
export const COMPUTER_USE_ROWS = {
  accessibility: { label: "Control apps" },
  screenRecording: { label: "See the screen" },
} as const;

const INTRO =
  "Lets workers see and use apps on this Mac in the background. Your cursor and keyboard stay yours.";

const GRANTS: readonly ComputerGrant[] = ["accessibility", "screenRecording"];

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
  return (
    <SettingsPage title="Computer use" description={INTRO}>
      <SettingsSection>
        <SettingsCard>
          {GRANTS.map((grant) => (
            <GrantRow key={grant} grant={grant} allowed={access[grant]} onAllow={onAllow} />
          ))}
        </SettingsCard>
        {asked && (
          <p className="text-foreground/65 text-label px-1">
            In System Settings, turn on Brigadier Computer Use.
          </p>
        )}
        {access.problem && (
          <p className="text-foreground/50 text-label px-1">{access.problem}</p>
        )}
      </SettingsSection>
    </SettingsPage>
  );
}

export function ComputerUsePage() {
  const access = useComputerAccess();
  const [asked, setAsked] = useState(false);

  // Read again when shown and whenever the window comes back: the user grants in System
  // Settings, then returns here.
  useEffect(() => {
    readComputerAccess();
    window.addEventListener("focus", readComputerAccess);
    return () => window.removeEventListener("focus", readComputerAccess);
  }, []);

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
