import assert from "node:assert/strict";
import { test } from "node:test";

import { codePath } from "@/components/assistant-ui/code-paths";

test("inline code reads as a file path only when it looks like one", () => {
  assert.deepEqual(codePath("src/cli.ts:64"), { path: "src/cli.ts", line: 64, end: null });
  assert.deepEqual(codePath("./test/cli.test.ts"), { path: "test/cli.test.ts", line: null, end: null });
  assert.deepEqual(codePath("notes.md:3-9"), { path: "notes.md", line: 3, end: 9 });
  assert.deepEqual(codePath("package.json"), { path: "package.json", line: null, end: null });
  assert.deepEqual(codePath("/repo/src/a.rs"), { path: "/repo/src/a.rs", line: null, end: null });
  for (const code of ["Math.round", "round(2.5)", "1.5", "npm test", "a.b.c()", "--note", "e.g", "foo.bar"]) {
    assert.equal(codePath(code), null, code);
  }
});
