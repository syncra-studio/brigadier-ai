/**
 * A finished production ConversationView with an offer to start a session and a suggested
 * setting outside its collapsed work (the two cards share their corners; the setting is the
 * offer card, its icon in a tile and its buttons not indented), and an old record's Full access
 * suggestion, which isn't shown.
 */
import type { OrchestratorStepKind } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

history.replaceState(null, "", `${location.pathname}?view=done&summary=0&approvals=0`);
await import("@/fixtures/flow");
useApp.setState((state) => ({ settings: { ...state.settings, browserEnabled: false } }));
const step = (position: number, kind: OrchestratorStepKind) => ({ requestId: "r2", atMs: Date.now(), position, kind });
useBoard.setState(({ board }) => board ? {
  board: {
    ...board,
    orchestratorSteps: [
      ...board.orchestratorSteps,
      step(18, { type: "sessionOffered", id: "offer-1", title: "Fix uploads", task: "Find why uploads stall at 99%." }),
      step(19, { type: "settingSuggested", change: { setting: "browser", on: true }, reason: "Checking the page needs the built-in browser, which is off." }),
      step(20, { type: "fullAccessSuggested", reason: "Installing the tool writes outside the project." }),
    ],
  },
} : {});

/** What a CSS declaration resolves to, on a probe. */
function probe(css: string) {
  const node = document.createElement("span");
  node.style.cssText = `display: block; ${css}`;
  document.body.append(node);
  const style = getComputedStyle(node);
  const value = { radius: style.borderTopLeftRadius, width: style.width };
  node.remove();
  return value;
}

const observer = new MutationObserver(() => {
  const offer = document.querySelector<HTMLElement>('[data-kind="sessionOffered"]');
  const reply = offer?.closest('[data-slot="aui_assistant-message-root"]');
  const setting = reply?.querySelector<HTMLElement>('[data-kind="settingSuggested"]');
  if (!offer || !reply || !setting) return;
  observer.disconnect();
  const question = probe("border-radius: var(--radius-question)").radius;
  const tile = setting.querySelector(":scope > div > span");
  const actions = setting.querySelector<HTMLElement>(":scope > div:last-child");
  const result = document.createElement("pre");
  result.id = "access-thread-result";
  result.textContent = JSON.stringify({
    state: reply.getAttribute("data-state"),
    outsideWork: [offer, setting].every((card) => card.closest('[data-slot="request-work"]') === null),
    count: document.querySelectorAll('[data-kind="fullAccessSuggested"]').length,
    corners: [offer, setting].map((card) => getComputedStyle(card).borderTopLeftRadius === question),
    setting: {
      buttons: [...setting.querySelectorAll("button")].map((button) => button.textContent?.trim()),
      tiled: !!tile && getComputedStyle(tile).width === probe("width: var(--spacing-icon-button-md)").width,
      indented: !!actions && parseFloat(getComputedStyle(actions).paddingInlineStart) > 0,
    },
  });
  document.body.append(result);
});
observer.observe(document.getElementById("root")!, { childList: true, subtree: true });
