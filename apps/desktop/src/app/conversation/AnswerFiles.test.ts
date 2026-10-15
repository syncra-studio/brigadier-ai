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

    const store = createStore(() => ({ board: { ...emptyBoard("c"), tasks }, orchestrator: null }));
    const render = (requestIds: string[]) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(AnswerFiles, { requestIds })),
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
