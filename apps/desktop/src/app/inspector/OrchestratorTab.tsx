import { useEffect, useMemo, useState } from "react";

import { OrchestratorLogView } from "@/app/inspector/OrchestratorLogView";
import { SessionBar } from "@/app/inspector/SessionBar";
import { openOrchestratorLog, shownBeforeSettings } from "@/state/actions";
import { useApp } from "@/state/store";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * A session's orchestrator: every context injection against the CLI's reported context size,
 * so it is visible that its context grows only by messages and reports. It starts on the
 * session open before Settings (else the newest one); the picker shows another.
 */
export function OrchestratorTab() {
  const conversations = useApp((s) => s.conversations);
  // The sessions to choose from, newest first.
  const options = useMemo(
    () =>
      Object.values(conversations)
        .filter(
          (conversation) =>
            conversation.kind === "session" &&
            conversation.lifecycle !== "archived" &&
            conversation.sideOf === null,
        )
        .toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)
        .map((conversation) => ({ value: conversation.id, label: conversation.title })),
    [conversations],
  );
  const [picked, setPicked] = useState<string | null>(() => {
    const before = shownBeforeSettings();
    return before.type === "conversation" ? before.id : null;
  });
  const sessionId =
    options.find((option) => option.value === picked)?.value ?? options[0]?.value ?? null;
  const [failure, setFailure] = useState<{ sessionId: string; message: string } | null>(null);

  // Follow the open session's log while this tab shows; stop following when it closes.
  useEffect(() => {
    if (sessionId === null) return undefined;
    openOrchestratorLog(sessionId).catch((cause: unknown) =>
      setFailure({ sessionId, message: errorText(cause) }),
    );
    return () => {
      void openOrchestratorLog(null);
    };
  }, [sessionId]);

  if (sessionId === null) {
    return (
      <p className="text-muted-foreground p-4 text-xs">
        Start a session to see its orchestrator.
      </p>
    );
  }
  const error = failure?.sessionId === sessionId ? failure.message : null;
  return (
    <>
      <SessionBar sessionId={sessionId} options={options} onChange={setPicked} />
      <OrchestratorLogView sessionId={sessionId} error={error} />
    </>
  );
}
