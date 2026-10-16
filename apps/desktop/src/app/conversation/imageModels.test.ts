import assert from "node:assert/strict";
import { test } from "node:test";

import { takesImages } from "@/app/conversation/imageModels";
import type { ModelInfo } from "@/ipc/generated";

const model = (inputModalities: string[]) => ({ id: "m", inputModalities }) as unknown as ModelInfo;

test("a model takes images when it says so, or says nothing of what it takes; one that takes only text holds them", () => {
  assert.equal(takesImages(model(["text", "image"])), true);
  assert.equal(takesImages(model([])), true);
  assert.equal(takesImages(null), true);
  assert.equal(takesImages(model(["text"])), false);
});
