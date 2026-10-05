/** Real conversation/composer, with attachment storage and sends confined to this fixture. */
// The existing fixture owns the real window layout and mounts its conversation.
// oxlint-disable-next-line import/no-unassigned-import
import "@/fixtures/flow";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { AttachmentRef, Message, Request } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const calls: Request[] = [];
const uploads: { failed: boolean; name: string }[] = [];
const bytes = new Map<string, string>(Object.entries(JSON.parse(sessionStorage.getItem("inline-paste-bytes") ?? "{}") as Record<string, string>));
let delay = 0;
let fail = false;
let sequence = 40;
Object.assign(window, { inlinePaste: {
  calls, uploads,
  setDelay: (ms: number) => { delay = ms; },
  setFail: (value: boolean) => { fail = value; },
} });
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "addAttachment": {
      const shouldFail = fail;
      uploads.push({ failed: shouldFail, name: req.name });
      if (delay) await new Promise((resolve) => setTimeout(resolve, delay));
      if (shouldFail) throw new Error("Fixture upload failed");
      const hash = await crypto.subtle.digest("SHA-256", Uint8Array.from(atob(req.data), (char) => char.charCodeAt(0)));
      const id = [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
      bytes.set(id, req.data);
      sessionStorage.setItem("inline-paste-bytes", JSON.stringify(Object.fromEntries(bytes)));
      const attachment: AttachmentRef = { id, name: req.name, mime: req.mime, bytes: atob(req.data).length, pasted: req.pasted, inline: null };
      return { method: req.method, attachment };
    }
    case "readAttachment":
      return { method: req.method, data: bytes.get(req.id) ?? "" };
    case "sendMessage": {
      const message: Message = {
        id: `paste-${sequence++}`, conversationId: req.conversationId, seq: sequence,
        role: "user", text: req.text, blob: null, attachments: req.attachments, mentions: req.mentions,
        createdAtMs: Date.now(), model: null, requestId: `paste-request-${sequence}`, parentId: null,
      };
      return { method: req.method, outcome: { type: "sent", message } };
    }
    case "getPullRequest": return { method: req.method, pullRequest: null };
    case "getRunDiff": return { method: req.method, diff: null };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    case "listWorkerEvents": return { method: req.method, page: { entries: [], hasMore: false } };
    default: return { method: req.method };
  }
});
// A quiet finished chat makes screenshots about the paste, without synthetic phase work.
useApp.setState((state) => ({
  conversations: Object.fromEntries(Object.entries(state.conversations).map(([id, conversation]) => [id, { ...conversation, kind: "chat", setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } }, title: "Inline image paste" }])),
  threads: Object.fromEntries(Object.entries(state.threads).map(([id, thread]) => [id, { ...thread, items: [] }])),
  pinnedSummary: false,
}));
useBoard.setState(({ board }) => board ? { board: { ...board, tasks: {}, requests: {}, plans: {}, approvals: {}, waiting: {}, orchestratorSteps: [], run: "idle", runRequest: null } } : {});
