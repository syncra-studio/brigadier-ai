import { PinnedSummaryToggle } from "@/app/conversation/PinnedSummary";
import { TerminalButton } from "@/app/side-panel/SidePanel";
import { WorkspacePaneToggle } from "@/app/side-panel/WorkspacePane";
import { toolbarToggles } from "@/state/workspacePane";

/** Shared by sessions, project drafts and chats, so their controls keep one order. */
export function ConversationToolbar({ kind, onContext }: { kind: "session" | "chat"; onContext?: () => void }) {
  return toolbarToggles(kind).map((toggle) => toggle === "context"
    ? <span key={toggle} className="contents" onClickCapture={onContext}><PinnedSummaryToggle /></span>
    : toggle === "terminal" ? <TerminalButton key={toggle} /> : <WorkspacePaneToggle key={toggle} />);
}
