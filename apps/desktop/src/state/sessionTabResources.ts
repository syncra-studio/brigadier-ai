import { forgetDocument, request } from "@/ipc/client";
import type { Conversation, TerminalInfo } from "@/ipc/generated";
import { closePage, useBrowsers } from "@/state/browsers";
import { discardDocument } from "@/state/documentDrafts";
import { changeSessionTab, discardTab, onDiscardTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { abandonedSideChats } from "@/state/sideChats";

// Lives across ConversationView remounts, but not a real app reload. The stable tab ID is
// the daemon session ID; its first attachment in this app lifetime replaces the old shell.
const shells = new Map<string, { terminal: Promise<TerminalInfo>; closed: boolean }>();
export async function openMainTerminal(conversationId: string, tabId: string, cols: number, rows: number): Promise<TerminalInfo> {
  const tab = sessionTabs(conversationId).tabs.find((candidate) => candidate.id === tabId);
  if (tab?.kind !== "terminal") throw new Error("Terminal tab closed");
  let entry = shells.get(tab.id);
  const started = !entry;
  if (!entry) {
    const terminal = request({ method: "openTerminal", conversationId, sessionId: tab.id, fresh: true,
      ...(tab.cwd ? { cwd: tab.cwd } : {}), cols, rows }).then((opened) => {
        changeSessionTab(conversationId, tab.id, (current) => current.kind === "terminal" ? { ...current, cwd: opened.terminal.cwd } : current);
        return opened.terminal;
      });
    entry = { terminal, closed: false };
    shells.set(tab.id, entry);
    terminal.catch(() => { if (shells.get(tab.id) === entry) shells.delete(tab.id); });
  }
  const terminal = await entry.terminal;
  if (entry.closed || !sessionTabs(conversationId).tabs.some((current) => current.id === tab.id)) {
    await request({ method: "closeTerminal", terminalId: terminal.id });
    throw new Error("Terminal tab closed");
  }
  // A shell started here streams its first output to the view; attaching again would show
  // that output twice, once in the scrollback and once from the stream.
  if (started) return terminal;
  // Attach this connection and get current scrollback when its session becomes visible again.
  const attached = await request({ method: "openTerminal", conversationId, sessionId: tab.id, ...(tab.cwd ? { cwd: tab.cwd } : {}), cols, rows });
  if (entry.closed || !sessionTabs(conversationId).tabs.some((current) => current.id === tab.id)) {
    await request({ method: "closeTerminal", terminalId: attached.terminal.id });
    throw new Error("Terminal tab closed");
  }
  return attached.terminal;
}

function closeShell(id: string): void {
  const entry = shells.get(id);
  if (!entry) return;
  entry.closed = true;
  shells.delete(id);
  void entry.terminal.then((terminal) => request({ method: "closeTerminal", terminalId: terminal.id })).catch(console.error);
}

useSessionTabs.subscribe((state, before) => {
  for (const [id, previous] of Object.entries(before.sessions)) {
    const kept = state.sessions[id]?.tabs ?? [];
    for (const tab of previous.tabs) if (!kept.some((current) => current.id === tab.id)) {
      if (tab.kind === "terminal") closeShell(tab.id);
      if (tab.kind === "browser") closePage(tab.id);
    }
  }
});

useBrowsers.subscribe(({ pages }) => {
  for (const [id, session] of Object.entries(useSessionTabs.getState().sessions)) {
    for (const tab of session.tabs) {
      const page = pages[tab.id];
      // A navigated New tab keeps its ID; resource ownership follows its kind, not the prefix.
      if (tab.kind === "browser" && page && (tab.url !== page.url || tab.title !== page.title))
        changeSessionTab(id, tab.id, (current) => current.kind === "browser" ? { ...current, url: page.url, title: page.title } : current);
    }
  }
});

function deleteSideChat(id: string): void {
  void request({ method: "delete", ids: [id] }).catch(console.error);
}
onDiscardTab((tab) => {
  if (tab.kind === "document") {
    discardDocument(tab.id);
    void forgetDocument(tab.id).catch(console.error);
  }
  if (tab.kind === "sideChat" && tab.conversationId && !Object.values(useSessionTabs.getState().sessions)
    .some((session) => session.tabs.some((entry) => entry.kind === "sideChat" && entry.conversationId === tab.conversationId)))
    deleteSideChat(tab.conversationId);
});
let pruned = false;
export function pruneSideChats(conversations: Conversation[]): void {
  // Called by loadCatalog only after the conversation list has loaded.
  if (pruned) return;
  pruned = true;
  const referenced = new Set(Object.values(useSessionTabs.getState().sessions).flatMap((session) =>
    session.tabs.flatMap((tab) => tab.kind === "sideChat" && tab.conversationId ? [tab.conversationId] : [])));
  for (const id of abandonedSideChats(conversations, referenced)) deleteSideChat(id);
}

useApp.subscribe(({ conversations }) => {
  for (const [id, session] of Object.entries(useSessionTabs.getState().sessions)) {
    if (conversations[id]?.lifecycle === "archived") {
      const tabs = session.tabs.filter((tab) => tab.kind !== "terminal" && tab.kind !== "sideChat");
      if (tabs.length !== session.tabs.length) {
        useSessionTabs.setState((state) => ({ sessions: { ...state.sessions, [id]: {
          tabs, active: tabs.some((tab) => tab.id === session.active) ? session.active : "chat",
        } } }));
        for (const tab of session.tabs) if (tab.kind === "terminal" || tab.kind === "sideChat") discardTab(tab);
      }
    }
  }
});
