import assert from "node:assert/strict";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { cliBlocks } from "@/app/conversation/cliBlocks";
import type { ProviderEvent, ThreadView } from "@/ipc/generated";

// D9(c), D25, D29: these are production-parser/native-reader snapshots, never hand-built
// events. The ordinary node --test runner exercises every version without installing a CLI.
const root = new URL("../../../../../crates/providers/fixtures/versions/", import.meta.url);
const folders = readdirSync(root, { withFileTypes: true }).filter((entry) => entry.isDirectory());
assert.ok(folders.length > 0, "no CLI version fixtures");

for (const folder of folders) {
  const base = new URL(`${folder.name}/`, root);
  const index = JSON.parse(readFileSync(new URL("index.json", base), "utf8")) as {
    scenarios: Record<string, { view: string }>;
  };
  for (const [name, scenario] of Object.entries(index.scenarios)) {
    test(`${folder.name}/${name}: recorded live and reloaded views become thread blocks`, () => {
      const fixture = JSON.parse(readFileSync(new URL(scenario.view, base), "utf8")) as {
        events: ProviderEvent[]; live: ThreadView; persisted: ThreadView;
      };
      assert.ok(fixture.events.length > 0);
      const rendered = Object.fromEntries((["live", "persisted"] as const).map((source) => {
        const blocks = cliBlocks(fixture[source], "recorded-thread");
        assert.ok(blocks.length > 0, `${source} has no blocks`);
        assert.equal(new Set(blocks.map((block) => block.key)).size, blocks.length);
        const texts = blocks.flatMap((block) => block.texts);
        assert.equal(new Set(texts.map((text) => text.messageId)).size, texts.length, "reply duplicated after live/native merge");
        return [source, blocks.map((block) => ({
          key: block.key, state: block.state,
          user: block.user?.kind === "message" ? block.user.text : null,
          texts: block.texts.map(({ messageId, text }) => ({ messageId, text })),
          steps: block.orchestratorSteps.map((step) => step.kind),
        }))];
      }));
      const path = new URL(`${name}.blocks.json`, base);
      if (process.env.UPDATE_FIXTURES === "1") writeFileSync(path, `${JSON.stringify(rendered, null, 2)}\n`);
      assert.deepEqual(rendered, JSON.parse(readFileSync(path, "utf8")), `${fileURLToPath(path)} changed; inspect before UPDATE_FIXTURES=1`);
    });
  }
}
