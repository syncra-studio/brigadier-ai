import { AssistantRuntimeProvider, ComposerPrimitive, ThreadPrimitive, useLocalRuntime } from "@/components/conversation/kit";
import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("the field shown while the composer loads turns off the system's text substitutions, as the composer does", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ComposerStandIn, keepTypedSpace } = await server.ssrLoadModule("/src/app/conversation/Composer.tsx");
    function Page() {
      const runtime = useLocalRuntime({ run: async () => ({ content: [] }) });
      const field = React.createElement(ComposerPrimitive.Root, null, React.createElement(ComposerStandIn, { placeholder: "Ask" }));
      return React.createElement(AssistantRuntimeProvider, { runtime },
        React.createElement(ThreadPrimitive.Root, null, React.createElement(ThreadPrimitive.Viewport, null, field)));
    }
    const html = ReactDOMServer.renderToStaticMarkup(React.createElement(Page));
    assert.match(html, /<textarea[^>]*aria-label="Message input"/);
    for (const attribute of ['spellcheck="false"', 'autocorrect="off"', 'autocapitalize="off"']) assert.ok(html.toLowerCase().includes(attribute), `${attribute}: ${html}`);

    // macOS's double-space period: ". " in place of the space before the caret, or of that space selected.
    const typed = (value: string, start: number, end: number, data: string, inputType = "insertReplacementText") => {
      const inserted: string[] = [];
      let prevented = false;
      const field = { value, selectionStart: start, selectionEnd: end, ownerDocument: { execCommand: (_: string, __: boolean, text: string) => inserted.push(text) } };
      keepTypedSpace({ data, inputType, currentTarget: field, preventDefault: () => (prevented = true), stopImmediatePropagation: () => {} } as unknown as InputEvent);
      return { prevented, inserted };
    };
    assert.deepEqual(typed("a ", 2, 2, ". "), { prevented: true, inserted: [" "] });
    assert.deepEqual(typed("a ", 1, 2, ". "), { prevented: true, inserted: ["  "] });
    assert.deepEqual(typed("a ", 2, 2, ". ", "insertText"), { prevented: true, inserted: [" "] });
    // Anything else goes in as typed.
    assert.deepEqual(typed("a", 1, 1, ". "), { prevented: false, inserted: [] });
    assert.deepEqual(typed("a ", 2, 2, "b"), { prevented: false, inserted: [] });
  } finally {
    await server.close();
  }
});
