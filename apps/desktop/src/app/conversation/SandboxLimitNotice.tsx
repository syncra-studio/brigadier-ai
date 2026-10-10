import { useState } from "react";

import {
  FullAccessDialog,
  setSessionPermission,
  useUntrusted,
} from "@/app/conversation/SetupPickers";
import { useAction } from "@/app/conversation/useAction";
import { useViewConversation } from "@/app/conversation/viewContext";
import { WorkerLine } from "@/app/conversation/WorkerChip";
import { ShieldExclamation } from "@/components/glyphs/permission-glyphs";
import { Button } from "@/components/ui/button";
import { UNTRUSTED_NOTE } from "@/lib/setup";
import { openSettings } from "@/state/actions";

/**
 * The thread's notice that something the user asked for needs more than the session's sandbox
 * allows (its `suggest_full_access`): the lead's reason, "Switch this session to Full access"
 * (the composer's confirmation, then the composer's change, so its picker shows it too) and a
 * link to Settings > Configuration. Once the session has Full access the button gives way to
 * a line saying so; in a folder the user doesn't trust, to the reason it can't switch.
 */
export function SandboxLimitNotice({ reason }: { reason: string }) {
  const conversation = useViewConversation();
  const setup = conversation?.setup?.type === "session" ? conversation.setup : null;
  const untrusted = useUntrusted(conversation?.projectId ?? null, setup?.repo ?? null);
  const action = useAction();
  const [confirming, setConfirming] = useState(false);
  const full = setup?.permission === "fullAccess";
  return (
    <section
      aria-label="This needs more access"
      data-slot="orchestrator-step"
      data-kind="fullAccessSuggested"
      className="bg-rail border-foreground/10 rounded-surface my-1 flex flex-col gap-2 border px-4 py-3 text-sm"
    >
      <div className="flex min-w-0 items-start gap-3">
        <ShieldExclamation className="size-icon-md mt-0.5 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="font-medium">This needs more access than the sandbox allows</p>
          <p className="text-muted-foreground wrap-break-word">
            <WorkerLine text={reason} />
          </p>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 ps-8">
        {full ? (
          <p data-slot="full-access-on" className="text-muted-foreground">
            This session has Full access.
          </p>
        ) : untrusted ? (
          <p className="text-muted-foreground">{UNTRUSTED_NOTE}</p>
        ) : (
          <Button
            size="xs"
            className="rounded-capsule bg-full-access/15 text-full-access hover:bg-full-access/25"
            disabled={!conversation || action.busy}
            onClick={() => setConfirming(true)}
          >
            Switch this session to Full access
          </Button>
        )}
        <button
          type="button"
          className="text-link text-xs hover:underline"
          onClick={() => openSettings("configuration")}
        >
          {"Open Settings > Configuration"}
        </button>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive ps-8 text-xs">
          {action.error}
        </p>
      )}
      <FullAccessDialog
        open={confirming}
        onCancel={() => setConfirming(false)}
        onConfirm={() => {
          setConfirming(false);
          if (conversation) action.run(() => setSessionPermission(conversation, "fullAccess"));
        }}
      />
    </section>
  );
}
