// Serves the production page on its own origin, with its content security policy on every response.
import fs from "node:fs/promises";
import path from "node:path";

import { app, protocol } from "electron";

import { pageDir } from "./identity";
import { PAGE_CSP, contentType, pageFile } from "./pagePolicy";

export function servePage(): void {
  const root = pageDir ?? path.join(app.getAppPath(), "page");
  protocol.handle("app", async (request) => {
    const file = pageFile(root, request.url);
    if (!file) return new Response("Not here", { status: 404 });
    try {
      const body = await fs.readFile(file);
      return new Response(body, {
        headers: { "Content-Type": contentType(file), "Content-Security-Policy": PAGE_CSP, "X-Content-Type-Options": "nosniff" },
      });
    } catch {
      return new Response("Not here", { status: 404 });
    }
  });
}
