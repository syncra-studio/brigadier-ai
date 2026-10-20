/**
 * The Full access card above the composer, in a finished session at Full access through the
 * real ConversationView (the phase-flow fixture). `?drive=1` scrolls the thread to its bottom,
 * writes what it saw into `#full-access-notice-result`, then closes the card with ×. With
 * `&waiting=1` the session still works with approvals waiting above the composer, and the
 * result says whether the card stepped aside for them. With `&queued=1` a message waits in the
 * queue, and the result says whether the card stepped aside for it and came back once it left.
 */
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const drive = params.has("drive");
const waiting = params.has("waiting");
const queued = params.has("queued");
history.replaceState(
  null,
  "",
  `${location.pathname}?${waiting ? "view=running" : "view=done&approvals=0"}&summary=0&sidebar=0`,
);
await import("@/fixtures/flow");

useApp.setState((state) => {
  const conversation = state.conversations["flow-session"];
  if (conversation?.setup?.type !== "session") return {};
  return {
    settings: { ...state.settings, showFullAccessNotice: true },
    conversations: {
      ...state.conversations,
      [conversation.id]: { ...conversation, setup: { ...conversation.setup, permission: "fullAccess" } },
    },
  };
});

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
const notice = () => document.querySelector<HTMLElement>('[data-slot="full-access-notice"]');

const px = (value: string) => Math.round(Number.parseFloat(value) * 10) / 10;

function report(seen: Record<string, unknown>): void {
  const result = document.createElement("pre");
  result.id = "full-access-notice-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

/** Approvals wait above the composer: whether the card is there with them. */
async function runWaiting() {
  for (let tries = 0; tries < 80 && !document.querySelector('[data-slot="approval-slot"]'); tries += 1) await pause();
  await pause(300);
  report({ approvals: document.querySelector('[data-slot="approval-slot"]') !== null, notice: notice() !== null });
}

/** The session's queue holds these messages. */
function setQueue(items: { id: string; text: string }[]): void {
  useBoard.setState(({ board }) =>
    board
      ? {
          board: {
            ...board,
            queue: {
              paused: false,
              items: items.map((item) => ({ ...item, attachments: [], mentions: [], queuedAtMs: Date.now(), editedAtMs: null, deciding: false })),
            },
          },
        }
      : {},
  );
}

/** A message waits in the queue (#448): whether the card is there with it, and back once it left. */
async function runQueued() {
  for (let tries = 0; tries < 80 && !notice(); tries += 1) await pause();
  const before = notice() !== null;
  setQueue([{ id: "q1", text: "Also tell me which Node version package.json asks for." }]);
  for (let tries = 0; tries < 30 && !document.querySelector('[data-slot="message-queue-item"]'); tries += 1) await pause();
  const rail = document.querySelector('[data-slot="message-queue-item"]') !== null;
  const withQueue = notice() !== null;
  setQueue([]);
  for (let tries = 0; tries < 30 && !notice(); tries += 1) await pause();
  report({ before, rail, withQueue, after: notice() !== null });
}

async function run() {
  for (let tries = 0; tries < 80 && !(notice() && document.querySelector('[data-slot="aui_message-group"] > *')); tries += 1) {
    await pause();
  }
  const line = notice();
  if (!line) throw new Error("No Full access line rendered");
  const viewport = document.querySelector<HTMLElement>('[data-slot="aui_thread-viewport"]')!;
  // To the bottom, again while the thread is still settling (its rows' content loading).
  let height = -1;
  for (let tries = 0; tries < 20 && height !== viewport.scrollHeight; tries += 1) {
    height = viewport.scrollHeight;
    viewport.dispatchEvent(new WheelEvent("wheel", { deltaY: 1 }));
    viewport.scrollTop = height;
    await pause(300);
  }
  const last = [...document.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > *')].at(-1)!;
  const box = line.getBoundingClientRect();
  const style = getComputedStyle(line);
  const composer = document.querySelector<HTMLElement>('[data-slot="composer"] .rounded-composer')!;
  const icon = line.querySelector("svg")!.getBoundingClientRect();
  const seen: Record<string, unknown> = {
    title: line.querySelector("p")?.textContent ?? null,
    paragraphs: line.querySelectorAll("p").length,
    fontSize: px(style.fontSize),
    padding: [px(style.paddingTop), px(style.paddingLeft)],
    icon: px(String(icon.width)),
    radius: px(style.borderTopLeftRadius),
    height: box.height,
    composerRadius: px(getComputedStyle(composer).borderTopLeftRadius),
    buttons: [...line.querySelectorAll("button")].map((button) => button.textContent?.trim() || button.getAttribute("aria-label")),
    lastBottom: Math.round(last.getBoundingClientRect().bottom),
    noticeTop: Math.round(box.top),
  };
  [...line.querySelectorAll("button")].find((button) => button.textContent?.trim() === "Close")?.click();
  await pause();
  seen.closed = notice() === null;
  report(seen);
}

if (drive) void (waiting ? runWaiting() : queued ? runQueued() : run());
