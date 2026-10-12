import assert from "node:assert/strict";
import { test } from "node:test";

import { hasEmail, maskEmail, maskEmailsIn } from "@/lib/email";

test("an email shows its first three letters and the domain's ending only", () => {
  assert.equal(maskEmail("stephen.golban@example.com"), "ste•••••@•••••.com");
  assert.equal(maskEmail("me@mail.example.co.uk"), "me•••••@•••••.uk");
  assert.equal(maskEmail("a@localhost"), "a•••••@•••••");
});

test("text that isn't an email comes back as it is", () => {
  assert.equal(maskEmail("Work account"), "Work account");
  assert.equal(hasEmail("Work account"), false);
  assert.equal(hasEmail("work: stephen@example.com"), true);
});

test("every email in a line is masked and the words around it are kept", () => {
  assert.equal(
    maskEmailsIn("Claude Code hit its usage limit on stephen@example.com; continuing on work@corp.io."),
    "Claude Code hit its usage limit on ste•••••@•••••.com; continuing on wor•••••@•••••.io.",
  );
  assert.equal(maskEmailsIn("No address here."), "No address here.");
});
