/**
 * The Full access line above the composer, in a finished session at Full access through the
 * real ConversationView (the phase-flow fixture). `?drive=1` scrolls the thread to its bottom,
 * writes what it saw into `#full-access-notice-result`, then closes the line with ×.
 */
import { useApp } from "@/state/store";

const drive = new URLSearchParams(location.search).has("drive");
history.replaceState(null, "", `${location.pathname}?view=done&approvals=0&summary=0&sidebar=0`);
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
    viewport.scrollTop = height;
    await pause(300);
  }
  const last = [...document.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > *')].at(-1)!;
  const box = line.getBoundingClientRect();
  const seen: Record<string, unknown> = {
    text: line.querySelector("p")?.textContent ?? null,
    paragraphs: line.querySelectorAll("p").length,
    height: Math.round(box.height),
    lineHeight: Math.round(Number.parseFloat(getComputedStyle(line).lineHeight)),
    buttons: [...line.querySelectorAll("button")].map((button) => button.textContent?.trim() || button.getAttribute("aria-label")),
    lastBottom: Math.round(last.getBoundingClientRect().bottom),
    noticeTop: Math.round(box.top),
  };
  line.querySelector<HTMLButtonElement>('button[aria-label="Close"]')?.click();
  await pause();
  seen.closed = notice() === null;
  const result = document.createElement("pre");
  result.id = "full-access-notice-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

if (drive) void run();
