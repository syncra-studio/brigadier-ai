import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The whole setup in Chromium over a faked daemon: when Continue without projects can't save
// (the daemon slow or gone), the setup stays on the projects and says why, so Continue can be
// pressed again; once it saves, the setup closes (#369).
test("onboarding: a failed Continue without projects stays on the step and says why", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "onboarding-finish.html?drive=1", "onboarding-finish-result", 30000)) as Record<string, unknown>;
  assert.deepEqual(seen, {
    failed: { title: "Choose your projects", error: "the daemon did not answer in time", open: true },
    saved: { title: "", error: null, open: false, onboarded: true },
    writes: ["failed", "saved"],
  });
});
