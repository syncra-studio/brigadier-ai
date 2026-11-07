import assert from "node:assert/strict";
import { test } from "node:test";

Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null, setItem: () => {}, removeItem: () => {} } });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage, crypto: globalThis.crypto } });
const { elementText, fitScale } = await import("./browserDev");

test("a device shows at the share of its tab's area that fits it, and is never enlarged", () => {
  const phone = { width: 400, height: 800 };
  assert.equal(fitScale(phone, { width: 1000, height: 1000 }), 1);
  assert.equal(fitScale(phone, { width: 200, height: 1000 }), 0.5);
  assert.equal(fitScale(phone, { width: 1000, height: 200 }), 0.25);
  assert.equal(fitScale(phone, { width: 0, height: 500 }), 1, "no room measured yet");
  assert.equal(fitScale(phone, { width: 500, height: -24 }), 1);
});

const PICKED = {
  url: "http://localhost:4173/",
  selector: "#buy",
  tag: "button",
  attrs: { id: "buy", type: "button" },
  text: "Buy widget",
  password: false,
  component: "BuyButton",
  rect: { x: 1, y: 2, width: 99.6, height: 30.2 },
  styles: { display: "block", color: "red" },
};

test("a picked element reads as what it is and where", () => {
  assert.equal(elementText(PICKED), [
    "The element I picked on http://localhost:4173/:",
    "- selector: #buy",
    '- element: <button id="buy" type="button">',
    "- React component: BuyButton",
    "- text: Buy widget",
    "- size: 100×30",
    "- styles: display: block; color: red",
  ].join("\n"));
});

test("a picked password field says so, with no empty rows", () => {
  const text = elementText({ ...PICKED, tag: "input", attrs: {}, text: "", password: true, component: null });
  assert.deepEqual(text.split("\n").slice(1, 4), ["- selector: #buy", "- element: <input>", "- a password field"]);
  assert.doesNotMatch(text, /React component|- text/);
});
