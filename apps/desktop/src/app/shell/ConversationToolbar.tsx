import { PinnedSummaryToggle } from "@/app/conversation/PinnedSummary";
import { NewTabButton } from "@/app/shell/SessionTabBar";
import { TerminalButton } from "@/components/shared";
import { WorkspacePaneToggle } from "@/app/side-panel/WorkspacePane";
import { toolbarToggles, type ToolbarKind } from "@/state/workspacePane";

/** Shared by sessions, project drafts and chats, so their controls keep one order. */
export function ConversationToolbar({ kind, tabsId = null, tabbed = false, onContext }: { kind: ToolbarKind; tabsId?: string | null; tabbed?: boolean; onContext?: () => void }) {
  return toolbarToggles(kind, tabbed).map((toggle) => toggle === "context"
    ? <span key={toggle} className="contents" onClickCapture={onContext}><PinnedSummaryToggle /></span>
    : toggle === "new-tab" ? tabsId && <NewTabButton key={toggle} tabsId={tabsId} />
    : toggle === "terminal" ? <TerminalButton key={toggle} /> : <WorkspacePaneToggle key={toggle} />);
}
