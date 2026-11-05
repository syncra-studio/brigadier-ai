import assert from "node:assert/strict";
import { test } from "node:test";

import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { FileImage } from "@/ipc/generated";
import { useWorkerThreads } from "@/state/cliThread";
import { loadThreadImage } from "@/state/threadImages";

test("an image a worker's own transcript holds is read from its session file", async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: { crypto: globalThis.crypto } });
  const image: FileImage = { file: "/worker/session.jsonl", line: 3, index: 0, mime: "image/png" };
  const asked: unknown[] = [];
  mockIPC((cmd, payload) => {
    if (cmd !== "ipc_request") return null;
    const { request } = payload as { request: { method: string; image?: FileImage } };
    asked.push(request);
    return { method: request.method, data: "AA==", mime: "image/png" };
  });
  useWorkerThreads.setState({
    views: { task: { entries: [{ event: { type: "toolCall", itemId: "read_1", name: "Read", input: null, status: "completed", output: null }, atMs: 1, images: [image] }], turns: [] } },
  });
  assert.equal(await loadThreadImage("conv", "read_1"), "data:image/png;base64,AA==");
  assert.deepEqual(asked, [{ method: "readSessionFileImage", conversationId: "conv", image }]);
  clearMocks();
  useWorkerThreads.setState({ views: {} });
});
