/** Exercises the real menu against files created on disk after the composer mounts. */
import { AssistantRuntimeProvider, ComposerPrimitive, useLocalRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useState } from "react";
import { createRoot } from "react-dom/client";

import { Mentions } from "@/app/conversation/Mentions";
import { ComposerMentions } from "@/components/assistant-ui/elements/composer-mentions";
import type { Conversation, Request } from "@/ipc/generated";

let calls = 0;
let settled = 0;
mockIPC(async (command, payload) => {
  const request = (payload as { request: Request }).request;
  if (command !== "ipc_request" || request.method !== "listFiles") {
    throw new Error(`Unexpected IPC: ${command}`);
  }
  calls++;
  try {
    const response = await fetch("/mention-test/files");
    if (!response.ok) throw new Error("Listing unavailable");
    return await response.json();
  } finally {
    settled++;
  }
});

const session: Conversation = {
  id: "mention-session", kind: "session", projectId: null, title: "Mention refresh",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
};

const emptySearch = () => [];
let opens = 0;
function Fixture({ conversation, inline = false }: { conversation: Conversation; inline?: boolean }) {
  const [, setOpenCount] = useState(0);
  const runtime = useLocalRuntime({ async run() { throw new Error("No model calls in this fixture"); } });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ComposerPrimitive.Root>
        <ComposerPrimitive.Unstable_TriggerPopoverRoot>
          <ComposerPrimitive.Input />
          {inline ? (
            <ComposerMentions search={emptySearch} onOpen={() => {
              opens++;
              setOpenCount(opens);
            }} />
          ) : <Mentions conversation={conversation} targets={[]} />}
        </ComposerPrimitive.Unstable_TriggerPopoverRoot>
      </ComposerPrimitive.Root>
    </AssistantRuntimeProvider>
  );
}

const root = createRoot(document.getElementById("root")!);
root.render(<Fixture conversation={session} />);
const pause = () => new Promise((resolve) => setTimeout(resolve, 25));
async function until(check: () => boolean, message: string) {
  for (let attempt = 0; attempt < 80; attempt++) {
    if (check()) return;
    await pause();
  }
  throw new Error(message);
}
async function type(text: string) {
  const input = document.querySelector("textarea")!;
  input.focus();
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(input, text);
  input.setSelectionRange(text.length, text.length);
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await pause();
}
const menu = () => document.querySelector('[data-slot="composer-mentions"]');
const hasFile = () => !!menu()?.textContent?.includes("hello.txt");
async function exercise() {
  await until(() => calls === 1, "Initial listing did not run");
  await type("@hello");
  await until(() => !!menu(), "First menu did not open");
  await until(() => !!menu()?.textContent?.includes("No results"), "Expected an empty checkout");
  await type("");
  await until(() => !menu(), "Menu did not close");
  await fetch("/mention-test/create", { method: "POST" });
  await type("@hello");
  await until(hasFile, "New file did not appear without switching sessions");
  await type("@hello.txt");
  await until(hasFile, "Typing more lost the file");
  await type("");
  await until(() => !menu(), "Menu did not close before failed refresh");
  await fetch("/mention-test/fail-next", { method: "POST" });
  await type("@hello");
  await until(() => settled === 4, "Failed refresh did not settle");
  await pause();
  if (!hasFile()) throw new Error("Failed refresh cleared the last good file list");
  const sessionCalls = calls;
  const row = [...menu()!.querySelectorAll("button")].find((button) => button.textContent?.includes("hello.txt"));
  if (!row) throw new Error("File cannot be selected");
  row.click();
  await until(() => !menu(), "Selecting the file did not close the menu");
  if (!document.querySelector("textarea")!.value.includes("@hello.txt")) throw new Error("File mention was not inserted");
  root.render(<Fixture key="chat" conversation={{ ...session, id: "chat", kind: "chat" }} />);
  await pause();
  await type("@hello");
  await until(() => !!menu(), "Plain chat menu did not open");
  await pause();
  if (hasFile()) throw new Error("Session files leaked into a plain chat");
  const chatCalls = calls - sessionCalls;
  root.render(<Fixture key="inline" conversation={session} inline />);
  await pause();
  await type("@a");
  await until(() => opens > 0, "Inline callback did not run");
  await type("@ab");
  if (opens !== 1) throw new Error("Inline callback ran again while the menu stayed open");
  await type("");
  await until(() => !menu(), "Inline menu did not close");
  await type("@a");
  await until(() => opens > 1, "Inline callback did not run on reopening");
  await pause();
  return { calls: sessionCalls, chatCalls, opens };
}
void exercise().then(
  (result) => finish(result),
  (error: unknown) => finish({ error: String(error), calls }),
);
function finish(result: object) {
  const output = document.createElement("pre");
  output.id = "mentions-refresh-result";
  output.textContent = JSON.stringify(result);
  document.body.append(output);
}
