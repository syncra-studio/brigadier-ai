import assert from "node:assert/strict";
import { test } from "node:test";

import { projectInitials, shortAge, tildePath } from "@/lib/rowLabels";

test("a row's time: now under a minute, then whole minutes, hours, days, weeks and years", () => {
  const minute = 60_000;
  const cases: [number, string][] = [
    [-5 * minute, "now"],
    [59_999, "now"],
    [minute, "1m"],
    [59 * minute, "59m"],
    [60 * minute, "1h"],
    [24 * 60 * minute, "1d"],
    [7 * 24 * 60 * minute, "1w"],
    [364 * 24 * 60 * minute, "52w"],
    [365 * 24 * 60 * minute, "1y"],
  ];
  for (const [age, word] of cases) assert.equal(shortAge(1_000_000_000_000 - age, 1_000_000_000_000), word);
});

test("a project's initials: two words' first letters, or a lone word's first two", () => {
  assert.equal(projectInitials("brigadier-ai"), "BA");
  assert.equal(projectInitials("steady-app-billing-service"), "SA");
  assert.equal(projectInitials("freelogo"), "FR");
  assert.equal(projectInitials("my project"), "MP");
  assert.equal(projectInitials("x"), "X");
});

test("a folder reads from the home folder when it is under it", () => {
  assert.equal(tildePath("/Users/ana/Development/app"), "~/Development/app");
  assert.equal(tildePath("/home/ana/app"), "~/app");
  assert.equal(tildePath("C:\\Users\\ana\\app"), "~\\app");
  assert.equal(tildePath("/opt/app"), "/opt/app");
});
