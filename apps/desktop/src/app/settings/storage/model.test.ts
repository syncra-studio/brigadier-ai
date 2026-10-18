import assert from "node:assert/strict";
import { test } from "node:test";

import {
  checkedWords,
  conversationsLine,
  listProjects,
  size,
  startTicked,
  summaryLine,
  tickState,
  unusedProjects,
  usedWords,
} from "@/app/settings/storage/model";
import type { CleanItem, ProjectUsage, StorageReport } from "@/ipc/generated";

const MB = 1024 ** 2;
const DAY = 24 * 60 * 60 * 1000;
const NOW = new Date(2026, 9, 18, 12).getTime();

const project = (name: string, mb: number, daysAgo: number, fields: Partial<ProjectUsage> = {}): ProjectUsage => ({
  projectId: name,
  name,
  path: `/code/${name}`,
  repoFound: true,
  lastUsedMs: NOW - daysAgo * DAY,
  conversations: 3,
  archived: 0,
  worktreesBytes: 0,
  brainBytes: mb * MB,
  blobsBytes: 0,
  scratchBytes: 0,
  ...fields,
});

const item = (id: string, fields: Partial<CleanItem>): CleanItem => ({
  id,
  category: "agentFiles",
  label: id,
  path: null,
  bytes: MB,
  reason: "",
  checked: true,
  selectable: true,
  toTrash: false,
  badges: [],
  projectId: null,
  ...fields,
});

const report = (fields: Partial<StorageReport>): StorageReport => ({
  scanId: "s",
  dataDir: "/data",
  totalBytes: 0,
  cleanableBytes: 0,
  projects: [],
  shared: [],
  items: [],
  kept: [],
  ...fields,
});

test("sizes read as the design writes them", () => {
  assert.deepEqual([0, 444 * 1024, 5.1 * MB, 1.25 * 1024 * MB].map(size), ["0 B", "444 KB", "5.1 MB", "1.3 GB"]);
});

test("when a project was used, and when the page last looked", () => {
  assert.deepEqual(
    [0, 1, 4, 14, 21, 150, 400].map((days) => usedWords(NOW - days * DAY, NOW)),
    ["Used today", "Used yesterday", "Used 4 days ago", "Used 2 weeks ago", "Used 3 weeks ago", "Used 5 months ago", "Used 1 year ago"],
  );
  assert.equal(checkedWords(NOW - 5000, NOW), "Checked just now");
  assert.equal(checkedWords(NOW - 3 * 60_000, NOW), "Checked 3m ago");
});

test("projects sort by size, last use or name, and search by name or folder", () => {
  const projects = [project("b", 1, 9), project("a", 3, 2), project("c", 2, 0)];
  assert.deepEqual(listProjects(projects, "largest").map((p) => p.name), ["a", "c", "b"]);
  assert.deepEqual(listProjects(projects, "recent").map((p) => p.name), ["c", "a", "b"]);
  assert.deepEqual(listProjects(projects, "name").map((p) => p.name), ["a", "b", "c"]);
  assert.deepEqual(listProjects(projects, "largest", " /CODE/b").map((p) => p.name), ["b"]);
});

test("projects not used in 3 months, but not those freed up already", () => {
  const stale = unusedProjects(
    report({
      projects: [
        project("old", 2, 120, { blobsBytes: MB }),
        project("freed", 0, 200, { conversations: 0 }),
        project("new", 5, 10),
      ],
    }),
    NOW,
  );
  assert.deepEqual(stale.projects.map((p) => p.name), ["old"]);
  assert.equal(stale.bytes, 3 * MB);
});

test("a project's free-up ticks only its own items; a group's box says how it stands", () => {
  const scanned = report({
    items: [
      item("mine", { projectId: "p" }),
      item("other", {}),
      item("by-hand", { projectId: "p", checked: false }),
    ],
  });
  assert.deepEqual([...startTicked(scanned, "p")], ["mine"]);
  assert.deepEqual([...startTicked(scanned)], ["mine", "other"]);
  const group = { category: "agentFiles" as const, title: "", items: scanned.items, bytes: 0 };
  assert.equal(tickState(group, new Set()), "none");
  assert.equal(tickState(group, new Set(["mine"])), "some");
  assert.equal(tickState(group, new Set(["mine", "other", "by-hand"])), "all");
});

test("the summary names what can go; conversations say how many are archived", () => {
  assert.equal(
    summaryLine(report({ items: [item("t", { category: "temporary" }), item("a", {}), item("w", { category: "finishedWork", bytes: 0 })] })),
    "Temporary files, and files left by sessions that are gone. Nothing goes until you confirm.",
  );
  assert.equal(summaryLine(report({})), "Brigadier is tidy.");
  assert.equal(conversationsLine(project("p", 1, 0, { conversations: 142, archived: 6 })), "142 chats and sessions, 6 of them archived");
  assert.equal(conversationsLine(project("p", 1, 0, { conversations: 1 })), "1 chat or session");
});
