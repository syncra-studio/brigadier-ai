/**
 * The access notice in a finished production ConversationView, outside its collapsed work, under
 * an offer to start a session (the two cards share their corners).
 */
import { useBoard } from "@/state/board";

history.replaceState(null, "", `${location.pathname}?view=done&summary=0&approvals=0`);
await import("@/fixtures/flow");
useBoard.setState(({ board }) => board ? {
  board: {
    ...board,
    orchestratorSteps: [...board.orchestratorSteps, {
      requestId: "r2",
      atMs: Date.now(),
      position: 19,
      kind: { type: "sessionOffered", id: "offer-1", title: "Fix uploads", task: "Find why uploads stall at 99%." },
    }, {
      requestId: "r2",
      atMs: Date.now(),
      position: 20,
      kind: { type: "fullAccessSuggested", reason: "Installing the tool writes outside the project." },
    }],
  },
} : {});

const observer = new MutationObserver(() => {
  const notice = document.querySelector<HTMLElement>('[data-kind="fullAccessSuggested"]');
  const reply = notice?.closest('[data-slot="aui_assistant-message-root"]');
  const offer = reply?.querySelector<HTMLElement>('[data-kind="sessionOffered"]');
  if (!notice || !reply || !offer) return;
  observer.disconnect();
  const probe = document.createElement("span");
  probe.style.borderRadius = "var(--radius-question)";
  document.body.append(probe);
  const question = getComputedStyle(probe).borderTopLeftRadius;
  probe.remove();
  const result = document.createElement("pre");
  result.id = "access-thread-result";
  result.textContent = JSON.stringify({
    state: reply.getAttribute("data-state"),
    outsideWork: notice.closest('[data-slot="request-work"]') === null,
    buttons: [...notice.querySelectorAll("button")].map((button) => button.textContent?.trim()),
    count: reply.querySelectorAll('[data-kind="fullAccessSuggested"]').length,
    corners: [notice, offer].map((card) => getComputedStyle(card).borderTopLeftRadius === question),
  });
  document.body.append(result);
});
observer.observe(document.getElementById("root")!, { childList: true, subtree: true });
