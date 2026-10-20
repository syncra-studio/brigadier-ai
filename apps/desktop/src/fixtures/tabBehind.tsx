/**
 * A session's Browser tab in front over its conversation, then the conversation brought back
 * (#417): every one of the tab's buttons hides with it at once, none lingers over the thread
 * while its transition would run. ?drive=1 drives it and writes what it saw to
 * #tab-behind-result (TabBehindRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SessionTabViews } from "@/app/conversation/SessionTabBar";
import { TooltipProvider } from "@/components/overlays";
import { useBrowsers } from "@/state/browsers";
import { CHAT_TAB, selectTab, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const SESSION = "session";
const TAB = "browser:todo";

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, browserFullUrl: false },
  selection: { type: "conversation", id: SESSION },
}));
useBrowsers.setState({ pages: { [TAB]: { url: "http://localhost:3000/", title: "Todo", loading: false, ready: true, blocked: null } } });
useSessionTabs.setState({
  sessions: { [SESSION]: { tabs: [{ kind: "browser", id: TAB, opener: CHAT_TAB, url: "http://localhost:3000/", title: "Todo" }], active: TAB } },
});
mockIPC(() => null);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));
/** The Browser tab's toolbar buttons that would show now. */
const showing = () =>
  [...document.querySelectorAll<HTMLElement>('[data-slot="browser-toolbar"] button')]
    .filter((button) => getComputedStyle(button).visibility === "visible")
    .map((button) => button.getAttribute("aria-label") ?? button.textContent?.trim() ?? "");

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      // The toolbar draws once its tab's chunk loads: slow under load, so wait for it.
      for (const end = performance.now() + 10_000; showing().length < 4 && performance.now() < end; ) await wait(16);
      const seen: Record<string, unknown> = { front: showing().length };
      selectTab(SESSION, CHAT_TAB);
      await frame();
      await frame();
      seen.behind = showing();
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="relative flex h-full min-w-0 flex-1 flex-col">
          <p className="p-6">The session's thread.</p>
          <SessionTabViews conversationId={SESSION} />
        </main>
      </div>
      {result && <pre id="tab-behind-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
