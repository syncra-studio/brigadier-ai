import assert from "node:assert/strict";
import { test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { CatalogRow, CatalogView, Request } from "@/ipc/generated";
import { catalogPage, livePlugins } from "./live";
import { connectPlugins, usePlugins } from "./adapter";
import { lineOf, signedIn, signingIn } from "./model";

const server = (id: string): CatalogRow => ({
  id, kind: "server", name: id, differentPerAgent: false,
  choice: { on: true, scope: ["/fixture/project"] }, signIn: { type: "signedOut" },
  definitions: [{ agent: "claude", file: "/fixture/project/.mcp.json", plugin: null,
    project: "/fixture/project", native: id, aliases: [id], fingerprint: id,
    transport: "http", endpoint: `https://${id}.test/mcp` }],
});
const catalog = (...rows: CatalogRow[]): CatalogView => ({ rows, version: "test" });
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

function mockedWindow() {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    crypto: globalThis.crypto, addEventListener: () => {}, removeEventListener: () => {},
  } });
  return () => {
    clearMocks();
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else Reflect.deleteProperty(globalThis, "window");
  };
}

test("the first catalog shows rows before independent sign-in reads finish", async () => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  const seen: string[] = [];
  const pending: Array<(value: unknown) => void> = [];
  let catalogReply!: (value: unknown) => void;
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return new Promise((resolve) => { catalogReply = resolve; });
    if (request.method === "getPluginSignInStatus") {
      seen.push(request.rowId);
      return new Promise((resolve) => { pending.push(resolve); });
    }
    throw new Error(`Unexpected request ${request.method}`);
  });
  const disconnect = connectPlugins(livePlugins());
  try {
    assert.equal(usePlugins.getState().loading, true);
    catalogReply({ method: "getPluginCatalog", catalog: catalog(server("one"), server("two")) });
    await tick();
    assert.deepEqual(seen, ["one", "two"], "neither status read waits for the other");
    assert.equal(usePlugins.getState().loading, false);
    assert.equal(usePlugins.getState().view.items.length, 2);
    pending[0]!({ method: "getPluginSignInStatus", state: { type: "signedIn" } });
    await tick();
    assert.equal(usePlugins.getState().loading, false);
    pending[1]!({ method: "getPluginSignInStatus", state: { type: "signedOut" } });
    await tick();
    assert.equal(usePlugins.getState().loading, false);
    assert.equal(usePlugins.getState().view.items.length, 2);
    assert.deepEqual(usePlugins.getState().view.items[0]!.signIn, { type: "signedIn" });
  } finally {
    disconnect();
    usePlugins.setState(previous);
    restore();
  }
});

test("project scopes from the catalog render as projects, including when switched off", () => {
  const row = server("proj-everything");
  const projects = [{ id: "project", name: "Project", path: "/fixture/project", notes: [] }];
  for (const on of [true, false]) {
    row.choice.on = on;
    const view = catalogPage(catalog(row), projects);
    assert.deepEqual(view.items[0]!.scope, { type: "someProjects", projectIds: ["project"], folders: [] });
    assert.equal(view.items[0]!.on, on);
  }
});

