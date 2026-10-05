import assert from "node:assert/strict";
import { test } from "node:test";

import { MAX_ATTACHMENTS, draftRefs, imageToken, imageTokens, imagePreview, inlineRefs, isInlineImage, queuedImageRefs, reconcileImages, normalizedImages } from "@/lib/inlineImages";
import type { AttachmentRef } from "@/ipc/generated";

const ref = (id: string, inline = true, mime = "image/png"): AttachmentRef => ({ id, inline: inline ? 0 : null, mime, name: `${id}.png`, bytes: 10, pasted: false });

test("image tokens parse and serialize with the backend's case-sensitive scanner", () => {
  const text = `a${imageToken("one")}b[Image:two][image:][image:x[y]][image:one]`;
  assert.deepEqual(imageTokens(text).map((token) => token.id), ["one", "", "one"]);
  for (const token of imageTokens(text)) assert.equal(text.slice(token.start, token.end), imageToken(token.id));

});

test("only PNG, JPEG, GIF and WebP clipboard files qualify for inline paste", () => {
  for (const mime of ["image/png", "image/jpeg", "image/gif", "image/webp"]) assert.equal(isInlineImage(mime), true);
  for (const mime of ["image/heic", "image/tiff", "image/bmp", "image/svg+xml", "text/plain", "application/pdf", "", "image/PNG"]) assert.equal(isInlineImage(mime), false);
  assert.deepEqual(["image/png", "text/plain", "image/webp", "image/svg+xml"].map(isInlineImage), [true, false, true, false]);
});

test("ref derivation deduplicates known tokens and preserves literal, unknown and ineligible tokens", () => {
  const text = "[image:a][image:unknown][Image:a][image:a][image:svg][image:row]";
  assert.deepEqual(inlineRefs(text, [ref("a"), ref("svg", true, "image/svg+xml"), ref("row", false)]), [ref("a")]);
  assert.deepEqual(inlineRefs("[image:a]", [ref("a", true), ref("a", false)]), [ref("a")]);
});

test("sent-edit reconciliation drops removed images and preserves rows sharing an inline id", () => {
  const refs = [ref("a", false), ref("a"), ref("gone"), ref("document", false, "text/plain")];
  assert.deepEqual(reconcileImages("[image:a][image:a]", refs), [refs[0], refs[3], refs[1]]);
  assert.deepEqual(reconcileImages("all images removed [image:unknown]", refs), [refs[0], refs[3]]);
});


test("sent edits leave unavailable originals to the backend instead of clearing attachments", () => {
  assert.equal(reconcileImages("edited", undefined), undefined);
  assert.deepEqual(reconcileImages("edited", []), []);
  assert.deepEqual(reconcileImages("edited", [ref("row", false), ref("removed")]), [ref("row", false)]);
});

test("queue edits take rows only from the edited composer without duplicating or restoring removed rows", () => {
  const original = [ref("shared", false), ref("removed-row", false), ref("shared"), ref("removed-inline")];
  const edited = [ref("shared", false), ref("new-row", false), ref("new-inline")];
  assert.deepEqual(queuedImageRefs("[image:shared][image:shared][image:new-inline]", original, edited), [
    ref("shared", false), ref("new-row", false), ref("shared"), ref("new-inline"),
  ]);
  assert.deepEqual(queuedImageRefs("[image:shared]", original, []), [ref("shared")]);
  assert.deepEqual(queuedImageRefs("no images", original, []), []);
});

test("draft pins keep the text's images and the newest retained ones within the daemon's limit", () => {
  const retained = Array.from({ length: 30 }, (_, index) => ref(`r${index}`));
  const row = ref("row", false);
  const pinned = draftRefs(`see ${imageToken("r3")} and ${imageToken("r3")}`, [row], retained);
  assert.equal(pinned.length, MAX_ATTACHMENTS);
  assert.deepEqual(pinned.slice(0, 4).map((attachment) => attachment.id), ["row", "r3", "r29", "r28"]);
  assert.equal(pinned.filter((attachment) => attachment.id === "r3").length, 1);
  assert.deepEqual(draftRefs("", [], [ref("a"), ref("b")]).map((attachment) => attachment.id), ["b", "a"]);
});


test("plain previews replace every valid image before truncation without changing message text", () => {
  const id = "a".repeat(64);
  const text = `Иконка [image:${id}] again [image:${id}] then [image:unknown] [image:row] [image:svg] [Image:${id}] [image:broken[image:${id}]`;
  const refs = [ref(id), ref("row", false), ref("svg", true, "image/svg+xml")];
  const before = JSON.stringify({ text, refs });
  const shown = imagePreview(text, refs);
  assert.equal(shown, `Иконка [image] again [image] then [image:unknown] [image:row] [image:svg] [Image:${id}] [image:broken[image]`);
  assert.equal(imagePreview(`Иконка [image:${id}] after`, refs).slice(0, 20), "Иконка [image] after");
  assert.equal(imagePreview("[image:unknown] [image:unclosed", refs), "[image:unknown] [image:unclosed");
  assert.equal(JSON.stringify({ text, refs }), before);
});

test("numbered images reconcile repeats, clean titles and keep row images", () => {
  const image = { ...ref("numbered"), inline: 1 };
  const row = ref("row", false);
  assert.deepEqual(reconcileImages("before [Image #1] again [Image #1]", [image, row]), [row, image]);
  assert.deepEqual(reconcileImages("deleted", [image, row]), [row]);
  assert.equal(imagePreview("[Image #1] again [Image #1] literal [Image #2]", [image]), "[image] again [image] literal [Image #2]");
});

test("old main drafts and messages turn into AB numbered chips without changing unknown tokens", () => {
  const numbered = { ...ref("new"), inline: 3 };
  const result = normalizedImages("[image:old] [Image #3] [image:old] [image:unknown]", [ref("old"), numbered, ref("row", false)]);
  assert.equal(result.text, "[Image #4] [Image #3] [Image #4] [image:unknown]");
  assert.deepEqual(result.attachments.map((image) => image.inline), [4, 3, null]);
  const saved = JSON.parse('{"id":"old","name":"old.png","mime":"image/png","bytes":1,"pasted":false,"inline":true}');
  assert.equal(normalizedImages("[image:old]", [saved]).text, "[Image #1]");
  saved.inline = false;
  assert.equal(normalizedImages("[image:old]", [saved]).text, "[image:old]");
});

test("numbered queue edits keep one reference per chip number and use the edited attachment", () => {
  const original = { ...ref("old"), inline: 1 };
  const edited = { ...ref("new"), inline: 1 };
  const row = ref("row", false);
  assert.deepEqual(queuedImageRefs("[Image #1] [Image #1]", [original], [edited, row]), [row, edited]);
});
