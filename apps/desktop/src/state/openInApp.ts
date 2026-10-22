import type { AppAnswer, DiffScope, OpenTarget } from "@/ipc/generated";
import {
  isAgentTab,
  openBrowserTab,
  openFileTab,
  openForAgent,
  openPageTab,
  openPanelTab,
  openReviewTab,
  openTerminalTab,
  REVIEW_TAB,
  type ReviewTarget,
  sameAddress,
  type SessionTab,
  sessionTabs,
} from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { askWorkerPanel } from "@/state/workerPanel";

/**
 * `open_in_app` (Q90): opens what the daemon checked, as the user's click would. In front only
 * when the user asked to see it (`show`) and is on the session; else behind the tab they are on,
 * marked new. A tab that already shows it and is the user's own, or the one in front, is left
 * as it was when it would open behind. Never brings the window forward.
 */
export function openInApp(conversationId: string, target: OpenTarget, show: boolean): AppAnswer {
  const state = useApp.getState();
  const onScreen = state.windowVisible && state.selection.type === "conversation" && state.selection.id === conversationId;
  const front = show && onScreen;
  if (target.kind === "worker") {
    if (!front) return failed("A worker's panel opens only in front, when the user asked to see it and is on this session; it wasn't opened.");
    askWorkerPanel(conversationId, target.task);
    return { app: "opened", tab: null, front: true, kept: false };
  }
  if (target.kind === "url" && !state.settings.browserEnabled) return failed("The built-in browser is off in Settings › Browser.");
  const current = sessionTabs(conversationId);
  const there = current.tabs.find((tab) => shows(tab, target));
  if (there && !front && (there.id === current.active || !isAgentTab(there)))
    return { app: "opened", tab: there.id, front: false, kept: true };
  const tab = openForAgent(conversationId, front, () => open(conversationId, target, there));
  return { app: "opened", tab, front, kept: false };
}

function open(conversationId: string, target: Exclude<OpenTarget, { kind: "worker" }>, there: SessionTab | undefined): string {
  switch (target.kind) {
    case "file":
      openFileTab(conversationId, target.path, { line: target.line });
      return `file:${target.path}`;
    case "diff":
      openReviewTab(conversationId, diffTarget(target.scope));
      return REVIEW_TAB;
    case "output":
      return openPanelTab(conversationId, { type: "output", source: target.source, id: target.id }, target.title);
    case "url":
      // The tab already showing the page, not another one of it (#418).
      return there?.id ?? openBrowserTab(conversationId, target.url, true);
    case "page":
      return openPageTab(conversationId, target.id, target.title);
    case "terminal":
      return openTerminalTab(conversationId, target.tab, target.cwd);
  }
}

/** Whether `tab` is where `target` opens: the same file, diff, panel or page, or a Browser tab on the same address. */
function shows(tab: SessionTab, target: OpenTarget): boolean {
  switch (target.kind) {
    case "file": return tab.id === `file:${target.path}`;
    case "diff": return tab.id === REVIEW_TAB;
    case "output": return tab.id === `panel:output:${target.source}:${target.id}`;
    case "url": return tab.kind === "browser" && sameAddress(tab.url, target.url);
    case "page": return tab.kind === "page" && tab.pageId === target.id;
    case "terminal": return tab.id === target.tab;
    case "worker": return false;
  }
}

function diffTarget(scope: DiffScope): ReviewTarget {
  switch (scope) {
    case "lastTurn": return { type: "lastTurn", requestId: null, staged: false };
    case "branch": return { type: "branch", staged: false };
    case "uncommitted": return { type: "uncommitted", staged: false };
  }
}

function failed(message: string): AppAnswer {
  return { app: "failed", message };
}
