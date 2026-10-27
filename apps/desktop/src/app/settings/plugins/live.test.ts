import assert from "node:assert/strict";
import { test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { CatalogRow, CatalogView, Request } from "@/ipc/generated";
import { catalogPage, livePlugins } from "./live";
import { connectPlugins, usePlugins } from "./adapter";

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

test("the first catalog stays loading while independent sign-in reads run in parallel", async () => {
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
    assert.equal(usePlugins.getState().loading, true);
    pending[0]!({ method: "getPluginSignInStatus", state: { type: "signedIn" } });
    await tick();
    assert.equal(usePlugins.getState().loading, true);
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
