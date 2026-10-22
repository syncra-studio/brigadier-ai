import { markdownText } from "@/components/conversation/markdown-lazy";
import { preloadable } from "@/lib/preloadable";

/** The thread's cards (a worker, a question, a plan, …): they load on first use. */
export const cardBody = preloadable(() => import("@/app/conversation/cards/CardBody").then((module) => module.default));

/**
 * Loads the code a thread's replies and cards show with: a conversation opens whole, never with
 * placeholder lines where its replies go or its cards missing (#500).
 */
export function preloadThreadCode(): Promise<unknown> {
  return Promise.all([markdownText.preload(), cardBody.preload()]);
}
