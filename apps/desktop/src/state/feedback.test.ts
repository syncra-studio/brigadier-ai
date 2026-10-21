import assert from "node:assert/strict";
import { after, afterEach, before, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { createServer, type ViteDevServer } from "vite";

import type { RequestOf } from "@/ipc/client";
import type { FeedbackFiled, FeedbackReport } from "@/ipc/generated";
import type * as FeedbackModule from "@/state/feedback";

// The module reads the provider labels from a component file, so Vite loads it.
type Feedback = typeof FeedbackModule;
let server: ViteDevServer;
let closeFeedback: Feedback["closeFeedback"];
let dismissOffer: Feedback["dismissOffer"];
let openFeedback: Feedback["openFeedback"];
let sendFromDialog: Feedback["sendFromDialog"];
let sendOffer: Feedback["sendOffer"];
let setFeedbackText: Feedback["setFeedbackText"];
let useFeedbackOffers: Feedback["useFeedbackOffers"];
let useFeedbackUi: Feedback["useFeedbackUi"];

before(async () => {
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ closeFeedback, dismissOffer, openFeedback, sendFromDialog, sendOffer, setFeedbackText, useFeedbackOffers, useFeedbackUi } =
    (await server.ssrLoadModule("/src/state/feedback.ts")) as Feedback);
});

after(async () => {
  await server.close();
});

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");

const report = (title: string): FeedbackReport => ({
  kind: "bug",
  title,
  area: "Chats",
  steps: "Not given; see What happened.",
  expected: "It stops.",
  actual: "It spins.",
  problem: "",
  today: "",
  wanted: "",
  duplicate: null,
  related: [],
});

/** What the faked daemon was asked to write up, and how it files: once per call, in turn. */
let wrote: string[];
let filings: ((items: RequestOf<"fileFeedback">["items"]) => FeedbackFiled[])[];
let asked: RequestOf<"fileFeedback">["items"][];
/** Who the faked daemon says it posts as, and when; the sign-in requests it got, in turn. */
let account: () => Promise<boolean> | boolean;
let signIns: string[];
/** When the faked sign-in's wait answers (GitHub said yes). */
let signedIn: Promise<void>;

const filed = (number: number): FeedbackFiled => ({ outcome: "filed", number, url: `https://github.com/syncra-studio/brigadier-ai/issues/${number}` });
const refused: FeedbackFiled = { outcome: "failed", reason: "HTTP 502: Bad gateway" };

beforeEach(() => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  wrote = [];
  filings = [];
  asked = [];
  account = () => true;
  signIns = [];
  signedIn = Promise.resolve();
  useFeedbackUi.setState({ open: false, draft: null, text: "", sending: { phase: "write" }, account: { signedIn: true, login: "someone" } });
  useFeedbackOffers.setState({}, true);
  mockIPC(async (command, payload) => {
    if (command === "system_name") return "macOS 27.0 (aarch64)";
    if (command === "open_url") return null;
    const request = (
      payload as {
        request:
          | RequestOf<"writeUpFeedback">
          | RequestOf<"fileFeedback">
          | RequestOf<"feedbackAccount">
          | RequestOf<"startGitHubSignIn">
          | RequestOf<"waitGitHubSignIn">
          | RequestOf<"cancelGitHubSignIn">;
      }
    ).request;
    if (request.method === "feedbackAccount") {
      const yes = await account();
      return { method: "feedbackAccount", signedIn: yes, login: yes ? "someone" : null };
    }
    if (request.method === "startGitHubSignIn") {
      signIns.push(request.method);
      return { method: "startGitHubSignIn", signIn: { state: "code", code: "ABCD-1234", url: "https://github.com/login/device" } };
    }
    if (request.method === "waitGitHubSignIn") {
      signIns.push(request.method);
      await signedIn;
      return { method: "waitGitHubSignIn" };
    }
    if (request.method === "cancelGitHubSignIn") {
      signIns.push(request.method);
      return { method: "cancelGitHubSignIn" };
    }
    if (request.method === "writeUpFeedback") {
      wrote.push(request.text);
      return { method: "writeUpFeedback", writeUp: { reports: request.text.split(". ").map(report), searched: true } };
    }
    asked.push(request.items);
    const next = filings.shift() ?? ((items) => items.map((_, index) => filed(700 + index)));
    return { method: "fileFeedback", signedIn: true, results: next(request.items) };
  });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
});

