import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import { action, confirmation, picked, result, summary, sweep } from "@/app/settings/freeUpSpace";
import type { CleanItem, StorageReport } from "@/ipc/generated";

const GB = 1024 ** 3;
const MB = 1024 ** 2;

function item(id: string, fields: Partial<CleanItem>): CleanItem {
  return {
    id,
    category: "finishedWork",
    label: id,
    path: null,
    bytes: 0,
    reason: "",
    checked: true,
    selectable: true,
    toTrash: false,
    badges: [],
    ...fields,
  };
}

const report: StorageReport = {
  scanId: "scan-1",
  dataDir: "/data",
  totalBytes: 31 * GB,
  cleanableBytes: 0,
  projects: [],
  shared: [],
  items: [
    item("done", { label: "Work folder of “Fix login”", bytes: 9 * GB, reason: "Its session ended." }),
    item("branch", { label: "Branch brigadier/a in /repo", bytes: 0 }),
    item("build", { category: "buildFiles", bytes: 2 * GB }),
    item("agent", { category: "agentFiles", bytes: 400 * MB }),
    item("logs", { category: "oldLogs", bytes: 40 * MB }),
    // Work on it: kept, or only by hand.
    item("dirty", {
      label: "Work folder of “Draft”",
      bytes: 2 * GB,
      checked: false,
      reason: "Has unsaved changes, so it isn't part of the sweep.",
      badges: [{ type: "hasChanges" }],
    }),
    item("unmerged", {
      label: "Branch brigadier/b in /repo",
      checked: false,
      selectable: false,
      reason: "Has 3 commits not merged into main.",
      badges: [{ type: "notMerged", ahead: 3 }],
    }),
    // Only by hand, never kept-on-purpose noise.
    item("brain", { category: "brains", bytes: GB, checked: false, toTrash: true }),
  ],
  kept: [{ category: "finishedWork", label: "3 work folders of open sessions", bytes: 4 * GB, reason: "Their sessions are still open." }],
};

test("the sweep takes only safe items of the plain groups, and lists what is kept and why", () => {
  const plan = sweep(report);
  assert.deepEqual(plan.ids, ["done", "branch", "build", "agent", "logs"]);
  assert.equal(plan.bytes, 9 * GB + 2 * GB + 400 * MB + 40 * MB);
  assert.deepEqual(
    plan.groups.map((group) => [group.title, group.items.length]),
    [
      ["Finished sessions’ work folders", 2],
      ["Build files in idle sessions", 1],
      ["Agent session files", 1],
      ["Old logs", 1],
    ],
  );
  // Kept: the dirty work folder, the unmerged branch, the counted line; never the Brain.
  assert.deepEqual(
    plan.kept.map((kept) => kept.label),
    ["Work folder of “Draft”", "Branch brigadier/b in /repo", "3 work folders of open sessions"],
  );
  assert.equal(plan.kept[1]?.reason, "Has 3 commits not merged into main.");
  assert.equal(plan.keptBytes, 6 * GB);
});

test("the summary, the button, the confirmation and the result say it plainly", () => {
  const plan = sweep(report);
  assert.equal(summary(report, plan), "Brigadier uses 31.0 GB on this computer. 11.4 GB can be freed.");
  assert.equal(action(plan), "Free up 11.4 GB");
  const words = confirmation(plan);
  assert.equal(words.title, "Free up 11.4 GB?");
  assert.deepEqual(words.lines, [
    "Finished sessions’ work folders: 2 items, 9.0 GB",
    "Build files in idle sessions: 1 item, 2.0 GB",
    "Agent session files: 1 item, 400.0 MB",
    "Old logs: 1 item, 40.0 MB",
  ]);
  assert.equal(words.footer, "3 things are kept on purpose. Nothing else is touched.");

  const tidy = { ...report, items: [], kept: [] };
  assert.equal(summary(tidy, sweep(tidy)), "Brigadier uses 31.0 GB on this computer. Nothing to free up: Brigadier is tidy.");
  assert.equal(action(sweep(tidy)), "Nothing to free up");

  assert.deepEqual(result({ removed: 5, reclaimedBytes: 11 * GB, trashedBytes: 0, failures: [] }), {
    title: "Freed 11.0 GB.",
    lines: [],
  });
  assert.deepEqual(
    result({
      removed: 0,
      reclaimedBytes: 0,
      trashedBytes: 0,
      failures: [{ label: "x", path: null, error: "something runs in it now" }],
    }),
    { title: "Nothing could be removed.", lines: ["1 item stays, as below."] },
  );
});

test("choosing by hand takes what is picked and can be removed, never a kept item", () => {
  const plan = picked(report, new Set(["dirty", "brain", "unmerged", "nope"]));
  assert.deepEqual(plan.ids, ["dirty", "brain"]);
  assert.equal(confirmation(plan).footer, "1 item goes to the Trash. 3 things are kept on purpose. Nothing else is touched.");
});

test("the screen shows the groups, what is kept, and one button", async () => {
  const server = await createServer({
    server: { middlewareMode: true, ws: false },
    appType: "custom",
    ssr: { noExternal: ["@openai/apps-sdk-ui"] },
  });
  try {
    const { FreeUpSpaceBody } = await server.ssrLoadModule("/src/app/settings/StoragePage.tsx");
    const render = (props: Record<string, unknown>) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(FreeUpSpaceBody, {
          report,
          error: null,
          cleaning: false,
          cleaned: null,
          onClean: () => {},
          onScanAgain: () => {},
          ...props,
        }),
      );
    const html = render({});
    assert.match(html, /11\.4 GB can be freed/);
    assert.match(html, />Finished sessions’ work folders</);
    assert.match(html, />Kept on purpose</);
    assert.match(html, />Free up 11\.4 GB</);
    assert.match(html, />Advanced</);
    assert.doesNotMatch(html, /Temporary files/);

    const looking = render({ report: null });
    assert.match(looking, /Looking at what Brigadier keeps/);

    const done = render({ cleaned: { removed: 5, reclaimedBytes: 11 * GB, trashedBytes: 0, failures: [] } });
    assert.match(done, />Freed 11\.0 GB\.</);
    assert.match(done, />Scan again</);
    assert.doesNotMatch(done, /Free up 11/);
  } finally {
    await server.close();
  }
});
