import { useCallback } from "react";
import { TerminalView } from "@/app/conversation/TerminalView";
import { changeSessionTab, type TerminalTabState } from "@/state/sessionTabs";
import { openMainTerminal } from "@/state/sessionTabResources";

export function MainTerminalTab({ conversationId, tab, active }: { conversationId: string; tab: TerminalTabState; active: boolean }) {
  const open = useCallback((cols: number, rows: number) => openMainTerminal(conversationId, tab.id, cols, rows), [conversationId, tab.id]);
  const onTitle = useCallback((title: string) => changeSessionTab(conversationId, tab.id,
    (current) => current.kind === "terminal" ? { ...current, title } : current), [conversationId, tab.id]);
  const onCwd = useCallback((cwd: string) => changeSessionTab(conversationId, tab.id,
    (current) => current.kind === "terminal" ? { ...current, cwd } : current), [conversationId, tab.id]);
  return <TerminalView open={open} onTitle={onTitle} onCwd={onCwd} focus={active} />;
}
