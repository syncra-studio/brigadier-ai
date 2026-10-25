import assert from "node:assert/strict";
import { test } from "node:test";

import { unmarked } from "@/app/conversation/findWords";

test("Find's words for a reply drop Markdown's marks and keep code as written", () => {
  assert.equal(unmarked("## A **bold** [link](https://x.y) and ~~gone~~"), "A bold link and gone");
  assert.equal(unmarked("- item one\n1. item two\n> quoted"), "item one\nitem two\nquoted");
  // Inline code keeps its underscores and stars; only its backticks go.
  assert.equal(unmarked("Call `__init__` or ``a `b` c`` with **care**"), "Call __init__ or a `b` c with care");
  // A fence's lines go; what is between them shows as written.
  assert.equal(unmarked("```python\ndef __init__(self, *args):\n    # **not** emphasis\n```\nAfter __it__"), "\ndef __init__(self, *args):\n    # **not** emphasis\n\nAfter it");
});
