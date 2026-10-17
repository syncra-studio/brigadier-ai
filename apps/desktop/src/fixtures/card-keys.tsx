/* oxlint-disable jsx-a11y/no-noninteractive-tabindex -- Focusable stand-ins for the window's parts give a key a target outside the card. */
/**
 * The production decision cards a stray key must not answer (BUILD-PLAN A5): a merge question,
 * "Implement this plan?" and a phase plan's rail link, each mounted fresh and driven with the
 * browser's real keys and clicks (`fixtureInput`) so focus decides where a key lands. IPC is
 * mocked and records every answer; no daemon or credentials.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { type PendingAction, PendingActionCard } from "@/app/conversation/ActionCards";
import { TooltipProvider } from "@/components/ui/tooltip";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import type { Plan, Question, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";

const conversationId = "card-keys-fixture";
// Its first answer is the recommended one: a plain card would light it at once.
const merge: Question = {
  id: "merge", conversationId, taskId: null, requestId: null, position: 0,
  kind: { type: "merge", branch: "brigadier/session", base: "main", conflicted: false, conflicts: [] },
  text: "", options: [], recommended: null,
  items: [{
    text: "Merge `brigadier/session` into `main`?",
    options: [{ label: "Merge into main", description: null }, { label: "Not yet", description: null }],
    recommended: 0,
  }],
  answer: null, answers: [], createdAtMs: 0, answeredAtMs: null,
};
// A plain question: its answers may change code, so its recommended answer is marked, never lit.
const ask: Question = {
  ...merge,
  id: "ask",
  kind: { type: "orchestrator" },
  items: [{
    text: "Which discount kinds?",
    options: [{ label: "Percent off", description: null }, { label: "Fixed amount", description: null }],
    recommended: 0,
  }],
};
// Answered by hand after a visit to Settings, which unmounts the card.
const keep: Question = { ...ask, id: "keep" };
// A merge card left lit and unanswered, apart from the one the other cases mount: an open card keeps what is lit.
const mergeLeftLit: Question = { ...merge, id: "merge-left-lit" };
const plan = (id: string, body: string | null): Plan => ({
  id, conversationId, requestId: null, position: 1, title: "Windows support", body,
  steps: [], state: { type: "proposed" }, createdAtMs: 0, decidedAtMs: null,
});
useBoard.setState({
  board: {
    ...emptyBoard(conversationId),
    loaded: true,
    questions: { merge, ask, keep, "merge-left-lit": mergeLeftLit },
    plans: { document: plan("document", "Handle drive letters, then run the platform checks."), phases: plan("phases", null) },
  },
});

type Call =
  | { method: "answerQuestion"; cardId: string; answers: string[]; input: string | undefined }
  | { method: "decidePlan"; cardId: string; approve: boolean; message: string | null }
  | { method: "dismiss" };
const calls: Call[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "answerQuestion") calls.push({ method: req.method, cardId: req.cardId, answers: req.answers, input: req.input });
  if (req.method === "getDictation") {
    return { method: req.method, dictation: { available: true, model: "speech.bin", modelBytes: 1, installed: true, downloading: false, languages: [] } };
  }
  if (req.method === "decidePlan") calls.push({ method: req.method, cardId: req.cardId, approve: req.approve, message: req.message });
  return { method: req.method };
});

const root = createRoot(document.getElementById("root")!);
let mounts = 0;

/** A fresh window: titlebar, thread body with the card on its rail, and a side panel, each focusable. */
function mount(action: PendingAction) {
  (document.activeElement as HTMLElement | null)?.blur();
  root.render(
    <TooltipProvider>
      <div className="bg-background text-foreground flex h-screen flex-col">
        {/* Plain focusable stand-ins for the window's parts, so a key's target can be each. */}
        <div data-fixture="titlebar" tabIndex={0} className="h-titlebar shrink-0">Titlebar</div>
        <div className="flex min-h-0 flex-1">
          <div data-fixture="thread" tabIndex={0} className="flex min-w-0 flex-1 flex-col justify-end p-8">
            <p>Thread body</p>
            <div data-fixture="rail" tabIndex={-1} className="bg-composer w-full max-w-2xl rounded-xl">
              <PendingActionCard key={++mounts} action={action} more={0} onDismiss={() => calls.push({ method: "dismiss" })} />
            </div>
          </div>
          <div data-fixture="side" tabIndex={0} className="w-80 shrink-0 p-4">Side panel</div>
        </div>
      </div>
    </TooltipProvider>,
  );
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for a condition (never a guess at timing), failing with what it waited for. */
async function until(what: string, check: () => boolean, ms = 5000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
// A picked answer is sent COMMIT_MS (180 ms) after the pick: by 400 ms, any answer a key gave
// would have been sent, so "nothing was sent" is checked after this settle.
const settle = () => sleep(400);
const region = (name: string) => document.querySelector<HTMLElement>(`[data-fixture="${name}"]`)!;
/** The answers lit (1-based), by the row's highlight and its radio state. */
const lit = () =>
  [...document.querySelectorAll("[data-slot=action-option]")].flatMap((row, index) =>
    row.hasAttribute("data-highlighted") || row.getAttribute("aria-checked") === "true" ? [index + 1] : []);
const focused = () =>
  document.activeElement === document.body ? "body"
    : document.activeElement?.closest("[data-slot=action-card]") ? "card"
      : document.activeElement?.getAttribute("data-fixture") ?? document.activeElement?.tagName ?? null;
const key = (name: string) => fixtureInput({ type: "key", key: name });
/** Each answer's marker: its text, size and corners (a 20px square, outlined). */
const markers = () =>
  [...document.querySelectorAll<HTMLElement>("[data-slot=action-option] [data-slot=action-marker]")].map((marker) => {
    const box = marker.getBoundingClientRect();
    const style = getComputedStyle(marker);
    return { text: marker.textContent, size: [Math.round(box.width), Math.round(box.height)], radius: Number.parseFloat(style.borderTopLeftRadius), border: Number.parseFloat(style.borderTopWidth) };
  });

/** Mounts a question or plan card, waits for it to take focus and finish opening. */
async function mountCard(action: PendingAction) {
  mount(action);
  await until("the card to take focus", () => focused() === "card");
  await Promise.all(document.getAnimations().map((animation) => animation.finished));
}

/** Runs a case on a fresh mount: what it saw and the calls it made. */
async function run(drive: (seen: Record<string, unknown>, since: () => Call[]) => Promise<void>) {
  const from = calls.length;
  const seen: Record<string, unknown> = {};
  try {
    await drive(seen, () => calls.slice(from));
  } catch (error) {
    seen.error = String(error);
  }
  seen.calls = calls.slice(from);
  return seen;
}

const question: PendingAction = { type: "question", id: "merge" };
const sent = (from: number) => () => calls.length > from;
/** The card goes, as when Settings takes the window, and comes back. */
async function visitSettings(action: PendingAction) {
  root.render(<p>Settings</p>);
  await until("the card to go", () => document.querySelector("[data-slot=action-card]") === null);
  await mountCard(action);
}
const field = () => document.querySelector<HTMLInputElement>("[data-slot=action-card] input")!;
const results = new URLSearchParams(location.search).get("case") === "kept" ? {
  // The answer the user lit, and then the text typed in the free-text row, survive a visit to Settings.
  kept: await run(async (seen) => {
    const action: PendingAction = { type: "question", id: "keep" };
    await fixtureInput({ type: "pointer", x: 4, y: 4 });
    await mountCard(action);
    seen.litFresh = lit();
    await key("ArrowDown");
    seen.litBefore = lit();
    await visitSettings(action);
    seen.litAfter = lit();
    await fixtureClick(field());
    for (const letter of "tiers") await key(letter);
    seen.typedBefore = field().value;
    await visitSettings(action);
    seen.typedAfter = field().value;
    // Answered from elsewhere while Settings is open: the kept state goes with the question.
    root.render(<p>Settings</p>);
    await until("the card to go", () => document.querySelector("[data-slot=action-card]") === null);
    const questions = useBoard.getState().board!.questions;
    useBoard.setState({ board: { ...useBoard.getState().board!, questions: { ...questions, keep: { ...keep, answer: "Elsewhere", answeredAtMs: 1 } } } });
    useBoard.setState({ board: { ...useBoard.getState().board!, questions } });
    await mountCard(action);
    seen.litReopened = lit();
    seen.typedReopened = field().value;
  }),
} : {
  mountEnter: await run(async (seen) => {
    await mountCard(question);
    seen.focused = focused();
    seen.markers = markers();
    // The free-text row has a microphone where dictation can run.
    await until("the microphone", () => document.querySelector("[data-card-dictate]") !== null).catch(() => {});
    seen.mic = document.querySelector("[data-card-dictate]")?.textContent ?? null;
    seen.lit = lit();
    // The pointer passing over an answer lights nothing either.
    const first = document.querySelector("[data-slot=action-option]")!.getBoundingClientRect();
    await fixtureInput({ type: "pointer", x: first.x + first.width / 2, y: first.y + first.height / 2 });
    seen.litHovered = lit();
    await key("Enter");
    await settle();
  }),
  arrowEnter: await run(async (seen) => {
    await mountCard(question);
    await key("ArrowDown");
    seen.lit = lit();
    const from = calls.length;
    await key("Enter");
    await until("the answer", sent(from));
  }),
  click: await run(async () => {
    await mountCard(question);
    const from = calls.length;
    await fixtureClick(document.querySelectorAll("[data-slot=action-option]")[1]!);
    await until("the answer", sent(from));
  }),
  // A lit answer, then Enter from elsewhere in the window: the danger a window-wide Enter was.
  enterOutside: await run(async (seen) => {
    await mountCard({ type: "question", id: "merge-left-lit" });
    await key("ArrowDown");
    seen.lit = lit();
    seen.targets = [];
    for (const name of ["thread", "side", "titlebar"]) {
      region(name).focus();
      (seen.targets as unknown[]).push(focused());
      await key("Enter");
    }
    await settle();
    seen.litAfter = lit();
  }),
  // A plain question lights nothing either, and a click in the thread, then Enter, answers nothing.
  plainEnterOutside: await run(async (seen) => {
    await fixtureInput({ type: "pointer", x: 4, y: 4 });
    await mountCard({ type: "question", id: "ask" });
    seen.lit = lit();
    seen.recommended = document.querySelector("[data-slot=action-option]")?.textContent?.includes("Recommended");
    const first = document.querySelector("[data-slot=action-option]")!.getBoundingClientRect();
    await fixtureInput({ type: "pointer", x: first.x + first.width / 2, y: first.y + first.height / 2 });
    seen.litHovered = lit();
    await fixtureInput({ type: "pointer", x: 4, y: 4 });
    await fixtureClick(region("thread").querySelector("p")!);
    seen.focused = focused();
    await key("Enter");
    await settle();
    seen.litAfter = lit();
  }),
  // Enter on the focused card right after mount sends nothing; 1 lights the first answer, then Enter sends it.
  plainEnterFocused: await run(async (seen, since) => {
    await fixtureInput({ type: "pointer", x: 4, y: 4 });
    await mountCard({ type: "question", id: "ask" });
    await key("Enter");
    await settle();
    seen.afterEnter = since();
    await key("1");
    await settle();
    seen.lit = lit();
    seen.afterDigit = since();
    const from = calls.length;
    await key("Enter");
    await until("the answer", sent(from));
  }),
  digitUnfocused: await run(async (seen) => {
    await mountCard(question);
    region("thread").focus();
    seen.focused = focused();
    await key("1");
    await settle();
    seen.lit = lit();
  }),
  digitFocused: await run(async (seen) => {
    await mountCard(question);
    await key("1");
    await settle();
    seen.lit = lit();
  }),
  phasePlan: await run(async (seen, since) => {
    mount({ type: "plan", id: "phases" });
    await until("the plan's rail link", () => document.querySelector("[data-slot=plan-row]") !== null);
    seen.targets = [focused()];
    await key("Enter");
    region("rail").focus();
    (seen.targets as unknown[]).push(focused());
    await key("Enter");
    await settle();
    seen.beforeEscape = since();
    // The card's keys are live: Esc puts it off for later.
    await key("Escape");
    await until("Esc to put it aside", () => calls.at(-1)?.method === "dismiss");
  }),
  implementPlan: await run(async (seen, since) => {
    await mountCard({ type: "plan", id: "document" });
    seen.lit = lit();
    seen.markers = markers();
    await key("Enter");
    await settle();
    seen.afterEnter = since();
    // A deliberate pick on the focused card builds it.
    await key("ArrowDown");
    const from = calls.length;
    await key("Enter");
    await until("the plan's decision", sent(from));
  }),
};
const result = document.createElement("pre");
result.id = "card-keys-result";
result.textContent = JSON.stringify(results);
document.body.append(result);
