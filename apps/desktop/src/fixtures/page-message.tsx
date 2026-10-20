/** Renders page context with the production user-message renderer, without a daemon. */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { Welcome } from "@/app/conversation/ConversationView";
import { ConversationComposer } from "@/app/conversation/Composer";
import { MentionMemory, mentionsIn, pageMentions, UserMessageText } from "@/app/conversation/Mentions";
import { RequestBlock } from "@/app/conversation/RequestBlock";
import { Thread } from "@/components/conversation";
import { TooltipProvider } from "@/components/overlays";
import type { Mention } from "@/ipc/generated";

const page: Mention = { type: "page", id: "page-one", title: "Launch plan <draft>" };
const messages: ThreadMessage[] = [
  { id: "from-page", role: "user", content: [{ type: "text", text: "Tick the first item." }],
    createdAt: new Date(), attachments: [], metadata: { custom: { mentions: [page], attachments: [] } } },
  { id: "plain", role: "user", content: [{ type: "text", text: "A plain follow-up." }],
    createdAt: new Date(), attachments: [], metadata: { custom: { mentions: [], attachments: [] } } },
];

function Fixture() {
  const [result, setResult] = useState("");
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages, onNew: async () => {} });
  useEffect(() => {
    const timer = setTimeout(() => {
      const memory = new MentionMemory();
      memory.recall([page]);
      setResult(JSON.stringify({
        labels: [...document.querySelectorAll('[data-slot="page-mention"]')].map((node) => node.textContent?.trim()),
        text: [...document.querySelectorAll(".aui-user-message-content")].map((node) => node.textContent),
        // The visible title never becomes a typed @mention that could grant another page.
        inferred: mentionsIn("@Launch plan <draft>", [], [page]),
        remembered: [...memory.known()],
        kept: pageMentions([page, { type: "chat", id: "chat", title: "Other chat" }]),
      }));
    }, 1000);
    return () => clearTimeout(timer);
  }, []);
  return <TooltipProvider>
    <AssistantRuntimeProvider runtime={runtime}>
      <Thread components={{ Welcome, Composer: ConversationComposer, AssistantMessage: RequestBlock, UserText: UserMessageText }} autoFocus={false} />
    </AssistantRuntimeProvider>
    {result && <pre id="page-message-result">{result}</pre>}
  </TooltipProvider>;
}

createRoot(document.getElementById("root")!).render(<Fixture />);
