/**
 * An agent's Browser tab taken over by the user, over a faked shell: the strip that says the
 * agent waits, its Continue, and the toolbar's own buttons pausing it too. ?drive=1 drives it
 * and writes what it saw to #agent-paused-result (AgentPausedRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTab } from "@/app/conversation/BrowserTab";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Request } from "@/ipc/generated";
import { userUsed } from "@/state/agentPause";
import { useBrowsers } from "@/state/browsers";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { fixtureClick } from "./input";

const params = new URLSearchParams(location.search);
const ID = "browser:agent-1";
const MINE = "browser:mine";

if (params.has("drive")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, browserFullUrl: false },
  selection: { type: "conversation", id: "session" },
}));
const page = (url: string) => ({ url, title: "Shop", loading: false, ready: true, blocked: null });
useBrowsers.setState({ pages: { [ID]: page("https://shop.example/cart"), [MINE]: page("https://news.example/") } });
useSessionTabs.setState({
  sessions: {
    session: {
      tabs: [
        { kind: "browser", id: ID, opener: CHAT_TAB, url: "https://shop.example/cart", title: "", agent: true, driver: { label: "Check the shop", taskId: "t1" } },
        { kind: "browser", id: MINE, opener: CHAT_TAB, url: "https://news.example/", title: "" },
      ],
      active: ID,
    },
  },
});

const told: { tab: string; paused: boolean }[] = [];
mockIPC((command, payload) => {
  const args = (payload ?? {}) as { request?: Request };
  if (command === "ipc_request" && args.request?.method === "browserPaused") {
    told.push({ tab: args.request.tab, paused: args.request.paused });
    return { method: "browserPaused" };
  }
  return null;
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const strip = () => document.querySelector<HTMLElement>('[data-slot="browser-paused"]');
const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((each) => each.textContent?.trim() === label || each.getAttribute("aria-label") === label) ?? null;

function Fixture() {
  const [result, setResult] = useState("");
  const front = useSessionTabs((state) => state.sessions.session?.active);
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      await wait(200);
      const seen: Record<string, unknown> = { before: strip() !== null };
      // The page's input script heard the user.
      userUsed(ID);
      await wait(50);
      const shown = strip();
      seen.paused = {
        text: shown?.textContent,
        role: shown?.getAttribute("role"),
        height: Math.round(shown?.getBoundingClientRect().height ?? 0),
        continue: button("Continue") !== null,
      };
      await fixtureClick(button("Continue")!);
      await wait(50);
      seen.continued = strip() === null;
      // The toolbar is the user's input too.
      await fixtureClick(document.querySelector('[data-slot="browser-navigation"] button:nth-of-type(3)') ?? button("Reload")!);
      await wait(50);
      seen.reloaded = strip() !== null;
      await fixtureClick(button("Continue")!);
      await wait(50);
      // The user's own tab has no agent to pause.
      userUsed(MINE);
      useSessionTabs.setState(({ sessions }) => ({ sessions: { ...sessions, session: { ...sessions.session!, active: MINE } } }));
      await wait(50);
      seen.mine = strip() === null;
      seen.told = told;
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1 flex-col">
          {front && <BrowserTab key={front} conversationId={front} active />}
        </main>
      </div>
      {result && <pre id="agent-paused-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
