import assert from "node:assert/strict";
import test from "node:test";

type GuestLogin = { from: number; kind: string; id?: string; profile?: string; site?: string; username?: string; update?: boolean };

const vaultCalls: [string, Record<string, unknown>][] = [];
const answered: Record<string, unknown>[] = [];
let tell: (details: GuestLogin) => void = () => {};

const stored = new Map<string, string>();
const localStorage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => void stored.set(key, value),
  removeItem: (key: string) => void stored.delete(key),
};
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: localStorage });

// What the preload gives the page. An answer goes through `@/ipc/native`, so that door is here too.
(globalThis as { window?: unknown }).window = {
  localStorage,
  brigadierShell: {
    vault: async (op: string, args: Record<string, unknown>) => {
      vaultCalls.push([op, args]);
    },
    onGuestLogin: (listener: typeof tell) => {
      tell = listener;
    },
  },
  brigadierNative: {
    invoke: async (command: string, args: Record<string, unknown>) => {
      if (command === "browser_password_offer_answer") answered.push(args);
      return { value: command === "ipc_request" ? { method: (args.request as { method: string }).method } : null };
    },
    onChannel: () => {},
    listen: () => () => {},
  },
};

const { handBack, isDriven, userUsed, useAgentPause } = await import("@/state/agentPause");
const { useSaveOffers } = await import("@/state/browserPasswords");
const { useSessionTabs } = await import("@/state/sessionTabs");
const { installGuestLogins } = await import("./guestLogins.ts");

const GUESTS: Record<number, string | null> = { 1: "mine", 2: "agent", 3: "agent-two", 9: null };
const browserTab = (id: string, agent: boolean) => ({
  kind: "browser",
  id,
  opener: "chat",
  url: "https://site.test/login",
  title: "",
  ...(agent ? { agent: true, driver: { label: "An agent", taskId: "t1" } } : {}),
});

function reset(): void {
  useSessionTabs.setState({
    sessions: { session: { tabs: [browserTab("mine", false), browserTab("agent", true), browserTab("agent-two", true)], active: "mine" } },
  } as never);
  useAgentPause.setState({ paused: {}, devtools: {} });
  useSaveOffers.setState({ offers: [] });
  vaultCalls.length = 0;
  answered.length = 0;
}

const settle = () => new Promise((resolve) => setImmediate(resolve));
const offerFrom = (from: number, id: string): GuestLogin => ({ from, kind: "offer", id, profile: "work", site: "https://site.test", username: "fake-sam", update: false });
const fills = () => vaultCalls.filter(([op]) => op === "fill").map(([, args]) => args);

installGuestLogins((guest) => GUESTS[guest] ?? null);

test("the tabs here are what they say", () => {
  reset();
  assert.equal(isDriven("mine"), false);
  assert.equal(isDriven("agent"), true);
});

test("the user's own tab fills when its boxes show", () => {
  reset();
  tell({ from: 1, kind: "fields" });
  assert.deepEqual(fills(), [{ guest: 1 }]);
});

test("a tab an agent drives doesn't fill", () => {
  reset();
  tell({ from: 2, kind: "fields" });
  assert.deepEqual(fills(), []);
});

test("a guest with no tab doesn't fill", () => {
  reset();
  tell({ from: 9, kind: "fields" });
  assert.deepEqual(fills(), []);
});

test("taking the tab over fills what it couldn't, once", () => {
  reset();
  tell({ from: 2, kind: "fields" });
  userUsed("agent");
  assert.equal(isDriven("agent"), false);
  assert.deepEqual(fills(), [{ guest: 2 }]);
  useAgentPause.setState((state) => ({ ...state }));
  assert.deepEqual(fills(), [{ guest: 2 }], "nothing still waits");
});

test("a paused tab fills as the user's", () => {
  reset();
  userUsed("agent");
  tell({ from: 2, kind: "fields" });
  assert.deepEqual(fills(), [{ guest: 2 }]);
});

test("taking over one tab doesn't fill another an agent still drives", () => {
  reset();
  tell({ from: 2, kind: "fields" });
  tell({ from: 3, kind: "fields" });
  userUsed("agent");
  assert.deepEqual(fills(), [{ guest: 2 }]);
});

test("handing the tab back fills nothing more", () => {
  reset();
  userUsed("agent");
  tell({ from: 2, kind: "fields" });
  handBack("agent");
  assert.equal(isDriven("agent"), true);
  assert.deepEqual(fills(), [{ guest: 2 }]);
});

test("the user's own tab shows an offer, and nothing goes to main", () => {
  reset();
  tell(offerFrom(1, "o1"));
  assert.deepEqual(useSaveOffers.getState().offers, [{ id: "o1", tab: "mine", profile: "work", site: "https://site.test", username: "fake-sam", update: false }]);
  assert.deepEqual(vaultCalls, []);
});

test("an offer from a tab an agent drives is refused at once, so main drops the password", () => {
  reset();
  tell(offerFrom(2, "o2"));
  assert.deepEqual(useSaveOffers.getState().offers, []);
  assert.deepEqual(vaultCalls, [["answer", { offer: "o2", answer: "notNow" }]]);
});

test("an offer from a guest with no tab is refused too", () => {
  reset();
  tell(offerFrom(9, "o3"));
  assert.deepEqual(useSaveOffers.getState().offers, []);
  assert.deepEqual(vaultCalls, [["answer", { offer: "o3", answer: "notNow" }]]);
});

test("a paused tab gets its offer, and Continue drops it", async () => {
  reset();
  userUsed("agent");
  tell(offerFrom(2, "o4"));
  assert.deepEqual(useSaveOffers.getState().offers.map(({ id }) => id), ["o4"]);
  handBack("agent");
  await settle();
  assert.deepEqual(useSaveOffers.getState().offers, []);
  assert.deepEqual(answered, [{ offer: "o4", answer: "notNow" }]);
});

test("Continue on one tab leaves another tab's offer", async () => {
  reset();
  tell(offerFrom(1, "mine-offer"));
  userUsed("agent");
  tell(offerFrom(2, "agent-offer"));
  handBack("agent");
  await settle();
  assert.deepEqual(useSaveOffers.getState().offers.map(({ id }) => id), ["mine-offer"]);
  assert.deepEqual(answered, [{ offer: "agent-offer", answer: "notNow" }]);
});

test("main saying an offer's gone takes it away", () => {
  reset();
  tell(offerFrom(1, "o5"));
  tell({ from: 1, kind: "offer-gone", id: "o5" });
  assert.deepEqual(useSaveOffers.getState().offers, []);
});
