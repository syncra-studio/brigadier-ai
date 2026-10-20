import { useCallback, useState } from "react";
import { TerminalView } from "@/components/views/TerminalView";
import { Banner } from "@/components/shared/banner";
import { Button } from "@/components/controls/button";
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
    {exited && <Banner text="The shell exited.">
      <Button size="xs" variant="ghost" onClick={restart}>Restart</Button>
    </Banner>}
    <TerminalView key={attempt} open={open} onExit={onExit} onTitle={onTitle} onCwd={onCwd} focus={active} />
  </>;
}
