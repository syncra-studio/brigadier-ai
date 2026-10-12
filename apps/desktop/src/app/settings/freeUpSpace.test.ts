import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import { action, confirmation, failure, freeLine, picked, result, sweep, usageParts } from "@/app/settings/freeUpSpace";
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

test("the button, the confirmation and the result say it plainly", () => {
  const plan = sweep(report);
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
  assert.equal(action(sweep(tidy)), "Nothing to free up");

  assert.deepEqual(result({ removed: 5, reclaimedBytes: 11 * GB, trashedBytes: 0, failures: [], codexThreadsDeleted: 0 }), {
    title: "Freed 11.0 GB.",
    lines: [],
  });
  assert.deepEqual(
    result({
      removed: 0,
      reclaimedBytes: 0,
      trashedBytes: 0,
      failures: [{ label: "x", path: null, error: "something runs in it now" }],
      codexThreadsDeleted: 0,
    }),
    { title: "Nothing could be removed.", lines: ["1 item stays, as below."] },
  );
  assert.deepEqual(
    result({ removed: 3, reclaimedBytes: 2 * GB, trashedBytes: 0, failures: [], codexThreadsDeleted: 3 }).lines,
    ["Deleted 3 Codex threads. The Codex app keeps its own list, so it may go on showing them."],
  );
});

const stayed = (error: string) => failure({ label: "x", path: null, error });

test("an item that stayed says why in a plain sentence, with the detail under it", () => {
  assert.deepEqual(stayed("something runs in it now"), {
    plain: "Left in place: something is using it now.",
    detail: "Something runs in it now",
  });
  assert.equal(stayed("brigadier/x/session changed since it was listed").plain, "Left in place: it changed since the scan.");
  assert.equal(
    stayed("its unsaved changes couldn't be kept on a branch, so it stays (no HEAD)").plain,
    "Left in place: it has work that isn't on a branch.",
  );
  assert.equal(
    stayed("Codex files: invalid thread id: invalid character").plain,
    "Left in place: Brigadier couldn't remove it safely.",
  );
});

test("choosing by hand takes what is picked and can be removed, never a kept item", () => {
  const plan = picked(report, new Set(["dirty", "brain", "unmerged", "nope"]));
  assert.deepEqual(plan.ids, ["dirty", "brain"]);
  assert.equal(confirmation(plan).footer, "1 item goes to the Trash. 3 things are kept on purpose. Nothing else is touched.");
});

test("the page shows what is used, the Free up space row and Advanced", async () => {
  const server = await createServer({
    server: { middlewareMode: true, ws: false },
    appType: "custom",
    ssr: { noExternal: ["@openai/apps-sdk-ui"] },
  });
  try {
    const { StorageBody } = await server.ssrLoadModule("/src/app/settings/StoragePage.tsx");
    const render = (props: Record<string, unknown>) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(StorageBody, {
          report,
          error: null,
          scanning: false,
          cleaning: false,
          cleaned: null,
          cleanError: null,
          onRefresh: () => {},
          onClean: () => {},
          onDone: () => {},
          ...props,
        }),
      );
    const html = render({});
    assert.match(html, />31\.0 GB used by Brigadier</);
    assert.match(html, />Refresh</);
    assert.match(html, /data-slot="storage-bar"/);
    assert.match(html, />Work folders</);
    assert.match(html, />\/data</);
    assert.match(html, /11\.4 GB can be freed/);
    assert.match(html, />Review</);
    assert.match(html, />Archived chats</);
    assert.match(html, />Manage</);
    assert.match(html, />Advanced</);
    // The groups live in the Review dialog, not on the page.
    assert.doesNotMatch(html, /Finished sessions’ work folders/);

    const looking = render({ report: null, scanning: true });
    assert.match(looking, /Looking at what Brigadier keeps/);
    assert.doesNotMatch(looking, /storage-bar/);

    const updating = render({ scanning: true });
    assert.match(updating, /Updating…/);
    assert.match(updating, /used by Brigadier/);

    const failed = render({ report: null, error: "Couldn't look: boom" });
    assert.match(failed, /Couldn&#x27;t look: boom/);
    assert.match(failed, />Try again</);
  } finally {
    await server.close();
  }
});

test("the disk use parts add up to the scan's total, the rest in Other", () => {
  const parts = usageParts({
    ...report,
    totalBytes: 20 * GB,
    projects: [
      { projectId: "p", name: "p", repoFound: true, worktreesBytes: 8 * GB, brainBytes: GB, blobsBytes: GB, scratchBytes: MB },
    ],
    shared: [
      { part: "database", bytes: GB },
      { part: "models", bytes: 2 * GB },
      { part: "logs", bytes: MB },
      { part: "personalBrain", bytes: GB },
      { part: "other", bytes: GB },
    ],
  });
  assert.deepEqual(
    parts.map((part) => [part.label, part.bytes]),
    [
      ["Work folders", 8 * GB],
      ["Conversations", 2 * GB],
      ["Brains", 2 * GB],
      ["Models", 2 * GB],
      ["Logs & recordings", MB],
      ["Other", 20 * GB - 14 * GB - MB],
    ],
  );
  assert.equal(parts.reduce((total, part) => total + part.bytes, 0), 20 * GB);
});

test("the Free up space line says what the button frees", () => {
  assert.equal(freeLine(sweep({ ...report, items: [] })), "Nothing to free up: Brigadier is tidy");
  assert.match(freeLine(sweep(report)), /^11\.\d GB can be freed; review what goes before anything is removed$/);
});
