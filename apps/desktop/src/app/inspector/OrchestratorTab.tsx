import { useMemo, useState } from "react";

import { SessionBar } from "@/app/inspector/SessionBar";
import { ThreadMetricsPanel } from "@/app/inspector/ThreadMetricsPanel";
import { shownBeforeSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * A session's thread: its own commits and how its context grew over each request. It starts on
 * the session open before Settings (else the newest one); the picker shows another. Brigadier
 * puts nothing into a thread's context of its own any more (D13), so there is no injection
 * log to show (#540).
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
  if (sessionId === null) {
    return (
      <p className="text-muted-foreground p-4 text-xs">
        Start a session to see its orchestrator.
      </p>
    );
  }
  return (
    <>
      <SessionBar sessionId={sessionId} options={options} onChange={setPicked} />
      <div className="flex min-h-0 flex-col gap-3 overflow-y-auto px-3 py-2.5 text-xs">
        <ThreadMetricsPanel sessionId={sessionId} />
      </div>
    </>
  );
}
