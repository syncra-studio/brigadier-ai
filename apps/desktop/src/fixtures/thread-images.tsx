/** Production image components over a synthetic daemon; ThreadImagesRender.test.ts reads #thread-images-result. */
import { TextMessagePartProvider } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ImageTiles } from "@/app/conversation/activity/ImageRows";
import { Proofs } from "@/app/conversation/activity/Proofs";
import type { LeadStep } from "@/app/conversation/activity/group";
import { LinkConversationContext } from "@/components/conversation/file-links";
import { MarkdownText } from "@/components/conversation/markdown-text";
import { TooltipProvider } from "@/components/overlays";
import type { Request } from "@/ipc/generated";

const PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
const image = { data: PNG, mime: "image/png" };
const LOCAL: Record<string, { data: string; mime: string }> = {
  "/tmp/answer.png": image,
  "/tmp/answer copy.png": image,
  "/tmp/undrawable.png": { data: "bm90IGFuIGltYWdl", mime: "image/png" },
};
// The daemon answers from its copy, whether or not the file at the item's path is still there.
const KEPT: Record<string, { data: string; mime: string }> = { "viewed-deleted": image };

const asked: string[] = [];
mockIPC(async (command, args) => {
  const request = (args as { request?: Request } | undefined)?.request;
  if (command !== "ipc_request" || !request) return null;
  if (request.method === "readLocalImage") {
    asked.push(request.path);
    const found = LOCAL[request.path];
    if (!found) throw new Error("This file is not a supported image");
    return { method: request.method, ...found };
  }
  if (request.method === "readThreadImage") {
    const found = KEPT[request.itemId];
    if (!found) throw new Error("This image is no longer there");
    return { method: request.method, ...found };
  }
  return { method: request.method };
});

const step = (itemId: string, position: number): LeadStep => ({
  position, atMs: 0, requestId: "fixture",
  kind: { type: "tool", itemId, name: "view_image", detail: `/tmp/${itemId}.png`, status: "completed", throughPosition: 0, turn: 1, item: "image" },
});

const Answer = ({ text }: { text: string }) => (
  <TextMessagePartProvider text={text} isRunning={false}>
    <MarkdownText />
  </TextMessagePartProvider>
);

const conversationId = "thread-images-fixture";
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <LinkConversationContext.Provider value={conversationId}>
      <main className="bg-background text-foreground mx-auto flex min-h-screen max-w-thread flex-col gap-8 px-8 py-12">
        <section data-case="answer"><Answer text="![Landscape](/tmp/answer.png)" /></section>
        <section data-case="file-url"><Answer text="![Landscape](file:///tmp/answer%20copy.png)" /></section>
        <section data-case="refused"><Answer text="![Notes](/tmp/notes.png)" /></section>
        <section data-case="undrawable"><Answer text="![Broken](/tmp/undrawable.png)" /></section>
        <section data-case="relative"><Answer text="![Relative](docs/picture.png)" /></section>
        <section data-case="tile"><ImageTiles conversationId={conversationId} steps={[step("viewed-deleted", 1)]} /></section>
        <section data-case="tile-gone"><ImageTiles conversationId={conversationId} steps={[step("viewed-gone", 2)]} /></section>
        <section data-case="proof">
          <Proofs conversationId={conversationId} proofs={[{ atMs: undefined, proof: { type: "proof", id: "viewed-deleted", name: "Night landscape", mime: "image/png" } }]} />
        </section>
      </main>
    </LinkConversationContext.Provider>
  </TooltipProvider>,
);

/** Each case's drawn images (an `<img>` that decoded) and the words shown instead of one. */
function seen(name: string) {
  const section = document.querySelector(`[data-case="${name}"]`)!;
  const images = [...section.querySelectorAll("img")];
  return {
    drawn: images.filter((shown) => shown.complete && shown.naturalWidth > 0 && shown.src.startsWith("data:image/")).length,
    broken: images.filter((shown) => !(shown.complete && shown.naturalWidth > 0)).length,
    text: section.textContent?.trim() ?? "",
  };
}

setTimeout(() => {
  const cases = ["answer", "file-url", "refused", "undrawable", "relative", "tile", "tile-gone", "proof"];
  const result = document.createElement("pre");
  result.id = "thread-images-result";
  result.textContent = JSON.stringify({ asked, ...Object.fromEntries(cases.map((name) => [name, seen(name)])) });
  document.body.append(result);
}, 2000);
