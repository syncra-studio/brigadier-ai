import { actionDetail, callDetail, leadAction, ShellBox, useLeadItems } from "@/app/conversation/activity/StepDetail";
import { SHELLS } from "@/app/conversation/activity/toolSets";
import { parsedInput, toolName, type ToolStep } from "@/app/conversation/activity/words";

/**
 * What a lead's call opens to, the same as a worker's action (`actionDetail`): its item in the
 * CLI's thread, while it runs too, its Shell box following the output as it comes.
 */
export function LeadItemDetail({ conversationId, kind }: { conversationId: string; kind: ToolStep }) {
  const { items } = useLeadItems(conversationId, kind);
  const item = leadAction(items, kind);
  if (!item) return kind.status === "inProgress" ? null : <span className="text-muted-foreground text-xs">Nothing more was kept of this step.</span>;
  if (item.kind === "tool") {
    const short = toolName(item.name);
    // A CLI's own command gives its command line as plain text, not a call's JSON.
    const command = SHELLS.has(short) && parsedInput(item.input) === null ? item.input || kind.detail : null;
    const detail = callDetail(item.name, kind.status, { input: item.input, output: item.output, exit: kind.exit ?? null, command, stopped: kind.stopped });
    if (detail) return <>{detail}</>;
    if (short === "WebSearch" || short === "web_search") return null;
    return <span className="text-muted-foreground text-xs">Nothing more was kept of this step.</span>;
  }
  if (item.kind === "command") return <ShellBox command={item.command || (kind.detail ?? "")} output={item.output || null} status={kind.status} exit={item.exitCode ?? kind.exit ?? null} stopped={kind.stopped} />;
  return <>{actionDetail(item)}</>;
}
