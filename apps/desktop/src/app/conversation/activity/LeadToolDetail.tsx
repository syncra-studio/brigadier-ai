import { useEffect, useState } from "react";

import { Diffs } from "@/app/conversation/activity/Diffs";
import { Output } from "@/app/conversation/activity/Output";
import { callDetail } from "@/app/conversation/activity/StepDetail";
import { EDITS, SHELLS } from "@/app/conversation/activity/toolSets";
import { fileDiffs, parsedInput, toolName, type ToolStep } from "@/app/conversation/activity/words";
import { ToolBox } from "@/components/conversation";
import type { ThreadItem } from "@/ipc/generated";
import { getThreadItem } from "@/state/actions";

/**
 * What a lead's tool step opens to, fetched from the thread's log when it opens: the daemon
 * keeps the call's whole input and output there, the board only its one-line detail.
 */
export function LeadToolDetail({ conversationId, kind }: { conversationId: string; kind: ToolStep }) {
  const [item, setItem] = useState<ThreadItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    getThreadItem(conversationId, kind.itemId).then(
      (found) => current && setItem(found),
      (cause: unknown) => current && setError(cause instanceof Error ? cause.message : String(cause)),
    );
    return () => {
      current = false;
    };
  }, [conversationId, kind.itemId]);
  if (error) return <p className="text-destructive text-xs">{error}</p>;
  if (!item) return <span className="shimmer text-xs">Loading</span>;
  const short = toolName(kind.name);
  // An edit the CLI reported as file changes (no call input): the daemon gives their diffs, each
  // after its path.
  if (EDITS.has(short) && !item.input && item.output) return <Diffs files={fileDiffs(item.output)} />;
  // A CLI's own command gives its command line as plain text, not a call's JSON.
  const command = SHELLS.has(short) && parsedInput(item.input) === null ? item.input || kind.detail : null;
  const detail = callDetail(kind.name, kind.status, { input: item.input, output: item.output, exit: item.exit ?? kind.exit ?? null, command, stopped: kind.stopped });
  if (detail) return <>{detail}</>;
  // A search with no results kept: nothing more to show than its row.
  if (short === "WebSearch" || short === "web_search") return null;
  if (EDITS.has(short) && kind.detail) return <ToolBox title="Files" className="mt-1"><Output text={kind.detail} /></ToolBox>;
  return <span className="text-muted-foreground text-xs">Nothing more was kept of this step.</span>;
}
