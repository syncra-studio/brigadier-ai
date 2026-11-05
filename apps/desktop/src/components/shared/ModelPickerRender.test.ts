import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the model picker: a star goes to Settings and Favorites, a pick stays open on its efforts, search crosses agents", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "model-picker.html?drive=1", "model-picker-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // With no favorites it opens on the agent in use, the selected level checked and the default tagged.
  assert.equal(seen.opensOn, "Claude Code");
  assert.deepEqual(seen.claude, ["Opus 5.5", "Fable 5.1", "Sonnet 5.5", "Haiku 5.5"]);
  assert.deepEqual(seen.levels, [["Low", "off"], ["MediumDefault", "off"], ["High", "on"], ["Extra high", "off"], ["Max", "off"]]);
  // A star is saved in Settings, and goes first in its agent's list.
  assert.deepEqual(seen.starred, [{ provider: "claude", id: "sonnet" }]);
  assert.deepEqual(seen.starredFirst, ["Sonnet 5.5", "Opus 5.5", "Fable 5.1", "Haiku 5.5"]);
  // Another model's star leaves it on the checked model's agent; the checked model's star opens Favorites.
  assert.equal(seen.otherStarredOpensOn, "Claude Code");
  assert.equal(seen.checkedStarredOpensOn, "Favorites");
  assert.deepEqual(seen.favorites, ["Opus 5.5, Claude Code", "Sonnet 5.5, Claude Code"]);
  // The search offers no autocorrect, capitals, spelling, autofill or predictions.
  assert.deepEqual(seen.searchField, ["off", "off", "false", "off", "false"]);
  // A pick keeps it open, on the picked model's levels.
  assert.deepEqual(seen.afterPick, { open: true, focused: "Max", levels: ["Max"] });
  assert.equal(seen.noEfforts, "No effort levels");
  // The model under the keyboard shows its levels, up to Ultra.
  assert.deepEqual(seen.underKeys, ["Low", "MediumDefault", "High", "Extra high", "Max", "Ultra"]);
  assert.equal(seen.searchRail, false);
  assert.deepEqual(seen.searched, [
    "Opus 5.5, Claude Code",
    "Sonnet 5.5, Claude Code",
    "Opus 4.6, Claude Code",
    "gpt-6.1-sol, Codex",
    "opencode/claude-opus-5-5, OpenCode",
    "opencode-go/kimi-k3, OpenCode",
    "opencode/big-pickle, OpenCode",
    "opencode-go/kimi-k3.1-pro, OpenCode",
  ]);
  assert.equal(seen.older, "Older models · 1");
  assert.deepEqual(seen.unfolded, ["Opus 5.5", "Sonnet 5.5", "Fable 5.1", "Haiku 5.5", "Opus 4.6"]);
  assert.equal(seen.asked, 0);
  // With no level checked, none looks selected: the keyboard is on the column.
  assert.deepEqual(seen.noneChecked, { focused: "Effort", checked: 0, levels: ["Low", "Medium", "High"] });
  // A pick that comes back late lands on its own checked level.
  assert.deepEqual(seen.lateAnswer, { focused: "Medium", state: "on", levels: ["Low", "Medium"] });
});
