import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { ArtifactRef, Task } from "@/ipc/generated";

const file = (id: string, fileName: string): ArtifactRef => ({ id, title: fileName, kind: "note", mime: "text/markdown", bytes: 10, fileName });
const task = (id: string, position: number, requestId: string | null, outputs: ArtifactRef[]) =>
  ({ id, position, requestId, outputs }) as unknown as Task;

test("a finished answer offers Save… for each file its request's workers left, one per name, the latest copy", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { answerFiles } = await server.ssrLoadModule("/src/app/conversation/answerFileList.ts");
    const { AnswerFiles } = await server.ssrLoadModule("/src/app/conversation/AnswerFiles.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const tasks = {
      later: task("later", 2, "r1", [file("rates-2", "rates.md")]),
      scout: task("scout", 1, "r1", [file("rates-1", "rates.md"), file("notes", "notes.md")]),
      other: task("other", 3, "r2", [file("elsewhere", "other.md")]),
      none: task("none", 4, null, [file("loose", "loose.md")]),
    };
    const picked = (answerFiles(Object.values(tasks), ["r1"]) as ArtifactRef[]).map((output) => output.id);
    assert.deepEqual(picked, ["notes", "rates-2"]);
    assert.deepEqual(answerFiles(Object.values(tasks), ["r9"]), []);

    // A6: the thread fixed rates.md and links its corrected copy in its workspace, so the
    // worker's original isn't offered; a link to the stored output itself changes nothing.
    const scouted = { ...tasks, scout: { ...tasks.scout, workspace: { scratch: "/data/scratch/scout" } } as unknown as Task };
    const ids = (answer: string) => (answerFiles(Object.values(scouted), ["r1"], answer) as ArtifactRef[]).map((output) => output.id);
    assert.deepEqual(ids("Fixed the total in [rates.md](/repo/rates.md:12)."), ["notes"]);
    assert.deepEqual(ids("See [rates](file:///repo/docs/rates.md#L3)."), ["notes"]);
    assert.deepEqual(ids("See [rates](outputs/rates.md) and [it](/data/scratch/scout/outputs/notes.md)."), ["notes", "rates-2"]);
    assert.deepEqual(ids("Sources: [rates](https://example.com/rates.md)."), ["notes", "rates-2"]);

    const store = createStore(() => ({ board: { ...emptyBoard("c"), tasks }, orchestrator: null }));
    const render = (requestIds: string[]) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(AnswerFiles, { requestIds, answer: "" })),
    );
    const html = render(["r1"]);
    assert.equal(html.match(/data-slot="answer-file"/g)?.length, 2);
    assert.equal(html.match(/Save…/g)?.length, 2);
    assert.match(html, /notes\.md/);
    assert.match(html, /rates\.md/);
    assert.equal(render(["r9"]), "");
  } finally {
    await server.close();
  }
});

test("a renamed corrected copy replaces only the worker file named by its link", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { answerFiles } = await server.ssrLoadModule("/src/app/conversation/answerFileList.ts");
    const worker = { ...task("worker", 1, "r1", [file("rates", "rates.md"), file("notes", "notes.md")]),
      workspace: { scratch: "/data/scratch/worker" } } as unknown as Task;
    const ids = (answer: string) => (answerFiles([worker], ["r1"], answer) as ArtifactRef[]).map((output) => output.id);
    assert.deepEqual(ids("Corrected [rates.md](/repo/final.md)."), ["notes"]);
    assert.deepEqual(ids("Corrected [**rates.md**](/repo/final.md)."), ["notes"]);
    assert.deepEqual(ids("Corrected [outputs/rates.md:12](/repo/final.md)."), ["notes"]);
    assert.deepEqual(ids("See [rates [v2].md](/repo/rates.md)."), ["notes"]);
    assert.deepEqual(ids("See [rates\nversion](/repo/rates.md)."), ["notes"]);
    assert.deepEqual(ids("Corrected [`rates.md`](</repo/final copy.md>)."), ["notes"]);
    assert.deepEqual(ids("See [rates.md](/data/scratch/worker/outputs/final.md)."), ["rates", "notes"]);
    assert.deepEqual(ids("See [rates.md](outputs/final.md)."), ["rates", "notes"]);
    assert.deepEqual(ids("See [rates.md](https://example.com/final.md)."), ["rates", "notes"]);
    assert.deepEqual(ids("See [unrelated](/repo/final.md)."), ["rates", "notes"]);
    assert.deepEqual(ids("See [rates.md](/repo/src/rates.ts)."), ["rates", "notes"]);
  } finally {
    await server.close();
  }
});
