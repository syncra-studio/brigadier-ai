import assert from "node:assert/strict";
import { test } from "node:test";

Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null, setItem: () => {}, removeItem: () => {}, length: 0, key: () => null } });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto } });
const { openFromTerminal, serverAddresses } = await import("./devServers");

test("a dev server's address is found in what a terminal printed, colours and all", () => {
  assert.deepEqual(serverAddresses("  \u001b[32m➜\u001b[39m  Local:   \u001b[36mhttp://localhost:\u001b[1m5173\u001b[22m/\u001b[39m\r\n"), ["http://localhost:5173/"]);
  assert.deepEqual(serverAddresses("ready on http://127.0.0.1:3000/app and http://127.0.0.1:3000/app"), ["http://127.0.0.1:3000/app"]);
});

test("an address elsewhere isn't a dev server", () => {
  assert.deepEqual(serverAddresses("see https://example.com:3000/ and http://localhost.evil.example/"), []);
});

test("only the terminal's own BROWSER can open a page", () => {
  // No session in front, so nothing opens either way; a wrong token or no token mustn't even try.
  assert.doesNotThrow(() => openFromTerminal("wrong;http://localhost:5173/", "right"));
  assert.doesNotThrow(() => openFromTerminal("http://localhost:5173/", null));
});

test("the terminal's own BROWSER opens a local page in the session in front, and only that", async () => {
  const { useApp } = await import("./store");
  const { CHAT_TAB, closeTab, sessionTabs, useSessionTabs } = await import("./sessionTabs");
  useApp.setState((state) => ({
    settings: { ...state.settings, browserEnabled: true, localLinksIn: "brigadier" },
    info: { ...state.info, platform: "macos" } as typeof state.info,
    selection: { type: "conversation", id: "dev-session" },
    conversations: { "dev-session": { id: "dev-session", kind: "session" } } as unknown as typeof state.conversations,
  }));
  useSessionTabs.setState({ sessions: {} });
  const urls = () => sessionTabs("dev-session").tabs.flatMap((tab) => (tab.kind === "browser" ? [tab.url] : []));
  openFromTerminal("wrong;http://localhost:5173/", "right");
  openFromTerminal("http://localhost:5173/", "right");
  openFromTerminal("right;https://example.com/", "right");
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.deepEqual(urls(), []);
  openFromTerminal("right;http://localhost:5173/", "right");
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.deepEqual(urls(), ["http://localhost:5173/"]);
  for (const tab of sessionTabs("dev-session").tabs) if (tab.id !== CHAT_TAB) closeTab("dev-session", tab.id);
});
