import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Button = { label: string; floating: boolean; abovePage: boolean; fromTop: number; fromEnd: number; fromBottom: number; round: string; dot: boolean } | null;
type Halves = { content: number; chat: number; whole: number; chatShare: number };

test("a tab's own chat: its button, the split, resizing, folding, hiding, the dot and New chat", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "tab-chat.html?drive=1", "tab-chat-result", 90000)) as Record<string, unknown>;
  if (process.env.TAB_CHAT_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.equal(seen.error, undefined);
  // A round button floats in the bottom-end corner of page, file and browser tabs; on macOS the
  // browser's floats over its page (beneath the app) from the end of the body, where the app's
  // cut doesn't reach; Review and New tab have none.
  const buttons = seen.buttons as Record<string, Button>;
  for (const kind of ["page", "file", "browser"]) {
    const button = buttons[kind]!;
    assert.equal(button.label, kind === "file" ? "Chat about this file" : "Chat about this page");
    assert.equal(button.floating, true, kind);
    assert.equal(button.abovePage, kind === "browser", kind);
    assert.deepEqual([button.fromEnd, button.fromBottom], [16, 16], `${kind}: 16 px from the corner`);
    assert.ok(Number.parseFloat(button.round) >= 20, `${kind}: round`);
  }
  assert.equal(buttons.review, null);
  assert.equal(buttons.newTab, null);
  // A click splits the tab half and half, the chat at the end, for this tab's page.
  const opened = seen.opened as { halves: Halves; buttonGone: boolean; header: string[]; chatAtEnd: boolean; asked: unknown[]; welcome: boolean; error: string | null; modelPicker: boolean };
  assert.equal(opened.error, null);
  assert.equal(opened.halves.chatShare, 50);
  assert.ok(Math.abs(opened.halves.content - opened.halves.chat) <= 1, JSON.stringify(opened.halves));
  assert.equal(opened.chatAtEnd, true);
  assert.equal(opened.buttonGone, true);
  assert.deepEqual(opened.header, ["New chat", "Minimize", "Hide chat"]);
  assert.equal(opened.asked.length, 1);
  assert.deepEqual((opened.asked[0] as { target: unknown; chatId: boolean }).target, { type: "page", pageId: "4".repeat(32) });
  assert.equal((opened.asked[0] as { chatId: boolean }).chatId, true, "the chat opened is the one the tab keeps");
  assert.equal(opened.welcome, true, "it says it is about the page");
  assert.equal(opened.modelPicker, true, "its composer has a model picker");
  // Dragging the divider shares the width differently, no further than three quarters; the
  // arrow keys step it.
  const dragged = seen.dragged as { share: number; halves: Halves };
  assert.equal(dragged.share, 0.6);
  assert.equal(dragged.halves.chatShare, 60);
  assert.equal(seen.draggedFar, 0.75);
  assert.deepEqual(seen.stepped, { share: 0.7, value: "70" });
  // Minimize folds it to a strip at the tab's end edge, the page full width beside it.
  const minimized = seen.minimized as { view: string; strip: { w: number }; atEnd: number; label: string; chatGone: boolean; button: Button; pageFull: boolean };
  assert.equal(minimized.view, "minimized");
  assert.equal(minimized.atEnd, 0);
  assert.ok(minimized.strip.w <= 40, `a slim strip: ${minimized.strip.w}`);
  assert.equal(minimized.label, "Show chat: Chat");
  assert.equal(minimized.chatGone, true);
  assert.equal(minimized.button, null);
  assert.equal(minimized.pageFull, true);
  assert.deepEqual(seen.expanded, { view: "split", id: true, share: 0.7 }, "the strip brings back the same chat as wide");
  // Hide closes the split and brings the button back; it opens the same chat as wide.
  const hidden = seen.hidden as { view: string; button: Button; pageFull: boolean };
  assert.equal(hidden.view, "hidden");
  assert.equal(hidden.button?.label, "Chat about this page");
  assert.equal(hidden.pageFull, true);
  assert.equal(seen.dotWorking, true, "a dot while the hidden chat works");
  assert.equal(seen.dotIdle, false);
  assert.equal(seen.dotNews, true, "a dot for what came since it was seen");
  const reopened = seen.reopened as { id: boolean; share: number; halves: Halves };
  assert.equal(reopened.id, true);
  assert.equal(reopened.share, 0.7);
  assert.equal(reopened.halves.chatShare, 70);
  // New chat starts over in the same half; the old chat is deleted.
  assert.deepEqual(seen.newChat, { fresh: true, view: "split", share: 0.7, deleted: true, opened: true });
  // A file tab's chat is about the file.
  assert.deepEqual(seen.fileChat, { target: { type: "file", path: "src/app.ts" }, welcome: true });
});
