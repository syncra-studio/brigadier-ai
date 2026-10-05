import assert from "node:assert/strict";
import { test } from "node:test";

import {
  imageMarker,
  imageMarkerAt,
  imageMarkersIn,
  namedImages,
  nextImageNumber,
  pastedImageNumbers,
  pastesInline,
  renumberImages,
  sameImages,
  splitAtImages,
} from "@/app/conversation/inlineImages";

const NOW = 1_800_000_000_000;
const LAST_WEEK = NOW - 7 * 24 * 60 * 60 * 1000;

test("a screenshot or a copied image goes inline", () => {
  // Image data: the webview makes up image.png for it at the paste.
  assert.ok(pastesInline({ files: [{ name: "image.png", type: "image/png", lastModified: NOW }], uris: "" }, NOW));
  // An image copied from a page comes with its address.
  assert.ok(pastesInline({ files: [{ name: "image.png", type: "image/png", lastModified: NOW - 20 }], uris: "https://example.com/photo.jpg" }, NOW));
});

test("a file copied in Finder, or anything not an image, goes in the attachments row", () => {
  const copied = { name: "Screenshot 2026-10-01.png", type: "image/png", lastModified: LAST_WEEK };
  assert.ok(!pastesInline({ files: [copied], uris: "" }, NOW));
  // Named like image data, but saved before.
  assert.ok(!pastesInline({ files: [{ ...copied, name: "image.png" }], uris: "" }, NOW));
  // Saved a few seconds ago.
  assert.ok(!pastesInline({ files: [{ ...copied, lastModified: NOW - 5_000 }], uris: "" }, NOW));
  assert.ok(!pastesInline({ files: [{ ...copied, lastModified: NOW }], uris: "file:///Users/me/image.png" }, NOW));
  assert.ok(!pastesInline({ files: [{ name: "notes.pdf", type: "application/pdf", lastModified: NOW }], uris: "" }, NOW));
  // One file among the images makes the whole paste attachments.
  assert.ok(
    !pastesInline(
      { files: [{ name: "image.png", type: "image/png", lastModified: NOW }, copied], uris: "" },
      NOW,
    ),
  );
  assert.ok(!pastesInline({ files: [], uris: "" }, NOW));
});

test("markers are found by number, and new images number past the highest", () => {
  const text = `Before ${imageMarker(1)} and ${imageMarker(3)}, not [Image #x] or [image #2]`;
  assert.deepEqual([...imageMarkersIn(text)], [1, 3]);
  assert.deepEqual(imageMarkerAt(text, text.indexOf("[")), { n: 1, length: "[Image #1]".length });
  assert.equal(imageMarkerAt(text, 0), null);
  assert.equal(imageMarkerAt("[Image #x]", 0), null);
  assert.equal(nextImageNumber([]), 1);
  assert.equal(nextImageNumber([1, 3]), 4);
});

test("a message's text splits around its images", () => {
  assert.deepEqual(splitAtImages("See [Image #1] and [Image #12]."), ["See ", 1, " and ", 12, "."]);
  assert.deepEqual(splitAtImages("[Image #2]"), [2]);
  assert.deepEqual(splitAtImages("No images"), ["No images"]);
});

test("a preview names pasted images rather than showing their markers", () => {
  assert.equal(
    namedImages("[Image #1] is off, unlike [Image #2]", [1]),
    "Image #1 is off, unlike [Image #2]",
  );
});

test("pasted images keep their numbers unless taken, and a marker without its image becomes words", () => {
  // Cut and pasted back: #1 is free again.
  assert.deepEqual([...pastedImageNumbers([1], [])], [[1, 1]]);
  // Copied and pasted next to themselves, or taken by an image undo could bring back.
  assert.deepEqual([...pastedImageNumbers([1, 2], [1, 2, 5])], [[1, 6], [2, 7]]);
  assert.deepEqual([...pastedImageNumbers([2, 1], [1])], [[2, 2], [1, 3]]);
  const numbers = new Map([[1, 4]]);
  assert.equal(
    renumberImages("See [Image #1] and [Image #2].", numbers),
    "See [Image #4] and Image #2.",
  );
});

test("an image pasted again is the image the composer has already, under its one number", () => {
  // Pasted again, or its chip copied and pasted: #2 is #1's image.
  assert.deepEqual([...sameImages([[2, "a"]], [[1, "a"]])], [[2, 1]]);
  // Its bytes twice in one paste, and a different image after them.
  assert.deepEqual([...sameImages([[1, "a"], [2, "a"], [3, "b"]], [])], [[2, 1]]);
  // The same as one deleted that undo could bring back; an old message's copies under their own
  // numbers lead to the first.
  assert.deepEqual([...sameImages([[4, "a"]], [[3, "a"], [1, "a"]])], [[4, 1]]);
  // A different image, or one pasted back where it was cut, keeps its number.
  assert.deepEqual([...sameImages([[2, "b"], [1, "a"]], [[3, "c"]])], []);
});
