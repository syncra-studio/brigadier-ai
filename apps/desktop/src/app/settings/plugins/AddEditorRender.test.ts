import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("Add reuses the file editor for server settings, skill drafts, folders and edited imports", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/AddEditor.fixture.html", "result", 20000)) as {
    error?: string; sharedEditor: boolean; startsEmpty: boolean; errorKeptEditor: boolean; starter: string; folderFilled: string;
    folders: number; importSources: string; importPreview: string; server: { projects: string[]; valueNames: string[] };
    skill: { text: string; source: string }; imported: string;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.sharedEditor, true);
  assert.equal(seen.startsEmpty, true);
  assert.equal(seen.errorKeptEditor, true);
  assert.match(seen.starter, /name:\s*\ndescription:/);
  assert.match(seen.folderFilled, /Folder skill/);
  assert.equal(seen.folders, 1);
  assert.deepEqual(seen.server.projects, ["/fixture/project"]);
  assert.deepEqual(seen.server.valueNames, ["API_KEY"]);
  assert.match(seen.skill.text, /Edited instructions/);
  assert.equal(seen.skill.source, "/fixture/chosen");
  assert.match(seen.importSources, /Import from Claude Desktop/);
  assert.match(seen.importSources, /Import from Cursor/);
  assert.match(seen.importPreview, /••••••••/);
  assert.match(seen.imported, /https:\/\/edited.test\/mcp/);
});
