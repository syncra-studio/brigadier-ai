/**
 * A standalone Vite entry, excluded from the app entry and production build: the Preview card in
 * each of its states, as the user's design has them (`K53xQ`). IPC is mocked, no daemon.
 *
 * - `running`: a web preview running at localhost:3000 with two captioned screenshots taken in
 *   the browser ("Checked in the browser", each at its own time); Open, its ▾ and the fold chevron.
 * - `collapsed`: the same, folded: the header counts the screenshots.
 * - `paused`: the page paused: Start again, the screenshots kept.
 * - `stopped`: the page stopped: Start again (its own command run again), the screenshots kept.
 * - `no-proofs`: running, no screenshots: the header line alone, no chevron.
 * - `no-record`: an address with no preview record behind it: no card (#438).
 * - `proofs-only`: proofs with no page left running, from before proofs kept their kind and
 *   place: "Screenshots and recordings".
 * - `test-output`: a test run's output as a proof: "Test output", its last five lines under its
 *   command.
 * - `simulator`: an iOS simulator run on "iPhone 17" with a recording of it: Open brings Simulator forward.
 * - `pdf`: a rendered PDF whose render ended well, ready, "2 pages": Open opens the file in the system's viewer.
 * - `pdf-uncounted`: one whose page count couldn't be read: no count.
 * - `test-run`: a test run that passed, "Passed", its own output's last five lines; Run again.
 * - `test-failed`: one that failed, "Failed".
 * - `turn`: a turn's previews, found as its end finds them: the page and the test run started
 *   while its request ran, the simulator its recording names and the run its request started,
 *   though it was started again later; not a run started after.
 * - `turn-said`: a turn that started nothing, its answer naming an address no preview serves: no
 *   card (#438). `turn-said-stopped`: the address of an older preview, stopped: no card either.
 *   `turn-running`: the address of an older preview still running: its card, route and all.
 * - `turn-later-no-url`: a turn's server still running, then a server that ended before it gave an
 *   address and an app: the running server's card (#450).
 * - `turn-two-servers`: the app's server, then a smoke-test server, the answer naming the app's:
 *   the app's card (#451).
 * - `turn-stopped`: the turn's server stopped, its answer naming it: Stopped, Start again, no Open (#407).
 * - `turn-worker`: a worker's own fixture under the turn's request, stopped with its task: no card (#507).
 *
 * `?case=<id>` shows one case alone (for a screenshot); `?drive=1` writes what each case shows,
 * then clicks through it, to `<pre id="preview-card-result">` (PreviewCardRender.test.ts).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import type { ReactNode } from "react";

import { PreviewCard, TurnPreviews } from "@/app/conversation/activity/PreviewCard";
import { webPreviewUrl } from "@/app/conversation/activity/webPreview";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { ViewContext } from "@/app/conversation/viewContext";
import { driveFold } from "@/fixtures/cardFold";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, Preview, PreviewState, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const CHAT = "preview-card";

// A tall page, so a thumbnail shows its top, not the whole page shrunk.
const page = (title: string) =>
  btoa(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 1600"><rect width="640" height="1600" fill="white"/>` +
      `<text x="32" y="56" font-family="sans-serif" font-size="28" font-weight="700" fill="black">${title}</text>` +
      Array.from({ length: 30 }, (_, row) => `<rect x="32" y="${96 + row * 48}" width="576" height="36" rx="6" fill="${row % 2 ? "whitesmoke" : "lavender"}"/>`).join("") +
      `</svg>`,
  );

const TEST_LOG = ["> todo@1.0.0 test", "> node --test", "", "✓ rejects a title longer than 200 characters", "✓ accepts years 0000 to 0099", "✓ marks a past due date as overdue", "✓ a todo due today is not overdue", "15 passing (412ms)", ""].join("\n");

const calls: string[] = [];
mockIPC((command, payload) => {
  if (command === "open_pdf") {
    calls.push(`open_pdf ${(payload as { path: string }).path}`);
    return null;
  }
  if (command === "open_simulator") {
    calls.push(`open_simulator ${(payload as { device: string | null }).device}`);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "readThreadImage") {
    if (req.itemId === "test-log") return { method: req.method, data: btoa(String.fromCharCode(...new TextEncoder().encode(TEST_LOG))), mime: "text/plain" };
    const video = req.itemId.includes("video");
    return { method: req.method, data: video ? btoa("not a real recording") : page(req.itemId), mime: video ? "video/webm" : "image/svg+xml" };
  }
  calls.push(req.method === "restartPreview" ? `restartPreview ${req.previewId}` : req.method);
  if (req.method === "previewLog") return { method: req.method, tail: TEST_LOG, url: null };
  return { method: req.method };
});

const preview = (id: string, port: number, state: PreviewState): Preview => ({
  id, conversationId: CHAT, name: "npm run dev", command: "npm run dev", workdir: "/w", workspace: "/w", pid: 1, state,
  startedAtMs: 0, endedAtMs: null, log: null, url: `http://localhost:${port}/`, kind: "web", building: false,
});
const other = (id: string, kind: Preview["kind"], target: string, state: PreviewState, startedAtMs: number, command = "xcrun simctl boot"): Preview => ({
  id, conversationId: CHAT, name: kind, command, workdir: "/w/app", workspace: "/w", pid: 1, state,
  startedAtMs, endedAtMs: null, log: null, kind, target, building: false,
});
const OTHERS = [
  other("preview-sim", "simulator", "iPhone 17", { type: "running" }, 500),
  // Its render ended well, as most do: the file is there.
  other("preview-pdf", "pdf", "out/invoice.pdf", { type: "exited", code: 0, status: "exit 0" }, 2000, "pandoc invoice.md -o out/invoice.pdf"),
  other("preview-test", "test", "npm test", { type: "exited", code: 0, status: "exit 0" }, 3000, "npm test"),
  other("preview-failed", "test", "npm test", { type: "exited", code: 1, status: "exit 1" }, 3500, "npm test"),
  // Started after the turn's request ended: another turn's.
  other("preview-later", "test", "npm run e2e", { type: "exited", code: 0, status: "exit 0" }, 9000, "npm run e2e"),
  // The first turn's, started again (Run again) while the second ran: still the first's (#435).
  { ...other("preview-again", "test", "npm run lint", { type: "exited", code: 0, status: "exit 0" }, 7000, "npm run lint"), requestId: "turn-1" },
];

// Later turns' pages. turn-3: a server still running, then a second one that ended before it gave
// an address, and an app (#450). turn-4: the app's server, then a smoke-test server on another
// port, both running, its answer naming the app's (#451). turn-5: a server that stopped, its answer
// naming it (#407).
const noUrl = (each: Preview): Preview => {
  const { url: _, ...rest } = each;
  return rest;
};
const LATER_TURNS: Preview[] = [
  { ...preview("preview-t3", 4100, { type: "running" }), startedAtMs: 10100, requestId: "turn-3" },
  { ...noUrl(preview("preview-t3-failed", 4101, { type: "exited", code: 1, status: "exit 1" })), startedAtMs: 10500, requestId: "turn-3" },
  { ...noUrl(preview("preview-t3-app", 0, { type: "starting" })), kind: "app", startedAtMs: 10800, requestId: "turn-3" },
  { ...preview("preview-t4-app", 4200, { type: "running" }), startedAtMs: 13100, requestId: "turn-4" },
  { ...preview("preview-t4-smoke", 4300, { type: "running" }), startedAtMs: 13500, requestId: "turn-4" },
  { ...preview("preview-t5", 4400, { type: "stopped", reason: "stopped" }), startedAtMs: 16100, requestId: "turn-5" },
  { ...preview("preview-t6-worker", 4500, { type: "stopped", reason: "its worker's task ended" }), startedAtMs: 19100, requestId: "turn-6", taskId: "task-1" },
];
const turnRequest = (id: string, startedAtMs: number, endedAtMs: number) => ({
  id, conversationId: CHAT, preview: "", state: { type: "done" }, startedAtMs, endedAtMs, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false,
});

useBoard.setState({
  board: {
    ...emptyBoard(CHAT),
    loaded: true,
    previews: Object.fromEntries(
      [
        preview("preview-1", 3000, { type: "running" }),
        preview("preview-2", 3002, { type: "paused" }),
        preview("preview-3", 3003, { type: "stopped", reason: "stopped" }),
        // Started while the turn's request ran: its page.
        { ...preview("preview-own", 4000, { type: "running" }), startedAtMs: 1500 },
        ...OTHERS,
        ...LATER_TURNS,
      ].map((each) => [each.id, each]),
    ),
    requests: {
      "turn-1": { id: "turn-1", conversationId: CHAT, preview: "", state: { type: "done" }, startedAtMs: 1000, endedAtMs: 5000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false },
      // A later turn that started nothing.
      "turn-2": { id: "turn-2", conversationId: CHAT, preview: "", state: { type: "done" }, startedAtMs: 6000, endedAtMs: 8000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false },
      "turn-3": turnRequest("turn-3", 10000, 12000),
      "turn-4": turnRequest("turn-4", 13000, 15000),
      "turn-5": turnRequest("turn-5", 16000, 18000),
      "turn-6": turnRequest("turn-6", 19000, 21000),
    } as unknown as ReturnType<typeof emptyBoard>["requests"],
  },
});
useApp.setState({ selection: { type: "conversation", id: CHAT } });

const at22 = new Date(2026, 9, 19, 22, 11).getTime();
// A proof is attached a few minutes after it was taken: its caption reads the time it was taken.
const shot = (id: string, caption: string | undefined, minute: number): BlockOrchestratorStep => ({
  position: minute,
  atMs: at22 + (minute + 5) * 60_000,
  kind: { type: "proof", id, name: `${id}.png`, mime: "image/png", kind: "screenshot", takenOn: "http://localhost:3000/", takenAtMs: at22 + minute * 60_000, ...(caption ? { caption } : {}) },
});
const SHOTS = [shot("todo-added", "Added a todo with a due date", 0), shot("todo-checked", "Checked off an overdue todo", 1)];
// From before proofs kept their kind, place and time.
const MIXED = [
  { position: 0, atMs: at22, kind: { type: "proof", id: "readme", name: "readme.png", mime: "image/png", caption: "The install section, fixed" } },
  { position: 2, atMs: at22 + 120_000, kind: { type: "proof", id: "install-video", name: "install.webm", mime: "video/webm" } },
] as BlockOrchestratorStep[];
const TESTS = [
  { position: 0, atMs: at22 + 240_000, kind: { type: "proof", id: "test-log", name: "test.log", mime: "text/plain", kind: "test", takenOn: "npm test", caption: "The todo tests", takenAtMs: at22 + 240_000 } },
] as BlockOrchestratorStep[];

// A recording of the simulator, attached after the turn's request: it names its preview.
const SIM_PROOFS = [
  { position: 0, atMs: at22, kind: { type: "proof", id: "sim-video", name: "sim.mov", mime: "video/quicktime", kind: "recording", takenOn: "iPhone 17", preview: "preview-sim", caption: "Adding a todo on the phone", takenAtMs: at22 } },
] as BlockOrchestratorStep[];

/** A turn's end as RequestBlock lays it out, its answer read as RequestBlock reads it. */
const Turn = ({ request, steps = [], answer = "" }: { request: string; steps?: BlockOrchestratorStep[]; answer?: string }) => (
  <div className="flex flex-col gap-3">
    <TurnPreviews requestIds={[request]} steps={steps} answered={webPreviewUrl(answer)} session />
  </div>
);

