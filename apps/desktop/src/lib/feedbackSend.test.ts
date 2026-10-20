import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { FeedbackFiled, FeedbackReport, FeedbackWriteUp } from "@/ipc/generated";
import { type FeedbackStep, fileWriteUp, retry, send, writeUp as writeUpNote } from "@/lib/feedbackSend";

const details = { version: "0.1.0", system: "macOS 27.0", agents: [] };
const sendFeedback = (text: string, kind?: "bug" | "idea") => send(details, text, kind);
const retryFeedback = (results: Parameters<typeof retry>[1]) => retry(details, results);

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

const bug: FeedbackReport = {
  kind: "bug",
  title: "Stop leaves the tab spinning",
  area: "Chats",
  steps: "Not given; see What happened.",
  expected: "It stops.",
  actual: "It spins.",
  problem: "",
  today: "",
  wanted: "",
  duplicate: null,
};
const idea: FeedbackReport = { ...bug, kind: "idea", title: "Pin a chat", area: "", steps: "", expected: "", actual: "", problem: "Lost chats", wanted: "A pin" };
const repeat: FeedbackReport = {
  ...bug,
  title: "Spinner after Stop",
  duplicate: { number: 12, title: "Spinner", url: "https://github.com/syncra-studio/brigadier-ai/issues/12", open: true, comment: "Seen again." },
};

/** What the faked daemon answers, and what it was asked. */
let writeUp: FeedbackWriteUp | null;
let signedIn: boolean;
let filed: (items: RequestOf<"fileFeedback">["items"]) => FeedbackFiled[];
let asked: RequestOf<"fileFeedback">["items"][];
let opened: string[];

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  writeUp = null;
  signedIn = true;
  asked = [];
  opened = [];
  filed = (items) => items.map((_, index) => ({ outcome: "filed", number: 600 + index, url: `https://github.com/syncra-studio/brigadier-ai/issues/${600 + index}` }));
  mockIPC((command, payload) => {
    if (command === "open_url") {
      opened.push((payload as { url: string }).url);
      return null;
    }
    const request = (payload as { request: RequestOf<"writeUpFeedback"> | RequestOf<"fileFeedback"> }).request;
    if (request.method === "writeUpFeedback") {
      if (!writeUp) throw { code: "invalid", message: "no model can write the report now" };
      return { method: "writeUpFeedback", writeUp };
    }
    asked.push(request.items);
    return { method: "fileFeedback", signedIn, results: signedIn ? filed(request.items) : [] };
  });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

test("signed in, each report is filed as the user and a repeat goes to its issue as a comment", async () => {
  writeUp = { reports: [bug, repeat, idea], searched: true };
  filed = () => [
    { outcome: "filed", number: 601, url: "https://github.com/syncra-studio/brigadier-ai/issues/601" },
    { outcome: "commented", number: 12, url: "https://github.com/syncra-studio/brigadier-ai/issues/12#issuecomment-1" },
    { outcome: "filed", number: 602, url: "https://github.com/syncra-studio/brigadier-ai/issues/602" },
  ];
  const results = await sendFeedback("The tab spins. Pin chats please.");
  assert.deepEqual(
    results.map((result) => [result.title, result.outcome, "number" in result ? result.number : null]),
    [
      ["Stop leaves the tab spinning", "filed", 601],
      ["Spinner after Stop", "added", 12],
      ["Pin a chat", "filed", 602],
    ],
  );
  const [items] = asked;
  assert.equal(items![0]!.title, "[Bug]: Stop leaves the tab spinning");
  assert.ok(items![0]!.body.startsWith("_Filed from Brigadier’s Send feedback"));
  assert.equal(items![0]!.commentOn, null);
  assert.equal(items![1]!.commentOn, 12);
  assert.ok(items![1]!.body.includes("Seen again."));
  assert.equal(items![2]!.title, "[Proposal]: Pin a chat");
  assert.deepEqual(opened, [], "nothing opens in the browser");
});

test("signed out, each report opens its form and a repeat shows the issue it repeats", async () => {
  writeUp = { reports: [bug, repeat], searched: true };
  signedIn = false;
  const results = await sendFeedback("The tab spins.");
  assert.equal(results[0]!.outcome, "opened");
  const known = results[1]!;
  assert.equal(known.outcome, "known");
  if (known.outcome === "known") assert.equal(known.number, 12);
  assert.equal(opened.length, 1, "only the new report's form opens");
  assert.equal(new URL(opened[0]!).searchParams.get("template"), "bug_report.yml");
});

test("with no writer, the note opens in one form of the kind given, nothing else filled in", async () => {
  const results = await sendFeedback("Let me pin a chat", "idea");
  assert.deepEqual(results, [{ kind: "idea", title: "Let me pin a chat", outcome: "opened" }]);
  assert.deepEqual(asked, []);
  assert.deepEqual(Object.fromEntries(new URL(opened[0]!).searchParams), {
    template: "proposal.yml",
    title: "[Proposal]: Let me pin a chat",
    problem: "Let me pin a chat",
  });
  await sendFeedback("It spins");
  assert.equal(new URL(opened[1]!).searchParams.get("template"), "bug_report.yml", "a bug when no kind is known");
});

test("a failure stays its own, and sending again sends only what failed", async () => {
  writeUp = { reports: [bug, idea], searched: true };
  filed = () => [
    { outcome: "filed", number: 601, url: "https://github.com/syncra-studio/brigadier-ai/issues/601" },
    { outcome: "failed", reason: "GitHub didn't take it: HTTP 502" },
  ];
  const first = await sendFeedback("Two things.");
  assert.deepEqual(first.map((result) => result.outcome), ["filed", "failed"]);
  filed = () => [{ outcome: "filed", number: 603, url: "https://github.com/syncra-studio/brigadier-ai/issues/603" }];
  const second = await retryFeedback(first);
  assert.equal(asked.length, 2);
  assert.deepEqual(asked[1]!.map((item) => item.title), ["[Proposal]: Pin a chat"]);
  assert.deepEqual(second.map((result) => [result.outcome, "number" in result ? result.number : null]), [
    ["filed", 601],
    ["filed", 603],
  ]);
});

test("sending says each step: writing it up, then filing as many as it wrote", async () => {
  writeUp = { reports: [bug, idea], searched: true };
  const steps: FeedbackStep[] = [];
  await send(details, "The tab spins. Pin chats.", undefined, (step) => steps.push(step));
  assert.deepEqual(steps, [{ step: "writing" }, { step: "filing", count: 2 }]);
});

test("a write-up made first (the thread's card) is filed as it was shown; none falls back to one form", async () => {
  writeUp = { reports: [idea], searched: false };
  const written = await writeUpNote("Pin chats.");
  assert.deepEqual(written, writeUp);
  const results = await fileWriteUp(details, "Pin chats.", "idea", written);
  assert.equal(results[0]!.outcome, "filed");
  assert.match(asked[0]![0]!.body, /couldn’t search/);

  writeUp = null;
  assert.equal(await writeUpNote("Pin chats."), null);
  const fallback = await fileWriteUp(details, "Pin chats.", "idea", null);
  assert.deepEqual(fallback, [{ kind: "idea", title: "Pin chats.", outcome: "opened" }]);
  assert.equal(new URL(opened[0]!).searchParams.get("template"), "proposal.yml");
});