test("reconnecting keeps cached rows and sign-in state visible through a failed background read", async () => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return { method: request.method, catalog: catalog(server("cached")) };
    return { method: request.method, state: { type: "signedIn" } };
  });
  let disconnect = connectPlugins(livePlugins());
  try {
    await tick();
    assert.equal(usePlugins.getState().hasLoaded, true);
    disconnect();
    let fail!: (error: Error) => void;
    mockIPC(() => new Promise((_resolve, reject) => { fail = reject; }));
    disconnect = connectPlugins(livePlugins());
    assert.equal(usePlugins.getState().loading, false);
    assert.equal(usePlugins.getState().view.items[0]!.key, "cached");
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedIn");
    fail(new Error("Temporarily offline"));
    await tick();
    assert.equal(usePlugins.getState().loading, false);
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedIn");
    assert.equal(usePlugins.getState().error, "Temporarily offline");
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("an empty successful catalog is cached too", async () => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  mockIPC(() => ({ method: "getPluginCatalog", catalog: catalog() }));
  let disconnect = connectPlugins(livePlugins());
  try {
    await tick();
    disconnect();
    mockIPC(() => new Promise(() => {}));
    disconnect = connectPlugins(livePlugins());
    assert.equal(usePlugins.getState().loading, false);
    assert.deepEqual(usePlugins.getState().view.items, []);
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("a rebuilt daemon catalog is fetched in the background without showing loading", async (t) => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  t.mock.timers.enable({ apis: ["setInterval"] });
  let current = { ...catalog(server("one")), version: "first" };
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    return request.method === "getPluginCatalog" ? { method: request.method, catalog: structuredClone(current) }
      : { method: request.method, state: { type: "signedIn" } };
  });
  const disconnect = connectPlugins(livePlugins());
  try {
    await tick();
    current = { ...catalog(server("one"), server("two")), version: "second" };
    t.mock.timers.tick(1000);
    await tick();
    assert.equal(usePlugins.getState().loading, false);
    assert.equal(usePlugins.getState().view.items.length, 2);
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("sign-in follows shared state and updates the grid within 500 ms", async (t) => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  t.mock.timers.enable({ apis: ["setInterval"] });
  let status: CatalogRow["signIn"] = { type: "signedOut" };
  let reads = 0;
  let polls = 0;
  let failNextPoll = false;
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") {
      reads++;
      return { method: request.method, catalog: catalog(server("one")) };
    }
    if (request.method === "startPluginSignIn") {
      status = { type: "signingIn", flowId: "fixture-flow" };
      return { method: request.method, state: status };
    }
    if (request.method === "getPluginSignInStatus") {
      polls++;
      if (failNextPoll) { failNextPoll = false; throw new Error("Temporary disconnect"); }
      return { method: request.method, state: status };
    }
    throw new Error(`Unexpected request ${request.method}`);
  });
  const adapter = livePlugins();
  const disconnect = connectPlugins(adapter);
  try {
    await tick();
    await adapter.signIn!("one", "one", null);
    assert.equal(lineOf(usePlugins.getState().view.items[0]!).text, "Waiting for your browser…");
    failNextPoll = true;
    t.mock.timers.tick(500);
    await tick();
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signingIn");
    status = { type: "signedIn" };
    t.mock.timers.tick(500);
    await tick();
    assert.equal(lineOf(usePlugins.getState().view.items[0]!).text, "");
    assert.equal(reads, 2, "status polling is independent of the one background catalog version check");
    const finishedPolls = polls;
    t.mock.timers.tick(2000);
    await tick();
    assert.equal(polls, finishedPolls, "a completed flow stops polling");
    await adapter.signIn!("one", "one", null);
    disconnect();
    t.mock.timers.tick(2000);
    await tick();
    assert.equal(polls, finishedPolls, "disconnect stops the timer");
  } finally {
    disconnect();
    usePlugins.setState(previous);
    restore();
  }
});

