import assert from "node:assert/strict";
import { test } from "node:test";

import { branchNameProblem, branchPrefixProblem } from "@/lib/refname";

test("names git takes pass, and the ones it refuses say why", () => {
  for (const good of ["main", "brigadier/abc/session", "feature/x-1", "a.b", "@x", "ünï"]) {
    assert.equal(branchNameProblem(good), null, good);
  }
  const bad: [string, string][] = [
    ["", "can't be empty"],
    ["@", '"@"'],
    ["-x", 'start with "-"'],
    ["x/", 'end with "/"'],
    ["x.", 'end with "."'],
    ["a..b", '".."'],
    ["a@{b", '"@{"'],
    ["a//b", '"//"'],
    ["a\\b", '"\\"'],
    ["a b", "spaces"],
    ["a\tb", "control characters"],
    ["a~b", '"~"'],
    ["a:b", '":"'],
    ["a[b", '"["'],
    ["a/.b", 'start with "."'],
    ["a.lock/b", 'end with ".lock"'],
  ];
  for (const [name, why] of bad) {
    const problem = branchNameProblem(name);
    assert.ok(problem?.includes(why), `${name}: ${problem}`);
  }
});

test("a prefix is checked in the names it would make", () => {
  for (const good of ["", "brigadier/", "sg-", "team/sg/", "a.b/"]) {
    assert.equal(branchPrefixProblem(good), null, good);
  }
  assert.equal(branchPrefixProblem("x.lock/"), 'No part of the prefix can end with ".lock".');
  assert.match(branchPrefixProblem("a//") ?? "", /"\/\/"/);
  assert.match(branchPrefixProblem("team.") ?? "", /end with "\."/);
  assert.match(branchPrefixProblem("-x/") ?? "", /start with "-"/);
  assert.match(branchPrefixProblem("my team/") ?? "", /spaces/);
  assert.match(branchPrefixProblem("/x") ?? "", /"\/"/);
});
