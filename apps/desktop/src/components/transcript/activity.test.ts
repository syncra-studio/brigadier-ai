import assert from "node:assert/strict";
import { test } from "node:test";

import { shownCommand } from "@/components/transcript/activity";

test("a command drops its shell wrapper, the gate folder and git's display settings", () => {
  assert.equal(
    shownCommand("/bin/zsh -lc '/Users/me/Library/Application Support/Brigadier/gate/bin/git -c core.splitIndex=false -c color.ui=never push origin main'"),
    "git push origin main",
  );
});

test("git settings that change what a command does stay in view", () => {
  assert.equal(
    shownCommand("git -c core.hooksPath=/tmp/untrusted commit -m ok"),
    "git -c core.hooksPath=/tmp/untrusted commit -m ok",
  );
  assert.equal(
    shownCommand("git -c color.ui=never -c alias.x=!sh -c user.email=a@b.c x"),
    "git -c alias.x=!sh -c user.email=a@b.c x",
  );
});