const session = { id: CHAT, kind: "session", title: "Due dates", setup: { type: "session" } } as unknown as Conversation;

const CASES: { id: string; node: ReactNode }[] = [
  { id: "running", node: <PreviewCard url="http://localhost:3000/" steps={SHOTS} session /> },
  { id: "collapsed", node: <PreviewCard url="http://localhost:3000/" steps={SHOTS} session /> },
  { id: "paused", node: <PreviewCard url="http://localhost:3002/" steps={SHOTS} session /> },
  { id: "stopped", node: <PreviewCard url="http://localhost:3003/" steps={SHOTS} session /> },
  { id: "no-proofs", node: <PreviewCard url="http://localhost:3000/" steps={[]} session /> },
  { id: "no-record", node: <PreviewCard url="http://localhost:5173/" steps={[]} session /> },
  { id: "proofs-only", node: <PreviewCard url={null} steps={MIXED} session /> },
  { id: "test-output", node: <PreviewCard url={null} steps={TESTS} session /> },
  { id: "simulator", node: <PreviewCard url={null} preview={OTHERS[0]!} steps={SIM_PROOFS} session /> },
  { id: "pdf", node: <PreviewCard url={null} preview={{ ...OTHERS[1]!, pages: 2 }} steps={[]} session /> },
  { id: "pdf-uncounted", node: <PreviewCard url={null} preview={OTHERS[1]!} steps={[]} session /> },
  { id: "test-run", node: <PreviewCard url={null} preview={OTHERS[2]!} steps={[]} session /> },
  { id: "test-failed", node: <PreviewCard url={null} preview={OTHERS[3]!} steps={[]} session /> },
  { id: "turn", node: <Turn request="turn-1" steps={SIM_PROOFS} /> },
  { id: "turn-said", node: <Turn request="turn-2" answer="Run `npm run dev`; it starts on http://localhost:5173." /> },
  { id: "turn-said-stopped", node: <Turn request="turn-2" answer="It was on http://localhost:3003/ earlier." /> },
  { id: "turn-running", node: <Turn request="turn-2" answer="Still running at http://localhost:3000/#/todos." /> },
  { id: "turn-later-no-url", node: <Turn request="turn-3" /> },
  { id: "turn-two-servers", node: <Turn request="turn-4" answer="The app is up at http://localhost:4200/." /> },
  { id: "turn-stopped", node: <Turn request="turn-5" answer="It ran at http://localhost:4400/." /> },
  { id: "turn-worker", node: <Turn request="turn-6" answer="The pages worker is still building." /> },
];

