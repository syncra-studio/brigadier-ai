import assert from "node:assert/strict";
import { test } from "node:test";

import { previewChip } from "@/app/conversation/previewStatus";
import type { Preview, PreviewState } from "@/ipc/generated";

const preview = (id: string, name: string, startedAtMs: number, state: PreviewState): Preview => ({
  id,
  conversationId: "c1",
  name,
  command: `npm run ${name}`,
  workdir: "/wt/session",
  workspace: "/wt/session",
  pid: 42,
  state,
  startedAtMs,
  endedAtMs: null,
  log: null,
});

const byId = (...previews: Preview[]) => Object.fromEntries(previews.map((p) => [p.id, p]));

test("no chip until a preview runs, and none once every one has ended", () => {
  assert.equal(previewChip({}), null);
  assert.equal(
    previewChip(
      byId(
        preview("preview-1", "web", 1, { type: "exited", code: 1, status: "exit 1" }),
        preview("preview-2", "web", 2, { type: "stopped", reason: "stopped by the user" }),
      ),
    ),
    null,
  );
});

test("one running preview: its name, its command on hover, and Stop stops it", () => {
  const chip = previewChip(
    byId(
      preview("preview-1", "docs", 1, { type: "stopped", reason: "stopped by the thread" }),
      preview("preview-2", "web", 2, { type: "running" }),
    ),
  );
  assert.deepEqual(chip, { label: "web", title: "web: npm run web (in /wt/session)", stops: "preview-2" });
});

test("several running previews: counted, all on hover, and Stop stops them all", () => {
  const chip = previewChip(
    byId(preview("preview-1", "api", 1, { type: "running" }), preview("preview-2", "web", 2, { type: "running" })),
  );
  assert.deepEqual(chip, {
    label: "2 previews",
    title: "web: npm run web (in /wt/session)\napi: npm run api (in /wt/session)",
    stops: null,
  });
});
