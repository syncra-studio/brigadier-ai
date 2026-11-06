import assert from "node:assert/strict";
import { test } from "node:test";

import { projectInitials, tildePath } from "@/lib/rowLabels";

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
