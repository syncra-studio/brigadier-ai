import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { MAX_ATTACHMENT_BYTES, droppedFile } from "./dropped.ts";

test("a dropped file is read; a path that wasn't dropped, a dropped folder, or one too large is not", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-drops-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = path.join(root, "notes.txt");
  const other = path.join(root, "other.txt");
  const folder = path.join(root, "folder");
  const large = path.join(root, "large.bin");
  const limit = path.join(root, "limit.bin");
  fs.writeFileSync(file, "notes");
  fs.writeFileSync(other, "secret");
  fs.mkdirSync(folder);
  fs.writeFileSync(large, "");
  fs.truncateSync(large, MAX_ATTACHMENT_BYTES + 1);
  fs.writeFileSync(limit, "");
  fs.truncateSync(limit, MAX_ATTACHMENT_BYTES);
  const dropped = [file, folder, large, limit];

  assert.equal(Buffer.from(await droppedFile(dropped, file)).toString(), "notes");
  assert.equal((await droppedFile(dropped, limit)).length, MAX_ATTACHMENT_BYTES);
  await assert.rejects(droppedFile(dropped, other), { message: "Only a dropped file can be read" });
  await assert.rejects(droppedFile(dropped, folder), { message: `${folder} isn't a file` });
  await assert.rejects(droppedFile(dropped, large), { message: "large.bin is too large to attach (maximum 10 MB)" });
});
