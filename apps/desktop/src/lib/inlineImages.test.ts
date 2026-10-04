import assert from "node:assert/strict";
import { test } from "node:test";

import { IMAGE_UPLOAD, INLINE_IMAGE, InlineImages, imageSegments, imageToken, imageTokens, inlineRefs, isInlineImage, queuedImageRefs, reconcileImages, uploadToken } from "@/lib/inlineImages";
import type { AttachmentRef } from "@/ipc/generated";

const ref = (id: string, inline = true, mime = "image/png"): AttachmentRef => ({ id, inline, mime, name: `${id}.png`, bytes: 10, pasted: false });

test("image tokens parse and serialize with the backend's case-sensitive scanner", () => {
  const text = `a${imageToken("one")}b[Image:two][image:][image:x[y]][image:one]`;
  assert.deepEqual(imageTokens(text).map((token) => token.id), ["one", "", "one"]);
  for (const token of imageTokens(text)) assert.equal(text.slice(token.start, token.end), imageToken(token.id));
  const images = new InlineImages();
  images.retain([ref("one")]);
  images.begin("local", "upload.png");
  const source = `before${imageToken("one")}[image:unknown]${uploadToken("local")}after`;
  const parts = imageSegments(source, images, (plain) => plain ? [{ kind: "text", text: plain }] : []);
  assert.deepEqual(parts.filter((part) => part.kind === "mention").map((part) => part.type), [INLINE_IMAGE, IMAGE_UPLOAD]);
  assert.equal(parts.map((part) => part.kind === "text" ? part.text : part.type === INLINE_IMAGE ? imageToken(part.id) : uploadToken(part.id)).join(""), source);
});

test("only PNG, JPEG, GIF and WebP clipboard files qualify for inline paste", () => {
  for (const mime of ["image/png", "image/jpeg", "image/gif", "image/webp"]) assert.equal(isInlineImage(mime), true);
  for (const mime of ["image/heic", "image/tiff", "image/bmp", "image/svg+xml", "text/plain", "application/pdf", "", "image/PNG"]) assert.equal(isInlineImage(mime), false);
  assert.deepEqual(["image/png", "text/plain", "image/webp", "image/svg+xml"].map(isInlineImage), [true, false, true, false]);
});

test("pending uploads block sends and out-of-order completions retain their own refs", () => {
  const images = new InlineImages();
  images.begin("first", "first.png");
  images.begin("second", "second.png");
  assert.equal(images.blocked, true);
  assert.equal(images.complete("second", ref("b", false)), true);
  assert.equal(images.blocked, true);
  assert.equal(images.complete("first", ref("a", false)), true);
  assert.equal(images.blocked, false);
  assert.deepEqual(inlineRefs("[image:a][image:b]", images.refs.values()).map((attachment) => attachment.id), ["a", "b"]);
  const first = images.uploads.get("first");
  assert.equal(first?.status === "ready" && first.ref.id, "a");
});

test("failed uploads show the error and become sendable when removed", () => {
  const images = new InlineImages();
  images.begin("local", "shot.png");
  images.fail("local", new Error("Upload failed"));
  assert.deepEqual(images.uploads.get("local"), { status: "failed", name: "shot.png", error: "Upload failed" });
  assert.equal(images.blocked, true);
  images.sync([]);
  assert.equal(images.blocked, false);
  assert.equal(images.refs.size, 0);
});

test("deleted-before-completion and scope changes ignore late uploads even after undo", () => {
  for (const close of [false, true]) {
    const images = new InlineImages();
    images.begin("local", "shot.png");
    if (close) images.close(); else images.sync([]);
    images.sync(["local"]);
    assert.equal(images.complete("local", ref("late")), false);
    assert.equal(images.refs.size, 0);
    assert.equal(images.uploads.get("local")?.status, "failed");
  }
});

test("completed uploads retain refs across deletion and undo for draft pinning", () => {
  const images = new InlineImages();
  images.begin("local", "shot.png");
  images.complete("local", ref("shot"));
  images.sync([]);
  assert.deepEqual([...images.refs.values()], [ref("shot")]);
  assert.deepEqual(inlineRefs("deleted", images.refs.values()), []);
  images.sync(["local"]);
  assert.equal(images.blocked, false);
  assert.equal(images.uploads.get("local")?.status, "ready");
  assert.deepEqual(inlineRefs("undo [image:shot]", images.refs.values()), [ref("shot")]);
});

test("draft reload and history recall restore refs while interrupted uploads become removable errors", () => {
  const saved = JSON.parse(JSON.stringify({ text: "before[image:a]after", attachments: [ref("a"), ref("row", false), ref("deleted")] }));
  const images = new InlineImages();
  images.recall(saved.text, saved.attachments);
  assert.deepEqual(inlineRefs(saved.text, images.refs.values()), [ref("a")]);
  assert.deepEqual([...images.refs.keys()], ["a", "deleted"]);
  images.recall("history[image:history]", [ref("history")]);
  assert.deepEqual(inlineRefs("history[image:history]", images.refs.values()), [ref("history")]);
  images.recall("[image-upload:interrupted]", []);
  assert.equal(images.uploads.get("interrupted")?.status, "failed");
  assert.equal(images.blocked, true);
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
