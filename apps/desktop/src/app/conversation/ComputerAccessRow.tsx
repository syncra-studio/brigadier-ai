import { HandRaised } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { COMPUTER_USE_ROWS, GRANT_STEPS } from "@/app/settings/ComputerUsePage";
import { SummaryRow } from "@/components/assistant-ui/elements/summary-section";
import { Button } from "@/components/ui/button";
import type { ComputerAccess, ComputerGrant } from "@/ipc/generated";
import { allowComputerAccess, useLiveComputerAccess } from "@/state/computerAccess";

const GRANTS: readonly ComputerGrant[] = ["accessibility", "screenRecording"];

function GrantButton({ grant, onAllow }: { grant: ComputerGrant; onAllow: (grant: ComputerGrant) => Promise<void> }) {
  const allow = useAction();
  return (
    <div data-slot="computer-grant" className="flex min-h-7 items-center gap-2 ps-6 text-sm">
      <span className="text-foreground/65 min-w-0 flex-1 truncate">{COMPUTER_USE_ROWS[grant].label}</span>
      {allow.error && <span role="alert" className="text-destructive text-xs">{allow.error}</span>}
      <Button size="xs" variant="ghost" disabled={allow.busy} onClick={() => allow.run(() => onAllow(grant))}>
        Allow…
      </Button>
    </div>
  );
}

/**
 * The "Waiting on you" item for computer use's missing permissions: what workers need, and one
 * Allow button per permission still missing. It closes by itself once both are allowed.
 */
export function ComputerAccessBody({
  what,
  access,
  asked,
  onAllow,
}: {
  what: string;
  access: ComputerAccess | null;
  /** An Allow was clicked: the user is off to System Settings. */
  asked: boolean;
  onAllow: (grant: ComputerGrant) => Promise<void>;
}) {
  const missing = access ? GRANTS.filter((grant) => !access[grant]) : [];
  return (
    <div data-slot="computer-access" className="flex flex-col">
      <SummaryRow icon={<HandRaised />}>
        <span className="min-w-0 truncate">
          <WorkerLine text={what} />
        </span>
      </SummaryRow>
      {missing.map((grant) => <GrantButton key={grant} grant={grant} onAllow={onAllow} />)}
      {access && missing.length === 0 && <p className="text-foreground/65 ps-6 text-sm">Allowed. Workers can use apps now.</p>}
      {asked && missing.length > 0 && <p className="text-foreground/65 text-label ps-6">{GRANT_STEPS}</p>}
    </div>
  );
}

export function ComputerAccessRow({ what }: { what: string }) {
  // Kept current while shown; a read that finds both allowed closes this item.
  const access = useLiveComputerAccess();
  const [asked, setAsked] = useState(false);
  return (
    <ComputerAccessBody
      what={what}
      access={access}
      asked={asked}
      onAllow={(grant) => {
        setAsked(true);
        return allowComputerAccess(grant);
      }}
    />
  );
}
