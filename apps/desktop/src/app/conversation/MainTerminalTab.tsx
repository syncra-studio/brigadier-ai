import { useCallback, useState } from "react";
import { TerminalView } from "@/app/conversation/TerminalView";
import { Button } from "@/components/ui/button";
import { changeSessionTab, type TerminalTabState } from "@/state/sessionTabs";
import { endMainTerminal, openMainTerminal } from "@/state/sessionTabResources";

export function MainTerminalTab({ conversationId, tab, active }: { conversationId: string; tab: TerminalTabState; active: boolean }) {
  // Restart remounts the view, which opens a new shell.
  const [attempt, setAttempt] = useState(0);
  const [exited, setExited] = useState(false);
  const open = useCallback(async (cols: number, rows: number) => {
    const terminal = await openMainTerminal(conversationId, tab.id, cols, rows);
    // A reconnect reopens the view on a new shell: the old one's exit no longer applies.
    setExited(false);
    return terminal;
  }, [conversationId, tab.id]);
  const onExit = useCallback(() => setExited(true), []);
  const onTitle = useCallback((title: string) => changeSessionTab(conversationId, tab.id,
    (current) => current.kind === "terminal" ? { ...current, title } : current), [conversationId, tab.id]);
  const onCwd = useCallback((cwd: string) => changeSessionTab(conversationId, tab.id,
    (current) => current.kind === "terminal" ? { ...current, cwd } : current), [conversationId, tab.id]);
  const restart = () => {
    endMainTerminal(tab.id);
    setExited(false);
    setAttempt((count) => count + 1);
  };
  return <>
    {exited && <div className="border-border flex shrink-0 items-center gap-2 border-b px-4 py-2">
      <span role="status" className="text-muted-foreground min-w-0 flex-1 truncate text-sm">The shell exited.</span>
      <Button size="xs" variant="ghost" onClick={restart}>Restart</Button>
    </div>}
    <TerminalView key={attempt} open={open} onExit={onExit} onTitle={onTitle} onCwd={onCwd} focus={active} />
  </>;
}
