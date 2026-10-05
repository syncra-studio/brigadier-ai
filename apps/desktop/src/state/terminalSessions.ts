import { create } from "zustand";
import { persist } from "zustand/middleware";

import { notePaneClose } from "@/state/closedPanes";
import { closeTerminal } from "@/state/terminals";

export type ShellSession = { id: string; title: string; cwd: string | null };
type Sessions = {
  sessions: ShellSession[];
  active: string | null;
  closed: ShellSession[];
};
const EMPTY: Sessions = { sessions: [], active: null, closed: [] };
export const useTerminalSessions = create<{
  conversations: Record<string, Sessions>;
}>()(
  persist(() => ({ conversations: {} }), {
    name: "brigadier.terminalSessions",
  }),
);

export function terminalSessions(id: string): Sessions {
  return useTerminalSessions.getState().conversations[id] ?? EMPTY;
}
function update(id: string, change: (current: Sessions) => Sessions): void {
  useTerminalSessions.setState(({ conversations }) => ({
    conversations: {
      ...conversations,
      [id]: change(conversations[id] ?? EMPTY),
    },
  }));
}
export function addTerminalSession(conversationId: string): string {
  const id = crypto.randomUUID();
  update(conversationId, (current) => ({
    ...current,
    sessions: [...current.sessions, { id, title: "Terminal", cwd: null }],
    active: id,
  }));
  return id;
}
export function selectTerminalSession(
  conversationId: string,
  id: string,
): void {
  update(conversationId, (current) => ({ ...current, active: id }));
}
export function describeTerminalSession(
  conversationId: string,
  id: string,
  cwd: string,
): void {
  update(conversationId, (current) => ({
    ...current,
    sessions: current.sessions.map((session) =>
      session.id === id
        ? {
            ...session,
            cwd,
            title: cwd.split(/[\\/]/).filter(Boolean).at(-1) ?? "Terminal",
          }
        : session,
    ),
  }));
}
export function removeTerminalSession(
  conversationId: string,
  id: string,
): void {
  closeTerminal(id);
  notePaneClose(conversationId, "terminal");
  update(conversationId, (current) => {
    const closed = current.sessions.find((session) => session.id === id);
    const index = current.sessions.findIndex((session) => session.id === id);
    const sessions = current.sessions.filter((session) => session.id !== id);
    return {
      sessions,
      active:
        current.active === id
          ? (sessions[Math.max(0, index - 1)]?.id ?? null)
          : current.active,
      closed: closed ? [...current.closed, closed] : current.closed,
    };
  });
}
export function undoTerminalClose(conversationId: string): boolean {
  const current = terminalSessions(conversationId);
  const previous = current.closed.at(-1);
  if (!previous) return false;
  const session = { ...previous, id: crypto.randomUUID() };
  update(conversationId, (value) => ({
    ...value,
    sessions: [...value.sessions, session],
    active: session.id,
    closed: value.closed.slice(0, -1),
  }));
  return true;
}
