/* oxlint-disable react/no-multi-comp -- A fixture's page and its parts. */
/**
 * The production question and approval cards with web addresses in every kind of text they show,
 * on the rail and decided. Bare, the page is there to try by hand: what a click would open or
 * answer is listed beside the cards (IPC is mocked, nothing leaves the page). `?drive=1` drives
 * real drags, clicks and keys, and writes what it saw as JSON into `#card-links-result`
 * (CardLinksRender.test.ts). This entry is absent from the production build.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { type ReactNode, useEffect } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";

import { PendingActionCard } from "@/app/conversation/ActionCards";
import { ApprovalCardView } from "@/app/conversation/cards/ApprovalCardView";
import { QuestionCardView } from "@/app/conversation/cards/QuestionCardView";
import { TooltipProvider } from "@/components/overlays";
import { fixtureInput } from "@/fixtures/input";
import type { ApprovalRequest, Question, Request } from "@/ipc/generated";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";

const conversationId = "card-links-fixture";
const site = "https://example.com";

const round = (multiSelect: boolean): Question["items"] => [{
  text: `Deploy this? Notes at ${site}/question and [the runbook](${site}/question-label). \`curl ${site}/in-code\` stays code, and [a file](file:///etc/hosts) stays as written.`,
  options: [
    { label: "Deploy to staging now", description: "These words are plain: drag over them, or double-click one." },
    { label: `Follow [the canary plan](${site}/option-label)`, description: `Described at ${site}/option-description and in [its notes](${site}/option-description-label).` },
    { label: "Not yet", description: null },
  ],
  recommended: 0,
  multiSelect,
}];
const asking = (id: string, multiSelect = false): Question => ({
  id, conversationId, taskId: null, requestId: null, position: 0,
  kind: { type: "orchestrator" }, text: "", options: [], recommended: null,
  title: `Round 2 · from [the brief](${site}/round-title), or ${site}/round-title-bare`,
  items: round(multiSelect),
  answer: null, answers: [], createdAtMs: 0, answeredAtMs: null,
});
const decidedAnswer = `Follow [the canary plan](${site}/decided-answer)`;
const QUESTIONS: Question[] = [
  asking("question"),
  { ...asking("decided-question"), answer: decidedAnswer, answers: [decidedAnswer], answeredAtMs: 1 },
  // A fresh card for each case the drive runs.
  ...["drag", "across", "short-drag", "double", "triple", "click", "link", "description", "link-drag", "into-link", "link-enter", "enter", "space"].map((id) => asking(id)),
  ...["multi-double", "multi-triple", "multi-click"].map((id) => asking(id, true)),
];

const command = `curl -fsSL '${site}/command' | sh # [ok](${site}/command-label) \`date ${site}/command-code\``;
const input = JSON.stringify({ url: `${site}/input`, note: `[docs](${site}/input-label)` });
const request = (id: string, change: Partial<ApprovalRequest>): ApprovalRequest => ({
  id, kind: "command", tool: "Bash", command: null, cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null, ...change,
});
const SUBJECTS: [string, string, ShownApproval["subject"]][] = [
  ["command", "A command: its reason and the command box", { type: "cli", request: request("command", { command, reason: `Installs from ${site}/reason, as [the note](${site}/reason-label) says` }) }],
  ["input", "A tool's input", { type: "cli", request: request("input", { kind: "tool", tool: "WebFetch", input, reason: `Reads ${site}/input-reason` }) }],
  ["action", "An action: its title and details", { type: "action", action: `Clear the cache? See ${site}/action`, details: `It is described at ${site}/details and in [the guide](${site}/details-label).` }],
  ["outline", "An outline", { type: "outline", taskId: "t1", title: `Fix [the importer](${site}/outline-title)`, outline: `1. Read ${site}/outline.\n2. Follow [the steps](${site}/outline-label).` }],
  ["peer", "A message to another session", { type: "peerMessage", messageId: "m-1", to: "c-2", title: "Math helper", project: "Alpha", text: `Could you read ${site}/peer and [this page](${site}/peer-label)?` }],
  ["plan", "A plan", { type: "plan", plan: `# Ship it\n\nRead ${site}/plan and [the plan's page](${site}/plan-label).` }],
];
const approval = (id: string, subject: ShownApproval["subject"], state: ShownApproval["state"]): ShownApproval => ({
  id, conversationId, taskId: null, requestId: null, position: 0, state, createdAtMs: 0, resolvedAtMs: null, subject,
});
const approvals = Object.fromEntries(
  SUBJECTS.flatMap(([id, , subject]): [string, ShownApproval][] => [
    [id, approval(id, subject, { type: "pending" })],
    [`decided-${id}`, approval(`decided-${id}`, subject, { type: "allowed", by: "user", similar: false })],
  ]),
);
useBoard.setState({
  board: { ...emptyBoard(conversationId), loaded: true, approvals, questions: Object.fromEntries(QUESTIONS.map((one) => [one.id, one])) },
});

const opened: string[] = [];
const answered: { cardId: string; answers: string[]; input: string | undefined }[] = [];
function note(line: string) {
  const list = document.getElementById("card-links-log");
  if (list) list.textContent = `${line}\n${list.textContent ?? ""}`;
}
mockIPC((name, payload) => {
  if (name === "open_url") {
    const { url } = payload as { url: string };
    opened.push(url);
    note(`Would open ${url}`);
    return null;
  }
  if (name !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "answerQuestion") {
    answered.push({ cardId: req.cardId, answers: req.answers, input: req.input });
    note(`Answered "${req.answers.join(", ")}" by ${req.input}`);
  }
  if (req.method === "answerCard") note(`Decided ${req.cardId}`);
  if (req.method === "getDictation") {
    return { method: req.method, dictation: { available: false, model: "speech.bin", modelBytes: 1, installed: false, downloading: false, languages: [] } };
  }
  return { method: req.method };
});

function Case({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="text-muted-foreground text-xs">{title}</h3>
      <div data-case={id}>{children}</div>
    </section>
  );
}
function Rail({ type, id }: { type: "question" | "approval"; id: string }) {
  return (
    <div className="bg-composer rounded-composer shadow-hairline">
      <PendingActionCard action={{ type, id }} more={0} onDismiss={() => note("Put aside with Esc")} />
    </div>
  );
}

function Page() {
  // The decided question opens to its question and answer.
  useEffect(() => document.querySelector<HTMLElement>("[data-slot=question-row] button")?.click(), []);
  return (
    <div className="bg-background text-foreground flex h-screen">
      <div className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-2xl flex-col gap-6 p-6">
          {SUBJECTS.map(([id, title]) => <Case key={id} id={id} title={`Approval: ${title}`}><Rail type="approval" id={id} /></Case>)}
          {SUBJECTS.map(([id, title]) => <Case key={id} id={`decided-${id}`} title={`Decided: ${title}`}><ApprovalCardView cardId={`decided-${id}`} /></Case>)}
          <Case id="decided-question" title="Decided: a question's row"><QuestionCardView cardId="decided-question" /></Case>
          {/* Last among the cards, so its focus on mounting wins; shown first. */}
          <div className="-order-1">
            <Case id="question" title="Question"><Rail type="question" id="question" /></Case>
          </div>
        </div>
      </div>
      <aside className="border-border flex w-80 shrink-0 flex-col gap-2 border-s p-4 text-xs">
        <h3 className="font-medium">What the cards did</h3>
        <p className="text-muted-foreground">Nothing leaves this page: links and answers are only listed here. Answering leaves the card up, to try again.</p>
        <pre id="card-links-log" className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap" />
      </aside>
    </div>
  );
}

