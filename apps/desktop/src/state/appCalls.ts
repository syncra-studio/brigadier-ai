import { request } from "@/ipc/client";
import { openConversation } from "@/state/actions";
import type { AppAnswer, AppCall } from "@/ipc/generated";
import { closeRequestTabs, type PageHands, runCall } from "@/state/agentBrowser";
import { closeForAgent } from "@/state/closeForAgent";
import { openInApp } from "@/state/openInApp";
import { useApp } from "@/state/store";
import { windowView } from "@/state/windowView";

/**
 * The app calls (Q90): what an agent asks of this window, done here as the user's click would
 * be, and answered to the daemon by the call's id. A Browser tab's call is one kind. No call
 * brings the window forward or switches the session the user is on.
 */

/** Runs `call` and answers the daemon with what came of it. */
export async function runAppCall(call: AppCall, hands?: PageHands): Promise<void> {
  const result = await answerOf(call, hands);
  await request({ method: "appAnswer", id: call.id, result }).catch((error: unknown) =>
    console.error("answering an app call failed", error),
  );
}

/** What `call` comes to here. */
export async function answerOf(call: AppCall, hands?: PageHands): Promise<AppAnswer> {
  switch (call.op.app) {
    case "browser":
      return { app: "browser", result: await runCall({ ...call, op: call.op.op }, hands) };
    case "state":
      return { app: "state", view: windowView(call.conversationId) };
    case "open":
      return openInApp(call.conversationId, call.op.target, call.op.show);
    case "showSession":
      // As a click on its sidebar row: the selection moves, the window stays where it is.
      await openConversation(call.op.session);
      return { app: "sessionShown", visible: useApp.getState().windowVisible };
    case "endRequest":
      return { app: "closed", tabs: closeRequestTabs(call.conversationId, call.op.request) };
    case "closeTab":
      return closeForAgent(call.conversationId, call.op.tab, call.op.confirmed);
  }
}
