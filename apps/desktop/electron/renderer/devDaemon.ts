// A stand-in for brigadierd, for the Electron shell before stage 2: enough for the real front end to
// start and show its sessions. Replies are empty unless a screen needs more.
import type { BridgeEvent, Conversation, Project } from "@/ipc/generated";
import { emptyBoard } from "@/state/board";
import { useApp } from "@/state/store";

type Channelish = { onmessage: (message: unknown) => void };

/** Each named window runs its own sessions, as `?bcw=<name>` says. */
const windowName = new URLSearchParams(location.search).get("bcw") ?? "main";
export const SESSION_ID = `session-${windowName}`;
export const OTHER_SESSION_ID = `session-${windowName}-other`;
const PROJECT_ID = "dev-project";

const conversation = (id: string, title: string): Conversation => ({
  id,
  kind: "session",
  projectId: PROJECT_ID,
  title,
  pinnedAtMs: null,
  createdAtMs: 0,
  updatedAtMs: 0,
  setup: null,
  lifecycle: "active",
  forkedFrom: null,
  sideOf: null,
  quotaWait: null,
} as unknown as Conversation);

const conversations = () => [conversation(SESSION_ID, `Browser ${windowName}`), conversation(OTHER_SESSION_ID, `Other ${windowName}`)];

const view = (id: string) => ({
  conversation: conversations().find((each) => each.id === id),
  messages: { messages: [], hasMore: false },
  tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [],
  machineSteps: [], decisions: [], compactions: [], ratings: {},
  queue: emptyBoard(id).queue,
  run: "idle", runRequest: null, head: null, context: null, streaming: null, thinking: [], notices: [], previews: [],
});

export function answerDaemon(request: { method: string; id?: string }): unknown {
  switch (request.method) {
    case "getCatalog":
      return {
        method: request.method,
        catalog: {
          projects: [{ id: PROJECT_ID, name: "Browser check", path: "/tmp/brigadier-browser-check", createdAtMs: 0, updatedAtMs: 0, repos: [{ id: "dev-repo", name: "dev", path: "/tmp/brigadier-browser-check" }], prefs: {}, trust: [{ path: "/tmp/brigadier-browser-check", trusted: true }] } as unknown as Project],
          conversations: conversations(),
          settings: { ...useApp.getState().settings, browserEnabled: true, onboarded: true, introSeen: true },
        },
      };
    case "getConversation":
      return { method: request.method, view: view(request.id ?? SESSION_ID) };
    case "getProviders":
      return { method: request.method, view: { providers: [] } };
    case "getActivity":
      return { method: request.method, activity: [] };
    case "getUpdates":
      return { method: request.method, updates: { items: [] } };
    case "getConnect":
      return { method: request.method, connect: { providers: [] } };
    case "getSettledTabs":
      return { method: request.method, tabs: [], landed: [], merged: [] };
    case "threadView":
      return { method: request.method, view: { entries: [], turns: [], workers: [] } };
    default:
      return { method: request.method };
  }
}

export function startDaemonEvents(channel: Channelish): void {
  const connected: BridgeEvent = {
    type: "connected",
    lastSeq: 0,
    daemon: { version: "dev", protocol: 0, pid: 0, platform: "macos", startedAtMs: Date.now(), dataDir: "" },
  };
  queueMicrotask(() => channel.onmessage(connected));
}
