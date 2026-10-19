import type { TabView, WindowView } from "@/ipc/generated";
import { useRightSidebarState } from "@/state/rightSidebar";
import { isAgentTab, type SessionTab, sessionTabs } from "@/state/sessionTabs";
import { tabTitle } from "@/state/sessionTabTitle";
import { terminalPlace } from "@/state/terminalPlaces";
import { useApp } from "@/state/store";
import { useWorkspacePaneState } from "@/state/workspacePane";

/**
 * What the window shows of a session (Q90's `app_state`): whether it is on screen, the tab in
 * front, its tabs (each the agent's or the user's) and the panels open. Read only: nothing here
 * changes what the user sees.
 */
export function windowView(conversationId: string): WindowView {
  const state = useApp.getState();
  const onScreen = state.windowVisible && state.selection.type === "conversation" && state.selection.id === conversationId;
  const { tabs, active } = sessionTabs(conversationId);
  return {
    onScreen,
    front: active,
    tabs: tabs.map((tab) => tabView(tab, tabs)),
    panels: {
      // The files pane and Workers are the window's, shown with whichever session is in front.
      files: onScreen && useWorkspacePaneState.getState().open,
      terminal: terminalPlace(`conv:${conversationId}`).open,
      workers: onScreen && useRightSidebarState.getState().open,
      context: state.summaryShown[conversationId] ?? false,
    },
  };
}

function tabView(tab: SessionTab, tabs: readonly SessionTab[]): TabView {
  return {
    id: tab.id,
    kind: tab.kind,
    title: tabTitle(tab, tabs),
    agent: isAgentTab(tab),
    path: tab.kind === "file" || tab.kind === "conflict" ? tab.path : tab.kind === "review" && tab.target.type === "file" ? tab.target.path : null,
    line: tab.kind === "file" ? tab.line : null,
    url: tab.kind === "browser" ? tab.url || null : null,
  };
}
