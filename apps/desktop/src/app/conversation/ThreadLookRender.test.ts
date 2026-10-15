import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("the thread's look matches the reference's: column, tones, weight, link, corners and motion", { timeout: 120000 }, async (t) => {
  type Fade = { classed: boolean; duration: string | null };
  const seen = JSON.parse(await renderFixtureLive(t, "thread-look.html", "thread-look-result", 90000)) as {
    error?: string; column: number; chevronRest: string; chevronOpen: string; groupOpen: { duration: string; easing: string }; object: string;
    fades: Record<"li" | "hr" | "img", Fade>; prose: string; link: { href: string | null; rgb: number[] | null }; codeRadius: number;
  };
  assert.equal(seen.error, undefined);
  // The column is 736px wide; prose at weight 430; a code block's corners 16px.
  assert.deepEqual([seen.column, seen.prose, seen.codeRadius], [736, "430", 16]);
  // A web link's blue: the reference's 86, 133, 209, each channel within 1.
  const blue = [86, 133, 209];
  assert.ok(seen.link.rgb?.every((value, index) => Math.abs(value - (blue[index] ?? 0)) <= 1), `link ${String(seen.link.rgb)}`);
  // A row's object at 40% white; its chevron hidden at rest, shown while open.
  assert.match(seen.object, /\/ 0\.4\)$/);
  assert.deepEqual([seen.chevronRest, seen.chevronOpen], ["0", "1"]);
  // A group opens over 300ms, fast out.
  assert.deepEqual(seen.groupOpen, { duration: "0.3s", easing: "cubic-bezier(0.19, 1, 0.22, 1)" });
  // What arrives in a streaming reply fades in over 700ms like its words: list items, rules, images.
  for (const unit of ["li", "hr", "img"] as const) assert.deepEqual(seen.fades[unit], { classed: true, duration: "0.7s" }, unit);
});