test("an already signed-in start reply updates a stale Sign in row immediately", async () => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return { method: request.method, catalog: catalog(server("one")) };
    return { method: request.method, state: { type: request.method === "startPluginSignIn" ? "signedIn" : "signedOut" } };
  });
  const adapter = livePlugins();
  const disconnect = connectPlugins(adapter);
  try {
    await tick();
    await adapter.signIn!("one", "one", null);
    assert.equal(lineOf(usePlugins.getState().view.items[0]!).text, "");
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("a catalog refresh started before the callback cannot overwrite the polled Signed in state", async (t) => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  t.mock.timers.enable({ apis: ["setInterval"] });
  let status: CatalogRow["signIn"] = { type: "signedOut" };
  let hold = false;
  let reply!: (value: unknown) => void;
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return { method: request.method, catalog: catalog(server("one")) };
    if (request.method === "startPluginSignIn") status = { type: "signingIn", flowId: "fixture" };
    if (hold && request.method === "getPluginSignInStatus") {
      hold = false;
      return new Promise((resolve) => { reply = resolve; });
    }
    return { method: request.method, state: status };
  });
  const adapter = livePlugins();
  const disconnect = connectPlugins(adapter);
  try {
    await tick();
    await adapter.signIn!("one", "one", null);
    hold = true;
    usePlugins.getState().refresh();
    await tick();
    status = { type: "signedIn" };
    t.mock.timers.tick(500);
    await tick();
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedIn");
    reply({ method: "getPluginSignInStatus", state: { type: "signingIn", flowId: "fixture" } });
    await tick();
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedIn");
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("Cancel before start replies stays signed out and serializes backend invalidation", async (t) => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  t.mock.timers.enable({ apis: ["setInterval"] });
  let reply!: (value: unknown) => void;
  let polls = 0;
  const sent: string[] = [];
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return { method: request.method, catalog: catalog(server("one")) };
    if (request.method === "getPluginSignInStatus") { polls++; return { method: request.method, state: { type: "signedOut" } }; }
    sent.push(request.method);
    if (request.method === "startPluginSignIn") return new Promise((resolve) => { reply = resolve; });
    return { method: request.method, state: { type: "signedOut" } };
  });
  const adapter = livePlugins();
  const disconnect = connectPlugins(adapter);
  try {
    await tick();
    const start = adapter.signIn!("one", "one", null);
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signingIn");
    await tick();
    const cancel = adapter.cancelSignIn!("one");
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedOut", "Cancel stops waiting immediately");
    t.mock.timers.tick(500);
    await tick();
    assert.equal(polls, 1, "no pending-state poll can undo Cancel");
    assert.deepEqual(sent, ["startPluginSignIn"], "invalidation waits for the pending start acknowledgement");
    reply({ method: "startPluginSignIn", state: { type: "signingIn", flowId: "late" } });
    await Promise.all([start, cancel]);
    assert.deepEqual(sent, ["startPluginSignIn", "cancelPluginSignIn"]);
    assert.equal(usePlugins.getState().view.items[0]!.signIn?.type, "signedOut");
    t.mock.timers.tick(2000);
    await tick();
    assert.equal(polls, 1, "canceled flows stop polling");
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("a refused gateway starts CLI fallback inline and follows its startup and completion", async (t) => {
  const restore = mockedWindow();
  const previous = usePlugins.getState();
  t.mock.timers.enable({ apis: ["setInterval"] });
  let starts = 0;
  let status: CatalogRow["signIn"] = { type: "signedOut" };
  mockIPC((_command, payload) => {
    const request = (payload as { request: Request }).request;
    if (request.method === "getPluginCatalog") return { method: request.method, catalog: catalog(server("one")) };
    if (request.method === "startPluginSignIn") { starts++; status = { type: "cliFallback", agents: ["claude", "codex"], steps: [] }; }
    return { method: request.method, state: status };
  });
  const adapter = livePlugins();
  const disconnect = connectPlugins(adapter);
  try {
    await tick();
    await adapter.signIn!("one", "one", null);
    assert.equal(starts, 2, "gateway refusal hands off to native sign-in from the same click");
    assert.equal(signingIn(usePlugins.getState().view.items[0]!.signIn), true);
    await usePlugins.getState().reload();
    assert.equal(signingIn(usePlugins.getState().view.items[0]!.signIn), true, "a focus refresh cannot stop following native startup");
    t.mock.timers.tick(500);
    await tick();
    assert.equal(signingIn(usePlugins.getState().view.items[0]!.signIn), true, "keeps waiting while native steps are being collected");
    status = { type: "cliFallback", agents: [], steps: [
      { agent: "claude", server: "one", state: { type: "signedIn" } },
      { agent: "codex", server: "one", state: { type: "alreadySignedIn" } },
    ] };
    t.mock.timers.tick(500);
    await tick();
    assert.equal(signingIn(usePlugins.getState().view.items[0]!.signIn), false);
    assert.equal(signedIn(usePlugins.getState().view.items[0]!.signIn), true);
  } finally { disconnect(); usePlugins.setState(previous); restore(); }
});

test("genuinely different servers get distinct readable agent labels without paths", () => {
  const claude = server("one");
  const codex = server("two");
  claude.name = codex.name = "pencil";
  claude.definitions[0]!.project = codex.definitions[0]!.project = null;
  codex.definitions[0]!.agent = "codex";
  const view = catalogPage(catalog(claude, codex), []);
  assert.deepEqual(view.items.map((item) => item.name), ["pencil · Claude Code", "pencil · Codex"]);
});

test("collisions within one project or plugin keep distinct readable names", () => {
  const claude = server("one");
  const codex = server("two");
  claude.name = codex.name = "review";
  codex.definitions[0]!.agent = "codex";
  const projects = [{ id: "project", name: "Project", path: "/fixture/project", notes: [] }];
  for (const kind of ["skill", "server"] as const) {
    claude.kind = codex.kind = kind;
    let view = catalogPage(catalog(claude, codex), projects);
    assert.deepEqual(view.items.map((item) => item.name), ["review · Project · Claude Code", "review · Project · Codex"]);
    claude.definitions[0]!.plugin = codex.definitions[0]!.plugin = "tools@market";
    view = catalogPage(catalog(claude, codex), []);
    assert.deepEqual(view.items.map((item) => item.name), ["review · tools@market · Claude Code", "review · tools@market · Codex"]);
    assert.ok(view.items.every((item) => !item.name.includes("/")));
  }
});
