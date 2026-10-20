import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Look = {
  steps: { title: string; done: boolean; current: boolean }[];
  title: string;
  buttons: string[];
  card: boolean;
  text: string;
};
type Seen = Record<"accessibility" | "screenRecording" | "capture" | "captureFailed" | "done", Look> & {
  opened: string[];
  closed: boolean;
  calls: string[];
};

const steps = (look: Look) => look.steps.map((step) => `${step.done ? "✓" : step.current ? "▸" : "·"} ${step.title.replace(/^\d/, "")}`);

// The real window in Chromium over a faked daemon.
test("computer use setup: Accessibility, Screen Recording, then a test picture, ticking as they arrive", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "computer-setup.html?drive=1", "computer-setup-result", 15000)) as Seen;
  // Opening it checks once (the helper starts for setup); after that it only reads.
  assert.deepEqual(seen.opened, ["checkComputerUse"]);

  assert.deepEqual(steps(seen.accessibility), ["▸ Accessibility", "· Screen Recording", "· Check screen capture"]);
  assert.equal(seen.accessibility.title, "Allow Accessibility");
  assert.deepEqual(seen.accessibility.buttons, ["Open System Settings", "Check again", "Show in Finder"]);
  assert.equal(seen.accessibility.card, true);

  assert.deepEqual(steps(seen.screenRecording), ["✓ Accessibility", "▸ Screen Recording", "· Check screen capture"]);
  assert.equal(seen.screenRecording.title, "Allow Screen Recording");
  assert.match(seen.screenRecording.text, /Brigadier restarts it by itself and stays open/);

  assert.deepEqual(steps(seen.capture), ["✓ Accessibility", "✓ Screen Recording", "▸ Check screen capture"]);
  assert.equal(seen.capture.title, "Check screen capture");
  assert.match(seen.capture.text, /bypass the system private window picker/);
  assert.match(seen.capture.text, /Choose Allow/);
  assert.match(seen.captureFailed.text, /The test picture didn't work: the window couldn't be captured/);

  assert.deepEqual(steps(seen.done), ["✓ Accessibility", "✓ Screen Recording", "✓ Check screen capture"]);
  assert.equal(seen.done.title, "You're all set");
  assert.equal(seen.closed, true);

  assert.deepEqual(seen.calls, [
    "checkComputerUse",
    "checkComputerUse",
    "open_privacy_settings accessibility",
    "computer_setup_drag",
    "reveal_computer_helper",
    "checkComputerUse",
    "confirmComputerCapture",
    "confirmComputerCapture",
  ]);
});
