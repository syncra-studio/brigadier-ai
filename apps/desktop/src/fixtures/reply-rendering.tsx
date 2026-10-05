/** Tests the production ConversationView/RequestBlock, including its cold lazy renderer. */
const query = new URLSearchParams({ view: "done", summary: "0", approvals: "0" });
history.replaceState(null, "", `${location.pathname}?${query}`);
let loadingSeen = false;
let rawMarkdownSeen = false;
const observer = new MutationObserver(() => {
  const reply = [...document.querySelectorAll('[data-slot="aui_assistant-message-content"]')].at(-1);
  if (!reply) return;
  loadingSeen ||= !!reply.querySelector('[data-slot="reply-loading"]');
  rawMarkdownSeen ||= /\*\*Theme\*\*|^- Phase/m.test(reply.textContent ?? "");
  if (!reply.querySelector("strong") || reply.querySelectorAll("li").length !== 3) return;
  observer.disconnect();
  const result = document.createElement("pre");
  result.id = "reply-rendering-result";
  result.textContent = JSON.stringify({
    loadingSeen,
    rawMarkdownSeen,
    strong: [...reply.querySelectorAll("strong")].map((element) => element.textContent),
    listItems: reply.querySelectorAll("ul > li").length,
    inlineCode: [...reply.querySelectorAll("code")].map((element) => element.textContent),
  });
  document.body.append(result);
});
observer.observe(document.getElementById("root")!, { childList: true, subtree: true, characterData: true });
export const fixtureReady = import("@/fixtures/thread-replica");