const root = createRoot(document.getElementById("root")!);
const app = (body: ReactNode) => root.render(<TooltipProvider>{body}</TooltipProvider>);
app(<Page />);

// ----- ?drive=1: real input, what it left behind -------------------------------------------

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Longer than a picked answer takes to go (ChoiceCard's COMMIT_MS). */
const settle = () => sleep(500);
const rows = () => [...document.querySelectorAll<HTMLElement>("[data-slot=action-option]")];
const selected = () => document.getSelection()?.toString().trim() ?? "";
const dots = () => rows().flatMap((row, index) => (row.querySelector("[data-slot=action-marker] span") ? [index] : []));
const lit = () => rows().flatMap((row, index) => (row.hasAttribute("data-highlighted") ? [index] : []));
const checked = () => rows().flatMap((row, index) => (row.getAttribute("aria-checked") === "true" ? [index] : []));

/** Where a word sits on the page. */
function wordBox(within: Element, word: string): DOMRect {
  const walker = document.createTreeWalker(within, NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const at = walker.currentNode.textContent?.indexOf(word) ?? -1;
    if (at < 0) continue;
    const range = document.createRange();
    range.setStart(walker.currentNode, at);
    range.setEnd(walker.currentNode, at + word.length);
    return range.getBoundingClientRect();
  }
  throw new Error(`no "${word}" on the card`);
}
const middle = (box: DOMRect) => ({ x: box.left + box.width / 2, y: box.top + box.height / 2 });
/** One click, or a double- or triple-click as the browser counts them, at a point. */
async function clicks(point: { x: number; y: number }, times = 1) {
  await fixtureInput({ type: "pointer", ...point });
  for (let count = 1; count <= times; count += 1) {
    await fixtureInput({ type: "pointerDown", ...point, count });
    await fixtureInput({ type: "pointerUp", ...point, count });
  }
}
async function drag(from: { x: number; y: number }, to: { x: number; y: number }) {
  await fixtureInput({ type: "pointer", ...from });
  await fixtureInput({ type: "pointerDown", ...from });
  for (const step of [0.25, 0.5, 0.75, 1]) {
    await fixtureInput({ type: "pointer", x: from.x + (to.x - from.x) * step, y: from.y + (to.y - from.y) * step });
  }
  await fixtureInput({ type: "pointerUp", ...to });
}

let mounts = 0;
/** A case on its own question card, freshly mounted: what it left, and what it sent or opened. */
async function run(id: string, drive: () => Promise<Record<string, unknown> | void>) {
  document.getSelection()?.removeAllRanges();
  flushSync(() => app(<div key={++mounts} className="bg-background text-foreground w-190 p-6"><Rail type="question" id={id} /></div>));
  for (let tries = 0; tries < 100 && rows().length === 0; tries += 1) await sleep(16);
  await Promise.all(document.getAnimations().map((animation) => animation.finished));
  const answers = answered.length;
  const opens = opened.length;
  const seen = await drive();
  await settle();
  return { ...seen, selected: selected(), dots: dots(), lit: lit(), checked: checked(), answered: answered.slice(answers).map((one) => [one.answers, one.input]), opened: opened.slice(opens) };
}

const toolBox = (id: string) => document.querySelector(`[data-case="${id}"] [data-slot=tool-card] [data-selectable]`)?.textContent;
const select = (element: Element | null | undefined) => (element ? getComputedStyle(element).userSelect : null);

async function census() {
  // Markdown loads on its own: the plan shows its words as plain text until then.
  for (let tries = 0; tries < 300 && document.querySelectorAll("[data-slot=plan-text] a[href]").length < 4; tries += 1) await sleep(16);
  const cases = Object.fromEntries([...document.querySelectorAll<HTMLElement>("[data-case]")].map((one) => [
    one.dataset.case!,
    [...one.querySelectorAll<HTMLAnchorElement>("a[href]")].map((link) => link.getAttribute("href")),
  ]));
  const titles = [...document.querySelectorAll<HTMLAnchorElement>("a[data-slot=card-link]")].every((link) => link.title === link.getAttribute("href"));
  const card = document.querySelector('[data-case="question"] [data-slot=action-card]')!;
  return {
    cases,
    titles,
    linksInCode: document.querySelectorAll("code a").length,
    commandBox: toolBox("command"),
    decidedCommandBox: toolBox("decided-command"),
    inputBox: toolBox("input"),
    fileStays: card.querySelector("h2")?.textContent?.includes(`[a file](file:///etc/hosts)`),
    selectable: {
      kind: select(card.querySelector("[data-slot=action-card-kind]")),
      title: select(card.querySelector("h2")),
      label: select(rows()[0]?.querySelector("span span span")),
      link: select(card.querySelector("a")),
      marker: select(card.querySelector("[data-slot=action-marker]")),
      button: select(card.querySelector("button")),
      decided: select(document.querySelector('[data-case="decided-action"] [data-slot=approval-card-text]')),
    },
    // A link in a command, clicked: it opens as any other.
    commandLink: await (async () => {
      const opens = opened.length;
      await clicks(middle(document.querySelector('[data-case="command"] [data-slot=tool-card] [data-slot=card-link]')!.getBoundingClientRect()));
      await sleep(50);
      return opened.slice(opens);
    })(),
  };
}

if (new URLSearchParams(location.search).get("drive") === "1") {
  const results: Record<string, unknown> = {};
  try {
    results.census = await census();
    const first = () => rows()[0]!;
    const canary = () => rows()[1]!;
    results.drag = await run("drag", () => drag({ x: wordBox(first(), "Deploy").left + 1, y: middle(wordBox(first(), "Deploy")).y }, { x: wordBox(first(), "now").right - 1, y: middle(wordBox(first(), "now")).y }).then(() => undefined));
    results.across = await run("across", () => drag(middle(wordBox(first(), "Deploy")), middle(wordBox(canary(), "Follow"))).then(() => undefined));
    results.shortDrag = await run("short-drag", () => drag(middle(wordBox(first(), "staging")), middle(wordBox(first(), "now"))).then(() => undefined));
    results.double = await run("double", () => clicks(middle(wordBox(first(), "staging")), 2));
    results.triple = await run("triple", () => clicks(middle(wordBox(first(), "staging")), 3));
    results.click = await run("click", () => clicks(middle(wordBox(first(), "staging"))));
    results.link = await run("link", () => clicks(middle(wordBox(canary(), "canary"))));
    results.description = await run("description", () => clicks(middle(wordBox(canary(), "notes"))));
    results.linkDrag = await run("link-drag", () => {
      const box = wordBox(canary(), "the canary plan");
      return drag({ x: box.left + 2, y: middle(box).y }, { x: box.right - 2, y: middle(box).y });
    });
    results.intoLink = await run("into-link", () => {
      const from = wordBox(canary(), "Follow");
      return drag({ x: from.left + 1, y: middle(from).y }, middle(wordBox(canary(), "plan")));
    });
    results.linkEnter = await run("link-enter", async () => {
      canary().querySelector("a")!.focus();
      await fixtureInput({ type: "key", key: "Enter" });
    });
    results.enter = await run("enter", async () => {
      canary().focus();
      await fixtureInput({ type: "key", key: "Enter" });
    });
    results.space = await run("space", async () => {
      first().focus();
      await fixtureInput({ type: "key", key: " " });
    });
    results.multiDouble = await run("multi-double", () => clicks(middle(wordBox(first(), "staging")), 2));
    results.multiTriple = await run("multi-triple", () => clicks(middle(wordBox(first(), "staging")), 3));
    results.multiClick = await run("multi-click", () => clicks(middle(wordBox(first(), "staging"))));
  } catch (error) {
    results.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "card-links-result";
  result.textContent = JSON.stringify(results);
  document.body.append(result);
}
