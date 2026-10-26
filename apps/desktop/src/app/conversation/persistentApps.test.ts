import assert from "node:assert/strict";
import { test } from "node:test";
import cases from "../../../../../crates/core/fixtures/persistent_apps.json" with { type: "json" };
import { persistentApp } from "@/app/conversation/persistentApps";

test("literal persistent apps follow the same acceptance corpus as daemon held-work", () => {
  for (const example of cases) assert.equal(persistentApp(example.command), example.app, example.command);
});
