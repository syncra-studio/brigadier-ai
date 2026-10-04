import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";

import { ScopedChecks } from "@/app/conversation/cards/ScopedChecks";
import type { Gate } from "@/ipc/generated";

const render = (verificationScope?: Gate["verificationScope"]) =>
  ReactDOMServer.renderToStaticMarkup(React.createElement(ScopedChecks, { gate: { verificationScope } as Gate }));

test("task card labels only scoped verification, including older stored gates", () => {
  assert.match(render({ type: "scoped", reason: "Small change", crates: [], desktop: true }), /Scoped checks: small change/);
  assert.equal(render({ type: "full", reason: "Risky path" }), "");
  assert.equal(render(), "");
});