const only = query.get("case");
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <ViewContext.Provider value={{ selection: { type: "conversation", id: CHAT }, conversation: session, embedded: false }}>
      <div className="flex w-190 flex-col gap-6 p-6">
        {CASES.filter((each) => !only || each.id === only).map((each) => (
          <div key={each.id} data-case={each.id}>{each.node}</div>
        ))}
      </div>
    </ViewContext.Provider>
  </TooltipProvider>,
);

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean, ms = 8000) {
  for (const end = performance.now() + ms; performance.now() < end && !check(); ) await frame();
  await wait(50);
}
const words = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const at = (id: string) => document.querySelector<HTMLElement>(`[data-case="${id}"]`);

function look(id: string) {
  const card = at(id)?.querySelector<HTMLElement>("[data-slot=preview-card]");
  const thumb = card?.querySelector("[data-slot=proof] > *")?.getBoundingClientRect();
  return {
    state: card?.dataset.state ?? null,
    title: words(card?.querySelector("[data-slot=preview-title]")),
    standing: words(card?.querySelector("[data-slot=preview-state]")),
    target: words(card?.querySelector("[data-slot=preview-target]")),
    count: words(card?.querySelector("[data-slot=preview-count]")),
    pages: words(card?.querySelector("[data-slot=preview-pages]")),
    buttons: [...(card?.querySelectorAll("button") ?? [])].filter((each) => !each.closest("[data-slot=proof],[data-slot=preview-run-output]")).map((each) => each.getAttribute("aria-label") ?? words(each)),
    heading: words(card?.querySelector("[data-slot=preview-proofs-count]")),
    where: words(card?.querySelector("[data-slot=preview-proofs-where]")),
    shell: card?.querySelector("[data-slot=shell-card]") ? [words(card.querySelector("[data-slot=shell-command]")), card.querySelector("[data-slot=shell-output] pre")?.textContent ?? null] : null,
    captions: [...(card?.querySelectorAll("[data-slot=proof] figcaption") ?? [])].map((each) => [...each.children].map((part) => words(part)).join(" ")),
    thumb: thumb ? Math.round(thumb.height) : null,
    // The split Open pill's edge, as the design's (1 px of --border).
    edge: card?.querySelector("span[data-slot=preview-open]") ? Number.parseFloat(getComputedStyle(card.querySelector("span[data-slot=preview-open]")!).borderTopWidth) : null,
    // Cropped to the top of the page: the image fills its frame, its top at the frame's.
    fit: card?.querySelector("[data-slot=proof] img") ? getComputedStyle(card.querySelector("[data-slot=proof] img")!).objectPosition : null,
  };
}

