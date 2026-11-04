/**
 * Which sessions the Full access card counts as seen (#722): the real card, moved from session to
 * session as the composer moves it, while a question waits (it steps aside) or not, and while the
 * window can be seen or not. What showed at each step goes into `#full-access-seen-result`.
 */
import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";

import { FullAccessNotice } from "@/app/conversation/FullAccessNotice";
import type { Conversation } from "@/ipc/generated";
import { useApp } from "@/state/store";

const CLOSED = "brigadier.fullAccessNoticeClosed";
localStorage.removeItem(CLOSED);
useApp.setState((state) => ({ settings: { ...state.settings, showFullAccessNotice: true } }));

/** A session at Full access. */
const session = (id: string) => ({ id, setup: { type: "session", permission: "full" } }) as unknown as Conversation;

// Whether nobody can see the window, as the page reads it.
let hidden = false;
Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
function setHidden(value: boolean) {
  hidden = value;
  document.dispatchEvent(new Event("visibilitychange"));
}

type At = { id: string; aside: boolean };
let move: (at: At) => void = () => {};
function Fixture() {
  const [at, setAt] = useState<At>({ id: "none", aside: true });
  useEffect(() => {
    move = (next) => flushSync(() => setAt(next));
  }, []);
  return <FullAccessNotice conversation={session(at.id)} aside={at.aside} />;
}
createRoot(document.getElementById("root")!).render(<Fixture />);

const pause = (ms = 150) => new Promise((resolve) => setTimeout(resolve, ms));
const showing = () => document.querySelector('[data-slot="full-access-notice"]') !== null;
/** Shows session `id`, with a question waiting or not, and says whether the card is there. */
async function go(id: string, aside: boolean): Promise<boolean> {
  move({ id, aside });
  await pause();
  return showing();
}

const seen: Record<string, unknown> = {};
try {
  await pause();
  // Drawn while nobody could see the window, then a question came: it was never seen.
  setHidden(true);
  seen.hiddenDrawn = await go("hidden", false);
  await go("hidden", true);
  setHidden(false);
  seen.hiddenAfter = await go("hidden", false);
  // Seen in session A; session B, where it never showed, has a question waiting. (Each case
  // starts with it not gone, so one case's slip doesn't hide the next.)
  localStorage.removeItem(CLOSED);
  seen.aShown = await go("a", false);
  seen.bAside = await go("b", true);
  seen.closedByB = localStorage.getItem(CLOSED) !== null;
  seen.aAgain = await go("a", false);
  // Seen in A, then a question in A itself: gone, in every session.
  await go("a", true);
  seen.closed = localStorage.getItem(CLOSED) !== null;
  seen.aAfter = await go("a", false);
  seen.cAfter = await go("c", false);
} catch (error) {
  seen.error = String(error);
}
const result = document.createElement("pre");
result.id = "full-access-seen-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
