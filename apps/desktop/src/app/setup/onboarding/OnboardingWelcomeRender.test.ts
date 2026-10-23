import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

// The first launch's intro greets with its title on two lines, read out as one.
test("onboarding: the intro's title", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "onboarding-welcome.html", "onboarding-welcome-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(seen, { name: "Brigadier, at your command.", lines: ["Brigadier,", "at your command."] });
});

// The intro's Continue is the steps' own, its key Return; Enter continues wherever the focus is.
test("onboarding: Enter continues from the intro", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixtureLive(t, "onboarding-welcome.html?drive=1", "onboarding-intro-result", 20000),
  ) as Record<string, unknown>;
  assert.deepEqual(seen, { label: "Continue↩", continued: 1 });
});

// The name: its field has the focus as it opens; Continue stays off for an empty field or spaces,
// comes on with a name, shows Return as its key, and Return saves the name, trimmed.
test("onboarding: the name's Continue", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixtureLive(t, "onboarding-welcome.html?step=name&drive=1", "onboarding-name-result", 20000),
  ) as Record<string, unknown>;
  assert.deepEqual(seen, {
    focused: true,
    placeholder: "Your name",
    empty: false,
    spaces: false,
    typed: true,
    hint: "↩",
    named: ["Alex"],
  });
});

// The intro fills the window's shape to its very edges: macOS rounds the window's corners, so
// the canvas is clear in them; Windows' window is square, so it reaches every corner, under the
// title bar too, with no strip along an edge on either.
for (const [platform, corner] of [["macos", 0], ["windows", 255]] as const) {
  test(`onboarding: the intro's corners on ${platform}`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(
      await renderFixtureLive(t, `onboarding-welcome.html?corners=1&platform=${platform}`, "onboarding-corners-result", 20000),
    ) as Record<string, unknown>;
    assert.deepEqual(seen, { corners: [corner, corner, corner, corner], edges: [255, 255, 255, 255], centre: 255 });
  });
}

// #569: as onboarding ends, the intro gives its window-sized buffers back at once, where they
// stayed in the web process until the canvas was collected.
test("onboarding: the intro's canvas lets go of its buffers as it goes", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "onboarding-welcome.html?leave=1", "onboarding-leave-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen, { drawn: true, mounted: false, size: [1, 1], lost: true });
});