/** The states that need a click to reach: folded. */
async function reach() {
  await until(() => document.querySelectorAll("[data-slot=proof] img").length >= (only ? 0 : 6) && (!!only || document.querySelectorAll("[data-slot=shell-output]").length >= 4));
  at("collapsed")?.querySelector<HTMLButtonElement>("[data-slot=preview-fold]")?.click();
  // The folded card's body closes as it animates.
  await until(() => !at("collapsed") || look("collapsed").captions.length === 0);
  document.body.dataset.reached = "1";
}

async function drive() {
  const out: Record<string, unknown> = {};
  out.cases = Object.fromEntries(CASES.map((each) => [each.id, look(each.id)]));
  // Open opens the page in a Browser tab of the session (Settings › Browser's default).
  at("running")?.querySelector<HTMLButtonElement>("[data-slot=preview-open] > button")?.click();
  await wait(100);
  const tabs = sessionTabs(CHAT);
  const tab = tabs.tabs.find((each) => each.id === tabs.active);
  out.opened = tab?.kind === "browser" ? tab.url : null;
  // Again: the tab already on the page comes to the front, no second one.
  at("running")?.querySelector<HTMLButtonElement>("[data-slot=preview-open] > button")?.click();
  await wait(100);
  out.browserTabs = sessionTabs(CHAT).tabs.filter((each) => each.kind === "browser").length;
  // Its ▾ picks where.
  const trigger = at("running")?.querySelector<HTMLButtonElement>("[aria-label='Open in']");
  trigger?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, pointerType: "mouse" }));
  await until(() => document.querySelectorAll("[role=menuitem]").length > 0, 2000);
  out.menu = [...document.querySelectorAll("[role=menuitem]")].map((each) => words(each));
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  // Paused: Start again resumes it.
  at("paused")?.querySelector<HTMLButtonElement>("[data-slot=preview-start]")?.click();
  await until(() => calls.includes("resumePreview"), 2000);
  // The folded card opens to its proofs again.
  at("collapsed")?.querySelector<HTMLButtonElement>("[data-slot=preview-fold]")?.click();
  await wait(50);
  out.unfolded = look("collapsed").captions.length;
  // Ended: its own command again. A ready PDF opens in the system's viewer.
  at("stopped")?.querySelector<HTMLButtonElement>("[data-slot=preview-start]")?.click();
  at("test-run")?.querySelector<HTMLButtonElement>("[data-slot=preview-start]")?.click();
  at("pdf")?.querySelector<HTMLButtonElement>("[data-slot=preview-open]")?.click();
  // A running simulator's Open brings Simulator forward on its device.
  at("simulator")?.querySelector<HTMLButtonElement>("[data-slot=preview-open]")?.click();
  await until(() => calls.filter((each) => each.startsWith("restartPreview") || each.startsWith("open_")).length >= 4, 2000);
  // The turn's other previews, as its end finds them.
  const cards = (id: string) => [...(at(id)?.querySelectorAll("[data-slot=preview-card]") ?? [])].map((card) => [words(card.querySelector("[data-slot=preview-title]")), words(card.querySelector("[data-slot=preview-target]"))]);
  out.turn = cards("turn");
  // Only a preview really started makes a page's card, never an address in the answer (#438).
  out.said = [cards("turn-said"), cards("turn-said-stopped"), cards("turn-running")];
  // A later preview with no address leaves the page's card (#450); the page the answer names is
  // the card's, not the turn's latest server (#451); a page that stopped is no Open (#407).
  out.later = [cards("turn-later-no-url"), cards("turn-two-servers")];
  const stopped = look("turn-stopped");
  out.turnStopped = [stopped.title, stopped.standing, stopped.target, stopped.buttons];
  out.noRecord = !!at("no-record")?.querySelector("[data-slot=preview-card]");
  // A worker's own preview is a Background row, not the turn's page (#507).
  out.worker = cards("turn-worker");
  out.calls = calls;
  return out;
}

void reach()
  .then(() =>
    query.get("drive") === "1"
      ? drive()
      : query.get("drive") === "fold"
        ? driveFold(
            at("running")!.querySelector("[data-slot=preview-card]")!,
            at("running")!.querySelector<HTMLElement>("[data-slot=preview-open] > button")!,
            () => look("running").captions.length,
          )
        : new Promise<never>(() => {}),
  )
  .catch((error: unknown) => ({ error: String(error) }))
  .then((result) => {
    const out = document.createElement("pre");
    out.id = "preview-card-result";
    out.textContent = JSON.stringify(result);
    document.body.append(out);
  });
