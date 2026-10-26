import assert from "node:assert/strict";
import { test } from "node:test";
import { extractCompleteStyles, processPartialHtml } from "./partial-html";

const examples = [
  ["<div>Hello</div><div cla", "<div>Hello</div>"],
  ["<div>Hello</div><", "<div>Hello</div>"],
  ["<style>.a{color:red}</style><div>Hi</div>", "<div>Hi</div>"],
  ["<div>Hi</div><style>.a{color:", "<div>Hi</div>"],
  ['<script>alert("x")</script><div>Hi</div>', "<div>Hi</div>"],
  ['<div>Hi</div><script>fetch("/secr', "<div>Hi</div>"],
  ["<head><title>T</title></head><div>Hi</div>", "<div>Hi</div>"],
  ["<head><meta charset=", ""],
  ["<div>a &amp", "<div>a "],
  ["<div>a &#1", "<div>a "],
  ["<div>a &amp; b</div>", "<div>a &amp; b</div>"],
  ['<html><body class="x"><div>Hi</div></body></html>', "<div>Hi</div>"],
  ["<body><div>Hi</div><p>More</p>", "<div>Hi</div><p>More</p>"],
  ["<div>Hi</div>", "<div>Hi</div>"],
] as const;
for (const [input, expected] of examples) test(`preview: ${input}`, () => assert.equal(processPartialHtml(input), expected));

test("the preview keeps only complete styles", () => {
  assert.equal(extractCompleteStyles('<style>.a{color:red}</style><div>x</div><style media="all">.b{}</style>'), '<style>.a{color:red}</style><style media="all">.b{}</style>');
  assert.equal(extractCompleteStyles("<style>.a{}"), "");
  assert.equal(extractCompleteStyles("<p>No styles</p>"), "");
  assert.equal(extractCompleteStyles("<STYLE>.a{}</STYLE>"), "<STYLE>.a{}</STYLE>");
});
