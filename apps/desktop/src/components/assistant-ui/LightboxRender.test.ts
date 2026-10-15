import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("image preview geometry, zoom, pan and native download contract", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "lightbox.html", "lightbox-result", 60000, true));
  assert.equal(seen.error, undefined, seen.stack);
  assert.equal(seen.aria, "Image preview");
  assert.deepEqual(seen.labels, ["Download image", "Close image preview", "Zoom out image", "Zoom in image", "Previous image", "Next image"]);
  for (const label of ["download", "close", "previous", "next"]) {
    assert.equal(seen[label].width, 40, `${label} width`);
    assert.equal(seen[label].height, 40, `${label} height`);
  }
  for (const label of ["minus", "plus"]) { assert.equal(seen[label].width, 36); assert.equal(seen[label].height, 36); }
  assert.equal(seen.close.top, 12); assert.equal(seen.close.right, 12);
  assert.equal(seen.download.top, 12); assert.equal(seen.download.right, 60);
  assert.equal(seen.previous.left, 12); assert.equal(seen.next.right, 12);
  assert.equal(seen.counterBelow, 12);
  assert.equal(seen.initial, "100%");
  assert.deepEqual(seen.typing, { open: true, scale: "100%", count: "1 of 2" });
  assert.deepEqual(seen.first, { count: "1 of 2", disabled: true });
  assert.equal(seen.modifiedArrow, "1 of 2");
  assert.deepEqual(seen.ramp, ["110%", "125%"]);
  assert.equal(seen.minimum, "25%");
  assert.ok(seen.anchorError.x <= 1 && seen.anchorError.y <= 1, JSON.stringify(seen.anchorError));
  assert.deepEqual(seen.pan, { x: 45, y: 32 });
  assert.deepEqual(seen.saved, { name: "preview.png", bytes: seen.expectedBytes, ask: true });
  assert.deepEqual(seen.toastsAfterCancel, []);
  assert.equal(seen.inserted.fit, seen.inserted.below);
  assert.ok(parseInt(seen.inserted.fit) < 100);
  assert.ok(parseInt(seen.inserted.above) > parseInt(seen.inserted.fit));
  assert.deepEqual(seen.copied, { type: "image/png", signature: [137, 80, 78, 71, 13, 10, 26, 10] });
  assert.deepEqual(seen.copyFailure, ["Could not copy image"]);
  assert.equal(seen.closed, true);
});
