import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const PAGE = "https://www.example.com/docs/intro?q=1#top";

type Box = { width: number; height: number };

/** A ⋯ menu row as the fixture reads it. */
const item = (label: string, disabled = false) => ({ label, disabled });
/** The Zoom row: what its stepper shows and which of its buttons are disabled. */
const zoom = (shows: string, disabled: string[]) => ({ label: "Zoom", shows, disabled });

// The Browser tab in Chromium over a faked shell, at 1280 wide, with Show full URL off and on:
// the toolbar's measurements, the address idle and focused, site information for https, http
// and a local page, Reload while loading, the ⋯ menu and where each of its Settings rows leads,
// then the page's native actions: zoom, Print, the screenshot, the find bar and the tab's keys.
for (const full of [false, true]) {
  test(`browser tab toolbar: measurements, address, site information, loading, ⋯ menu and its destinations, zoom, find and the page keys${full ? " (Show full URL)" : ""}`, { timeout: 90000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, `browser-tab.html?drive=1${full ? "&full=1" : ""}`, "browser-tab-result", 60000)) as Record<string, unknown>;
    if (process.env.BROWSER_TAB_SEEN) console.log(JSON.stringify(seen, null, 2));

    // In px: a 48 bar with a hairline; 32 pills (padding 2) of 28 buttons and 16 icons, a 1×16
    // divider; the address at most 770; a round 32 ⋯; 13px text fading over its last 16.
    const measure = seen.measure as Record<string, Box | string | number>;
    assert.equal((measure.toolbar as Box).height, 48);
    assert.equal(measure.toolbarBorder, 1);
    assert.deepEqual(measure.navigation, { width: 97, height: 32 });
    assert.equal(measure.navigationPadding, 2);
    assert.deepEqual(measure.divider, { width: 1, height: 16 });
    for (const key of ["back", "reload", "external"]) assert.deepEqual(measure[key], { width: 28, height: 28 }, key);
    assert.deepEqual(measure.backIcon, { width: 16, height: 16 });
    assert.equal((measure.address as Box).height, 32);
    assert.equal((measure.address as Box).width, 770, "the address stops at 770 in a wide tab");
    assert.match(String(measure.addressRing), /0px 0px 0px 1px inset/, "a one pixel inset ring");
    assert.deepEqual(measure.options, { width: 32, height: 32 });
    assert.equal(measure.font, 13);
    assert.match(String(measure.fade), /calc\(100% - 16px\)/);

    // Idle: the site alone (or with Show full URL its path, query and fragment), centred; focused,
    // the whole URL from its start, without the site information button.
    const idle = full ? "example.com/docs/intro?q=1#top" : "example.com";
    assert.deepEqual(seen.idle, { value: idle, align: "center" });
    assert.deepEqual(seen.focused, { value: PAGE, align: "start", siteInfo: null, external: true });
    assert.deepEqual(seen.blurred, { value: idle, align: "center" });

    // https: a lock shown on hover; http: "Not secure"; a local page: neither warning.
    assert.deepEqual(seen.secure, { security: "secure", label: "View site information", shown: "", opacity: "0" });
    assert.match(String(seen.securePopover), /^www\.example\.comConnection is secure/);
    assert.deepEqual(seen.insecure, { security: "insecure", label: "Not secure, view site information", shown: "Not secure" });
    assert.deepEqual(seen.insecureAddress, { value: "example.com", align: "center" });
    assert.match(String(seen.insecurePopover), /^example\.comConnection is not secureDon't enter passwords/);
    assert.deepEqual(seen.local, { security: "local", label: "View site information", shown: "" });

    // Loading: the spinner stands in for Reload, and stops the load.
    assert.deepEqual(seen.loading, { busy: "true", spinner: true });

    // On a page the native rows act; at 100% there is nothing to reset.
    assert.deepEqual(seen.menu, [
      item("Find in page"),
      item("Print"),
      "|",
      zoom("100%", ["Reset zoom"]),
      "|",
      item("Take a screenshot"),
      "|",
      item("Import Browser Data…"),
      item("Downloads"),
      item("History"),
      item("Clear browsing data"),
      "|",
      item("Browser settings"),
    ]);
    assert.equal(seen.menuWidth, 240);
    assert.equal(seen.menuItemHeight, 29);

    const browser = { type: "settings", page: "browser" };
    assert.deepEqual(seen.destinations, {
      Downloads: { selection: browser, title: "Download history", dialog: "" },
      History: { selection: browser, title: "Browsing history", dialog: "" },
      "Clear browsing data": { selection: browser, title: "Browser", dialog: "Clear browsing data" },
      "Import Browser Data…": { selection: { type: "settings", page: "browserImport" }, title: "Import Browser Data", dialog: "" },
      "Browser settings": { selection: browser, title: "Browser", dialog: "" },
      "Site settings": { selection: browser, title: "Site settings", dialog: "" },
    });

    // The New tab page has no site: no site information, nothing to open outside, and Back faint.
    assert.deepEqual(seen.newTab, { value: "", align: "center", placeholder: "Search or enter a URL", siteInfo: null, external: false });
    assert.deepEqual(seen.newTabBack, { disabled: true, opacity: "0.4" });

    assert.deepEqual(seen.asked, [{ go: "stop" }, { open: PAGE }]);
    // Enter in the address goes there in the tab only: the form has no default button that would
    // also open the address in the system browser.
    assert.deepEqual(seen.addressEnter, { submitters: 0, asked: [{ navigate: "https://example.org/next" }] });

    // Zoom steps 10% and resets to 100%; the screenshot says where it went; each asks the shell.
    assert.deepEqual(seen.zoomStepped, { shows: "110%", group: zoom("110%", []) });
    assert.equal(seen.zoomReset, "100%");
    assert.deepEqual(seen.screenshotToast, ["Screenshot saved to clipboard"]);
    const id = "c-browser";
    assert.deepEqual(seen.menuAsked, [
      { zoom: 1.1, id },
      { zoom: 1.2, id },
      { zoom: 1.1, id },
      { zoom: 1, id },
      { snapshot: id },
      { print: id },
    ]);
    // ⌘= (also with Shift, ⌘+), ⌘-, ⌘0, ⌘R, ⇧⌘R, ⌘← and ⌘→ from the app or from the pane a
    // native page's menu keys reach; ⌘← in the address field stays the field's. With the page
    // holding the keyboard, ⌘←/⌘→ spare its fields and ⌘L takes the keyboard back first.
    assert.deepEqual(seen.keysAsked, [
      { zoom: 1.1, id },
      { zoom: 1, id },
      { zoom: 1.1, id },
      { zoom: 1, id },
      { go: "reload" },
      { go: "force-reload" },
      { go: "back" },
      { go: "forward" },
      { go: "back-unless-typing" },
      { go: "forward-unless-typing" },
      { focus: null },
    ]);
    assert.equal(seen.addressFromPage, true);

    // The find bar: under the toolbar at its end, 340 by 44 with 20 corners, 13px text, the
    // page right under it; it opens with the keyboard in its field.
    assert.deepEqual(seen.findOpen, { focused: true, placeholder: "Find in page…", count: "" });
    assert.deepEqual(seen.findMeasure, { width: 340, height: 44, radius: 20, endGap: 8, underToolbar: 0, pageBelow: 0, font: 13 });
    assert.equal(seen.typed, "1 of 3");
    assert.equal(seen.enter, "2 of 3");
    assert.equal(seen.commandG, "3 of 3");
    assert.equal(seen.shiftEnter, "2 of 3");
    assert.equal(seen.commandShiftG, "1 of 3");
    assert.deepEqual(seen.noResults, { count: "No results", previous: true, next: true });
    // Per tab: another tab has neither the bar nor the zoom; back, both are as they were.
    assert.deepEqual(seen.otherTab, { bar: false, zoom: "100%" });
    assert.deepEqual(seen.backTab, { query: "foo", count: "1 of 3", zoom: "110%" });
    // Esc closes it, keeping the text for next time, clears the match and refocuses the page.
    assert.deepEqual(seen.closed, { bar: false, kept: "foo" });
    assert.deepEqual(seen.findAsked, [
      { find: "foo", step: "type", id },
      { find: "foo", step: "next", id },
      { find: "foo", step: "next", id },
      { find: "foo", step: "previous", id },
      { find: "foo", step: "previous", id },
      { find: "fooz", step: "type", id },
      { find: "foo", step: "type", id },
      { zoom: 1.1, id },
      { find: "", step: "type", id },
      { focus: id },
    ]);

    // The New tab page has nothing to find, print, zoom or picture.
    assert.deepEqual(seen.newTabMenu, [
      item("Find in page", true),
      item("Print", true),
      zoom("100%", ["Zoom out", "Zoom in", "Reset zoom"]),
      item("Take a screenshot", true),
    ]);
    assert.equal(seen.newTabFind, false);
  });
}
