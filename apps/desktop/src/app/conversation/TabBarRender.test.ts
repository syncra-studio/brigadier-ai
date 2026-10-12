import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

for (const compact of [false, true]) {
  test(`a session's title turns into tabs and back, measured, with widths kept after a close and Delete on a tab${compact ? " (compact)" : ""}`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixturePage(t, `tabbar.html${compact ? "?compact=1" : ""}`, "tabbar-result", 4000)) as Record<string, unknown>;
    if (process.env.TABBAR_SEEN) console.log(JSON.stringify(seen, null, 2));
    assert.equal(seen.plain, true);
    assert.equal(seen.plainTabs, 0);
    assert.match(String(seen.plainTitle), /Tidy the release notes/);
    const plain = seen.plainControls as string[];
    assert.deepEqual(plain.slice(-2), ["|", "New tab"], `+ is last, past a divider: ${plain}`);
    assert.ok(plain.indexOf("Chat actions") < plain.indexOf("Context"), `⋯ comes before the toolbar: ${plain}`);
    assert.equal(seen.opened, false);
    assert.deepEqual(seen.openedLabels, ["Tidy the release notes", "New tab"], "the chat's tab carries its title");
    const opened = seen.openedControls as string[];
    assert.deepEqual(opened.slice(opened.indexOf("New tab"), opened.indexOf("Context") + 1).filter((label) => label === "|" || label === "New tab" || label === "Context"), ["New tab", "|", "Context"], `+ follows the tabs, a divider leads the toolbar: ${opened}`);
    // 6 px, scaled with the density.
    assert.equal(seen.plusAfterLast, compact ? 5 : 6, "+ comes right after the last tab");
    assert.ok(Number(seen.plusFarFromContext) > 100, "+ is no longer at the end");
    // In px: 32 tall, corners of 10, 13 text (26 and 12 at compact).
    assert.deepEqual(seen.measure, compact ? { height: 26, radius: 10, font: 12 } : { height: 32, radius: 10, font: 13 });
    assert.equal(seen.activeShadow, true);
    const rule = seen.betweenInactive as { height: number; opacity: string; color: string };
    assert.equal(rule.height, compact ? 10 : 12);
    assert.match(rule.color, /\/ 0\.082\)$/, "the rule is the foreground at 8.2%");
    assert.equal(rule.opacity, "1");
    assert.equal((seen.besideActive as { opacity: string }).opacity, "0", "no rule beside the tab in front");
    assert.equal((seen.activeNone as { opacity: string }).opacity, "0", "no rule after the last tab");
    assert.equal(seen.clamped, true, JSON.stringify(seen.before));
    assert.equal(seen.chatActions, "1", "the chat's ⋯ shows while another tab is in front");
    assert.deepEqual(seen.scroller, { basis: "0%", toListEnd: 0 }, "the tabs' scroller takes the rest of the list, not its content's width");
    const before = seen.before as number[];
    const held = seen.held as number[];
    const released = seen.released as number[];
    assert.equal(held.length, before.length - 1);
    assert.deepEqual(held, before.slice(1), "the tabs keep their widths after a close");
    assert.ok(released.every((width, index) => width > held[index]!), `they take the room back once the pointer leaves: ${JSON.stringify({ held, released })}`);
    assert.equal(seen.fieldAfter, seen.fieldBefore, "Delete in a text field closes no tab");
    assert.equal(seen.deleted, Number(seen.fieldBefore) - 1, "Delete on a tab closes it");
    assert.equal(seen.focusAfterDelete, true, "the keyboard goes to the tab in front");
    assert.equal(seen.chatKept, seen.deleted, "Chat is never closed");
    assert.equal(seen.back, true, "closing every tab brings the title back");
  });
}
