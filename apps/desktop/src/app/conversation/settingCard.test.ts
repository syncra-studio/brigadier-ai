import assert from "node:assert/strict";
import { test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Conversation, PermissionLevel, Request, SettingChange } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
  get length() { return data.size; },
  key: (at: number) => [...data.keys()][at] ?? null,
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { isMade, makeChange, settingPage, settingWords } = await import("./settingCard");
const { useApp } = await import("@/state/store");

const LABELS = { askForApproval: "Ask for approval", approveForMe: "Approve for me", fullAccess: "Full access" } as const;
const session = { id: "s1", setup: { type: "session", permission: "askForApproval" } } as unknown as Conversation;

test("each suggested change reads in plain words and opens its own Settings page", () => {
  assert.deepEqual(settingWords({ setting: "permission", level: "approveForMe" }, LABELS), {
    title: "Switch this session to Approve for me", button: "Switch to Approve for me", done: "This session is on Approve for me.",
  });
  assert.equal(settingWords({ setting: "browser", on: true }, LABELS).title, "Turn on the built-in browser");
  assert.equal(settingWords({ setting: "mergeOnOwn", on: false }, LABELS).button, "Turn off");
  assert.deepEqual((["permission", "browser", "mergeOnOwn"] as const).map((setting) =>
    settingPage(setting === "permission" ? { setting, level: "approveForMe" } : { setting, on: true })), ["configuration", "browser", "git"]);
});

test("a suggested change applies only on its press, and then reads as made", async () => {
  const sent: Request[] = [];
  mockIPC((command, payload) => {
    const request = (payload as { request: Request }).request;
    if (command !== "ipc_request") return null;
    sent.push(request);
    if (request.method === "updateSettings") {
      useApp.setState({ settings: request.settings });
      return { method: "updateSettings", settings: request.settings };
    }
    return { method: request.method };
  });
  useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: false, autoMerge: false } }));
  const browser: SettingChange = { setting: "browser", on: true };
  const merge: SettingChange = { setting: "mergeOnOwn", on: true };
  assert.equal(isMade(browser, useApp.getState().settings, session), false);
  assert.deepEqual(sent, [], "showing the card changes nothing");

  await makeChange(browser, session, async () => assert.fail("not a permission change"));
  assert.equal(useApp.getState().settings.browserEnabled, true);
  assert.equal(useApp.getState().settings.autoMerge, false, "only the pressed change is made");
  assert.equal(isMade(browser, useApp.getState().settings, session), true);
  await makeChange(merge, session, async () => assert.fail("not a permission change"));
  assert.equal(useApp.getState().settings.autoMerge, true);

  const switched: [string, PermissionLevel][] = [];
  const level: SettingChange = { setting: "permission", level: "approveForMe" };
  assert.equal(isMade(level, useApp.getState().settings, session), false);
  await makeChange(level, session, async (conversation, to) => { switched.push([conversation.id, to]); });
  assert.deepEqual(switched, [["s1", "approveForMe"]], "the session's own level, as its picker sets it");
});
