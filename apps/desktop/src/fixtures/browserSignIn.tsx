/**
 * An agent's Browser tab at pages only the user goes through, over a faked shell: the strip's words for each stop and
 * its Continue, Google's refusal card with what it offers, and the passkey banner. ?drive=1 drives it and writes what
 * it saw to #browser-sign-in-result (BrowserSignInRender.test.ts reads it).
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTab } from "@/app/side-panel/BrowserTab";
import { TooltipProvider } from "@/components/overlays";
import type { Request } from "@/ipc/generated";
import { isPaused } from "@/state/agentPause";
import { useBrowsers } from "@/state/browsers";
import { signInSeen, type SignInStop, stopAgent } from "@/state/browserSignIn";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { fixtureClick } from "./input";

const params = new URLSearchParams(location.search);
const ID = "browser:agent-1";

const still = document.createElement("style");
still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
document.head.append(still);

useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, browserFullUrl: false },
  selection: { type: "conversation", id: "session" },
}));
useBrowsers.setState({ pages: { [ID]: { url: "https://accounts.google.com/", title: "Sign in", loading: false, ready: true, blocked: null } } as never });
useSessionTabs.setState({
  sessions: {
    session: {
      tabs: [{ kind: "browser", id: ID, opener: CHAT_TAB, url: "https://accounts.google.com/", title: "", agent: true, driver: { label: "Check the shop", taskId: "t1" } }],
      active: ID,
    },
  },
} as never);

const told: { tab: string; paused: boolean }[] = [];
const opened: string[] = [];
mockIPC((command, payload) => {
  const args = (payload ?? {}) as { request?: Request; url?: string };
  if (command === "open_url") opened.push(args.url ?? "");
  if (command === "ipc_request" && args.request?.method === "browserPaused") {
    told.push({ tab: args.request.tab, paused: args.request.paused });
    return { method: "browserPaused" };
  }
  return null;
});

const wait = (ms = 50) => new Promise((resolve) => setTimeout(resolve, ms));
const slot = (name: string) => document.querySelector<HTMLElement>(`[data-slot="${name}"]`);
const buttonIn = (within: HTMLElement | null, label: string) =>
  [...(within?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) => each.textContent?.trim() === label || each.getAttribute("aria-label") === label) ?? null;
const words = (name: string) => slot(name)?.querySelector('[data-slot="banner-text"]')?.textContent ?? null;

const STOPS: Record<string, SignInStop> = {
  signIn: { kind: "signIn", site: "accounts.google.com", key: "signIn https://accounts.google.com" },
  window: { kind: "signIn", site: "accounts.google.com", key: "signIn window", window: true },
  captcha: { kind: "captcha", site: "shop.example", key: "captcha https://shop.example/" },
  rejected: { kind: "rejected", site: "accounts.google.com", key: "rejected https://accounts.google.com" },
};

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (!params.has("drive")) return;
    void (async () => {
      await wait(200);
      const seen: Record<string, unknown> = { before: slot("browser-paused") !== null };

      const stops: Record<string, unknown> = {};
      for (const [name, stop] of Object.entries(STOPS)) {
        stopAgent(ID, stop);
        await wait();
        const strip = slot("browser-paused");
        const carryOn = buttonIn(strip, "Continue");
        stops[name] = { words: words("browser-paused"), role: strip?.getAttribute("role"), continues: carryOn !== null && !carryOn.disabled };
        await fixtureClick(carryOn!);
        await wait();
        stops[`${name}Continued`] = slot("browser-paused") === null && !isPaused(ID);
      }
      seen.stops = stops;
      seen.told = told;

      signInSeen(ID, { rejected: true });
      await wait();
      const card = slot("browser-signin-rejected");
      seen.card = { words: words("browser-signin-rejected"), role: card?.getAttribute("role"), height: Math.round(card?.getBoundingClientRect().height ?? 0) };
      seen.cardPaused = isPaused(ID);
      await fixtureClick(buttonIn(card, "Open in browser")!);
      await wait();
      seen.opened = [...opened];
      seen.cardAfterOpen = slot("browser-signin-rejected") !== null;
      await fixtureClick(buttonIn(slot("browser-signin-rejected"), "Dismiss")!);
      await wait();
      seen.dismissed = slot("browser-signin-rejected") === null;
      signInSeen(ID, { rejected: true });
      await wait();
      seen.again = slot("browser-signin-rejected") !== null;
      signInSeen(ID, { rejected: false });
      await wait();
      seen.pageLeft = slot("browser-signin-rejected") === null;

      // A popup's passkey refusal arrives as its tab's own event (tabLayer.ts, main/browserSignIn.ts).
      useBrowsers.setState(({ pages }) => ({ pages: { ...pages, [ID]: { ...pages[ID]!, passkeyRefused: true } } }));
      await wait();
      seen.passkey = words("browser-passkey");
      await fixtureClick(buttonIn(slot("browser-passkey"), "Dismiss")!);
      await wait();
      seen.passkeyDismissed = slot("browser-passkey") === null;

      setResult(JSON.stringify(seen));
    })();
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex h-full min-w-0 flex-1 flex-col">
          <BrowserTab conversationId={ID} active />
        </main>
      </div>
      {result && <pre id="browser-sign-in-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
