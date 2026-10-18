import assert from "node:assert/strict";
import { test } from "node:test";

import { linkMarkers, splitSources } from "./answerSources";

const ANSWER = [
  "Start with Meshy's free plan [1]; for exact parts use Zoo [2, 3].",
  "",
  "- Rodin gives the finest detail [3].",
  "",
  "### Sources",
  "1. [Meshy pricing](https://meshy.ai/pricing)",
  "2. [Zoo Text-to-CAD](https://zoo.dev/text-to-cad)",
  "3. [Rodin Gen-2.5](https://hyper3d.ai/)",
].join("\n");

test("an answer's trailing sources list splits off, each source numbered with its words and address", () => {
  const split = splitSources(ANSWER);
  assert.ok(split);
  assert.equal(split.head, ANSWER.split("\n\n### Sources")[0]);
  assert.deepEqual(split.sources, [
    { n: 1, title: "Meshy pricing", url: "https://meshy.ai/pricing" },
    { n: 2, title: "Zoo Text-to-CAD", url: "https://zoo.dev/text-to-cad" },
    { n: 3, title: "Rodin Gen-2.5", url: "https://hyper3d.ai/" },
  ]);
});

test("a sources list as a model writes it otherwise still splits", () => {
  const bold = splitSources("Answer.\n\n**Sources:**\n- [One](https://one.example)\n- Two: https://two.example/page.");
  assert.deepEqual(bold?.sources, [
    { n: 1, title: "One", url: "https://one.example" },
    { n: 2, title: "Two", url: "https://two.example/page" },
  ]);
  const bracketed = splitSources("Answer.\n\n## Sources\n[1] [One](https://one.example)\n[2] [Two](https://two.example)");
  assert.deepEqual(bracketed?.sources.map((source) => source.n), [1, 2]);
});

test("text with no sources list, or prose after the heading, is left whole", () => {
  assert.equal(splitSources("No sources here."), null);
  assert.equal(splitSources("Answer.\n\n### Sources\nI found none worth naming."), null);
  assert.equal(splitSources("Answer.\n\n### Sources\n"), null);
});

test("markers that name a listed source link to it; others, links and code stay as written", () => {
  const sources = splitSources(ANSWER)!.sources;
  assert.equal(
    linkMarkers("Free plan [1]; exact parts [2, 3]. Not [7], not [1](https://x.example), not `a[1]`.", sources),
    "Free plan [1](brigadier-cite:1); exact parts [2](brigadier-cite:2)[3](brigadier-cite:3). Not [7], not [1](https://x.example), not `a[1]`.",
  );
  assert.equal(linkMarkers("```\nxs[1]\n```\nsee [2]", sources), "```\nxs[1]\n```\nsee [2](brigadier-cite:2)");
});