/** Waits for the dialog's send to leave `working`. */
async function settled(): Promise<void> {
  for (let tries = 0; tries < 200 && useFeedbackUi.getState().sending.phase === "working"; tries++) {
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.notEqual(useFeedbackUi.getState().sending.phase, "working");
}

test("a card's Edit counts as sent when a Try again files what failed the first time", async () => {
  let sent = 0;
  openFeedback({ kind: "bug", text: "Stop leaves it spinning", onSent: () => sent++ });
  filings = [(items) => items.map(() => refused)];
  sendFromDialog();
  await settled();
  assert.equal(useFeedbackUi.getState().sending.phase, "failed");
  assert.equal(sent, 0);
  sendFromDialog();
  await settled();
  assert.equal(useFeedbackUi.getState().sending.phase, "done");
  assert.equal(sent, 1);
});

test("a note changed after a failed send is written up anew on Try again", async () => {
  openFeedback();
  setFeedbackText("Stop leaves it spinning");
  filings = [(items) => items.map(() => refused)];
  sendFromDialog();
  await settled();
  setFeedbackText("Stop leaves the tab spinning");
  sendFromDialog();
  await settled();
  assert.deepEqual(wrote, ["Stop leaves it spinning", "Stop leaves the tab spinning"]);
  assert.equal(asked[1]![0]!.title, "[Bug]: Stop leaves the tab spinning");
});

test("a send that fails after the dialog closed is still there when it opens again", async () => {
  openFeedback();
  setFeedbackText("Stop leaves it spinning");
  filings = [(items) => items.map(() => refused)];
  sendFromDialog();
  closeFeedback();
  await settled();
  openFeedback();
  const { text, sending, open } = useFeedbackUi.getState();
  assert.equal(open, true);
  assert.equal(text, "Stop leaves it spinning");
  assert.equal(sending.phase, "failed");
});

test("a card that sent only some files the rest on Try again, and is answered once", async () => {
  useFeedbackOffers.setState({
    offer: { written: { reports: [report("Spins"), report("Hangs")], searched: true }, sending: { phase: "write" } },
  });
  let answered = 0;
  const settledOffer = async () => {
    for (let tries = 0; tries < 200 && useFeedbackOffers.getState().offer?.sending.phase === "working"; tries++) {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  };
  filings = [() => [filed(801), refused]];
  sendOffer("offer", "Spins. Hangs", "bug", () => answered++);
  await settledOffer();
  assert.equal(useFeedbackOffers.getState().offer?.sending.phase, "done");
  assert.equal(answered, 1);
  filings = [() => [filed(802)]];
  sendOffer("offer", "Spins. Hangs", "bug", () => answered++);
  await settledOffer();
  assert.equal(asked[1]!.length, 1);
  assert.equal(asked[1]![0]!.title, "[Bug]: Hangs");
  const sending = useFeedbackOffers.getState().offer?.sending;
  assert.deepEqual(sending?.phase === "done" && sending.results.map((result) => result.outcome), ["filed", "filed"]);
  assert.equal(answered, 1);
});

/** Waits until `ready` holds (the faked daemon answers on later turns). */
async function until(ready: () => boolean): Promise<void> {
  for (let tries = 0; tries < 200 && !ready(); tries++) {
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.ok(ready());
}

/** A sign-in GitHub says yes to only when the returned function is called. */
function signInPending(): () => void {
  let yes: (() => void) | undefined;
  signedIn = new Promise((resolve) => {
    yes = resolve;
  });
  return () => yes?.();
}

test("closing the dialog while its sign-in finishes sends nothing", async () => {
  useFeedbackUi.setState({ account: null });
  account = () => false;
  const yes = signInPending();
  openFeedback({ kind: "bug", text: "Stop leaves it spinning" });
  await until(() => useFeedbackUi.getState().account !== null);
  sendFromDialog();
  await until(() => signIns.includes("waitGitHubSignIn"));
  closeFeedback();
  // GitHub's yes was already on its way when the dialog closed.
  yes();
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(wrote, []);
  assert.deepEqual(asked, []);
  assert.equal(useFeedbackUi.getState().sending.phase, "write");
});

test("dismissing a card while its sign-in finishes files nothing", async () => {
  useFeedbackUi.setState({ account: null });
  account = () => false;
  const yes = signInPending();
  useFeedbackOffers.setState({ offer: { written: { reports: [report("Spins")], searched: true }, sending: { phase: "write" } } });
  openFeedback();
  await until(() => useFeedbackUi.getState().account !== null);
  closeFeedback();
  let answered = 0;
  sendOffer("offer", "Spins", "bug", () => answered++);
  await until(() => signIns.includes("waitGitHubSignIn"));
  dismissOffer("offer");
  yes();
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.deepEqual(asked, []);
  assert.equal(answered, 0);
});

test("a Send before it knows who it posts as waits, and signs in when signed out", async () => {
  useFeedbackUi.setState({ account: null });
  let answer: ((yes: boolean) => void) | undefined;
  account = () =>
    new Promise<boolean>((resolve) => {
      answer = resolve;
    });
  openFeedback({ kind: "bug", text: "Stop leaves it spinning" });
  sendFromDialog();
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.deepEqual(wrote, []);
  answer?.(false);
  await until(() => useFeedbackUi.getState().sending.phase === "done");
  assert.deepEqual(signIns, ["startGitHubSignIn", "waitGitHubSignIn"]);
  assert.deepEqual(wrote, ["Stop leaves it spinning"]);
  // The ask again once signed in.
  answer?.(true);
});
