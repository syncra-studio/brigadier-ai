import type { AppAnswer } from "@/ipc/generated";
import { isPaused } from "@/state/agentPause";
import { documentIsSaved } from "@/state/documentDrafts";
import { usePageEdits } from "@/state/pageEditing";
import { CHAT_TAB, closeTab, isAgentTab, type SessionTab, sessionTabs } from "@/state/sessionTabs";
import { tabTitle } from "@/state/sessionTabTitle";

/**
 * `close_tab` (Q90) in the window. A tab an agent opened, with nothing unsaved, closes at once.
 * A tab of the user's (a paused agent tab is theirs now), one with unsaved edits, or a terminal
 * stays open and the answer says why; the daemon asks the user, or knows the terminal is idle,
 * and calls again `confirmed`.
 */
export function closeForAgent(conversationId: string, id: string, confirmed: boolean): AppAnswer {
  const { tabs } = sessionTabs(conversationId);
  const tab = id === CHAT_TAB ? undefined : tabs.find((entry) => entry.id === id);
  if (!tab) return { app: "failed", message: `There is no tab ${id} in this session now.` };
  const users = !isAgentTab(tab) || isPaused(tab.id);
  const unsaved = hasUnsaved(tab);
  if (!confirmed && (users || unsaved || tab.kind === "terminal")) {
    return { app: "closeAsks", tab: id, title: tabTitle(tab, tabs), kind: tab.kind, users, unsaved };
  }
  closeTab(conversationId, id);
  return { app: "tabClosed", tab: id };
}

function hasUnsaved(tab: SessionTab): boolean {
  if (tab.kind !== "page") return false;
  if (tab.pageId === null) return tab.draft !== undefined && !documentIsSaved(tab.draft);
  return usePageEdits.getState().pages[tab.pageId]?.dirty === true;
}
