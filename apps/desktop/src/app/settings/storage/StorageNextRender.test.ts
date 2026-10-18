import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// Settings › Storage drawn to the user's design (#280), not shown in the app yet: what Brigadier
// uses, Free up space reviewed, removed and done right in its card, a project opened and freed
// up alone, all projects searched and paged, the projects not in use freed up after a confirm,
// Manage and Uninstall.
test("Storage reviews, frees up and shows each project, in the design's words", { timeout: 90000 }, async (t) => {
  const dump = await renderFixtureLive(t, "storage-next.html?drive=1", "storage-next-result", 60000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.equal(seen.title, "Storage");
  assert.deepEqual(seen.sections, ["This computer", "Free up space", "Disk use by project", "Archived chats", "Uninstall"]);
  assert.deepEqual(seen.legend, [
    "Work folders0 B",
    "Conversations5.1 MB",
    "Brains74.6 MB",
    "Models124.7 MB",
    "Logs & recordings444 KB",
    "Other40.7 MB",
  ]);
  assert.match(seen.usage as string, /^245\.5 MBused by Brigadier~\/Library\/Application Support\/Brigadier/);
  assert.deepEqual(seen.rest, [
    "40.0 MB can go",
    "Temporary files, and files left by sessions that are gone. Nothing goes until you confirm.",
    "Review",
  ]);
  assert.deepEqual(seen.top, ["brigadier", "website", "infra-scripts", "notes", "more", "shared"]);
  assert.deepEqual(seen.more, ["97 more projects", "38 of them haven't been used in 3 months", "27.3 MB", "Show all"]);
  assert.ok((seen.shared as string[]).includes("165.8 MB"));

  // The review: a group partly ticked counts and sizes what stays ticked; an unticked item dims.
  assert.deepEqual(seen.review, [
    ["Temporary files", "1 item", "33.9 MB"],
    ["Agent session files", "12 of 13", "5.8 MB"],
    ["Finished sessions' work folders", "1 item", "0 B"],
    ["Kept on purpose", "Brigadier still needs these, so they never go", "2 things", "40 KB"],
  ]);
  const items = seen.items as { text: string; ticked: string }[];
  assert.equal(items.length, 4);
  assert.deepEqual(items.map((item) => item.ticked), ["true", "true", "true", "false"]);
  assert.equal(items[1]!.text, "Claude Code session | Its data folder is gone; it used Brigadier's tools | /private/tmp/brigadier-polish-preview | 268 KB");
  assert.deepEqual(seen.footer, ["39.7 MB selected", "14 of 15 items · nothing else is touched", "Cancel", "Free up 39.7 MB"]);
  assert.equal(seen.afterMore, 13);

  assert.deepEqual(seen.removing, ["Freeing up 39.7 MB…", "9 of 14 items"]);
  assert.deepEqual(seen.done, ["39.7 MB freed", "14 items removed just now · 205.8 MB now used by Brigadier", "What was removed"]);
  assert.equal(seen.removed, 14);
  assert.equal(seen.afterRefresh, "rest");

  // A project opened: what's in it, and a cleanup of just its own files.
  assert.deepEqual(seen.opened, [
    ["Brains", "What agents have learned about this project", "28.0 MB"],
    ["Conversations", "142 chats and sessions, 6 of them archived", "2.1 MB"],
    ["Work folders", "None left over from finished sessions", "0 B"],
  ]);
  assert.deepEqual(seen.openedFooter, ["1.2 MB of this project's files can go", "Reveal in Finder", "Free up 1.2 MB"]);
  assert.deepEqual(seen.projectReview, ["1.2 MB selected", "2 of 15 items · nothing else is touched", "Cancel", "Free up 1.2 MB"]);

  // All projects: search, sort, the ones not in use, 20 at a time.
  assert.deepEqual(seen.allHead, ["Search 101 projects", "Largest first", "Show fewer"]);
  assert.deepEqual(seen.unused, [
    "38 projects not used in 3 months",
    "Their brains and conversations take 9.8 MB. Your repositories and files stay.",
    "Free up 9.8 MB",
  ]);
  assert.deepEqual((seen.allRows as string[][]).slice(0, 5), [
    ["brigadier", "Used today", "30.1 MB"],
    ["website", "Used yesterday", "12.4 MB"],
    ["infra-scripts", "Used 4 days ago", "6.4 MB"],
    ["notes", "Used 2 weeks ago", "3.5 MB"],
    ["design-tokens", "Used 5 months ago", "2.8 MB"],
  ]);
  assert.equal(seen.allCount, 20);
  assert.deepEqual(seen.paging, ["Showing 20 of 101", "Show 20 more"]);
  assert.equal(seen.afterPage, 40);
  assert.deepEqual(seen.searched, ["notes"]);
  assert.deepEqual(seen.confirm, [
    "38 projects not used in 3 months",
    "Delete their 456 conversations and move their Brains to the Trash? The projects, your repositories and files stay.",
    "Cancel",
    "Free up 9.8 MB",
  ]);
  assert.deepEqual(seen.unusedAfter, ["38 projects freed up", "1.2 MB back, 8.6 MB in the Trash. Their repositories and files stayed."]);

  assert.deepEqual(seen.selection, { type: "settings", page: "archived" });
  assert.equal(seen.uninstall, true);
  assert.deepEqual(seen.calls, ["clean 14", "free up 38 projects"]);
});
