/**
 * Send feedback over a faked app: the sidebar's foot and the dialog, its daemon faked (a writer
 * that makes a bug, a repeat of #12 and an idea; a filer that files them as @stephen-golban).
 * ?drive=1 opens it from the foot's Help menu, writes a note, sends it, closes it while it
 * works and opens it again to what was filed, follows a link, then sends signed out, with every
 * filing failing (and Try again), and with no writer (one form opens), and opens a draft; it
 * writes what it saw to #feedback-result (FeedbackRender.test.ts reads it). ?look=1 sets the
 * open dialog's parts beside the app's shared ones and writes their looks to #feedback-look.
 * ?state=empty|filled|signedout|working|done|browser|failed shows the design's state, for
 * screenshots.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/shell/feedback/FeedbackDialog";
import { SidebarFoot } from "@/app/shell/SidebarFoot";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { Canon, canon, look, type LookPairs } from "@/fixtures/dialogLook";
import type { FeedbackFiling, FeedbackReport, ProviderOverview } from "@/ipc/generated";
import { closeFeedback, COULDNT_REACH, type FeedbackResult, openFeedback, useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
// Driven, nothing animates (see the archived chats fixture).
if (params.has("drive") || params.has("look")) {
  const still = document.createElement("style");
  still.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(still);
}

const report = (kind: "bug" | "idea", title: string, fields: Partial<FeedbackReport>): FeedbackReport => ({
  kind,
  title,
  area: "",
  steps: "",
  expected: "",
  actual: "",
  problem: "",
  today: "",
  wanted: "",
  duplicate: null,
  ...fields,
});
const REPO = "https://github.com/syncra-studio/brigadier-ai/issues";
const reports = [
  report("bug", "Stop leaves the tab spinning", {
    area: "Sessions, workers, landing or review",
    steps: "Not given; see What happened.",
    expected: "It stops.",
    actual: "The tab keeps spinning after Stop.",
  }),
  report("bug", "Usage bar lags after a session ends", {
    actual: "The 5-hour bar only moves after reopening.",
    duplicate: { number: 12, title: "Usage popover doesn't refresh", url: `${REPO}/12`, open: true, comment: "Seen again." },
  }),
  report("idea", "Pin a chat", { problem: "Chats get lost in the list.", wanted: "A pin on a chat." }),
];
const opened: string[] = [];
const asked: { method: string; text?: string; items?: FeedbackFiling[] }[] = [];
/** The faked daemon: whether a writer runs, `gh` is signed in, and every filing fails. */
const daemon = { writer: true, signedIn: true, failing: false, hold: null as Promise<void> | null };
const state = params.get("state");
if (state === "signedout" || state === "browser") daemon.signedIn = false;
mockIPC(async (command, payload) => {
  if (command === "system_name") return "macOS 27.0 Tahoe (aarch64)";
  if (command === "open_url") {
    opened.push((payload as { url: string }).url);
    return null;
  }
  const request = (payload as { request?: { method: string; text?: string; items?: FeedbackFiling[] } }).request;
  if (request) asked.push(request);
  if (request?.method === "feedbackAccount") {
    return { method: request.method, signedIn: daemon.signedIn, login: daemon.signedIn ? "stephen-golban" : null };
  }
  if (request?.method === "writeUpFeedback") {
    await daemon.hold;
    if (!daemon.writer) throw { code: "invalid", message: "no model can write the report now" };
    return { method: request.method, writeUp: { reports, searched: true } };
  }
  if (request?.method === "fileFeedback") {
    return {
      method: request.method,
      signedIn: daemon.signedIn,
      results: !daemon.signedIn
        ? []
        : (request.items ?? []).map((item, index) =>
            daemon.failing
              ? { outcome: "failed", reason: "HTTP 502: Bad gateway" }
              : item.commentOn === null
                ? { outcome: "filed", number: 601 + index, url: `${REPO}/${601 + index}` }
                : { outcome: "commented", number: item.commentOn, url: `${REPO}/${item.commentOn}#issuecomment-1` },
          ),
    };
  }
  if (command === "ipc_request") return { method: request?.method };
  return null;
});

const provider = (kind: "claude" | "codex", version: string | null): ProviderOverview => ({
  provider: kind,
  status:
    version === null
      ? null
      : { provider: kind, path: null, version, loggedIn: true, authMethod: null, plan: null } as ProviderOverview["status"],
  models: null,
  quota: null,
  usage: null,
  error: null,
  checkedAtMs: null,
});
useApp.setState((app) => ({
  info: { ...app.info, version: "0.1.0", platform: "macos" } as typeof app.info,
  providers: {
    ...app.providers,
    view: {
      providers: [provider("claude", "2.1.4"), provider("codex", params.has("drive") ? null : "0.163.0")],
      sessions: [],
      fixtures: [],
    },
  },
}));
/** The design's note, written in its states. */
const NOTE =
  "The usage popover keeps the old numbers after a session ends, until I restart. Stop on a worker doesn't stop it either, it carries on writing files. And could I pin a chat to the top of the sidebar?";
const RESULTS: FeedbackResult[] = [
  { kind: "bug", title: "Usage popover doesn't refresh when a session ends", outcome: "filed", number: 482, url: `${REPO}/482` },
  { kind: "bug", title: "Stop doesn't stop a running worker", outcome: "added", number: 417, url: `${REPO}/417` },
  { kind: "idea", title: "Pin a chat to the top of the sidebar", outcome: "filed", number: 483, url: `${REPO}/483` },
];
// Undriven, it opens in the state asked for (else filled in), for a screenshot beside the design.
if (!params.has("drive")) {
  openFeedback(state === "empty" ? null : { kind: "bug", text: NOTE });
  if (state === "working") useFeedbackUi.setState({ sending: { phase: "working", step: { step: "writing" } } });
  if (state === "done") useFeedbackUi.setState({ sending: { phase: "done", results: RESULTS } });
  if (state === "browser") {
    useFeedbackUi.setState({
      sending: {
        phase: "done",
        results: RESULTS.map((result) =>
          result.outcome === "added" ? { ...result, outcome: "known" as const } : { kind: result.kind, title: result.title, outcome: "opened" as const },
        ),
      },
    });
  }
  if (state === "failed") useFeedbackUi.setState({ sending: { phase: "failed", error: COULDNT_REACH, results: null } });
}

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const dialog = () => document.querySelector<HTMLElement>('[data-slot="feedback"]');
const button = (name: string, root: ParentNode = document) =>
  [...root.querySelectorAll<HTMLButtonElement>("button")].find(
    (element) => text(element) === name || element.getAttribute("aria-label") === name,
  );
const type = (area: HTMLTextAreaElement, value: string) => {
  Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set?.call(area, value);
  area.dispatchEvent(new Event("input", { bubbles: true }));
};
/** A shared part's look with the muted words' colour. */
const muted = (shared: Record<string, string> | null) => {
  const words = document.createElement("span");
  words.className = "text-muted-foreground";
  document.body.append(words);
  const color = getComputedStyle(words).color;
  words.remove();
  return shared && { ...shared, color };
};
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function Fixture() {
  const [result, setResult] = useState("");
  const [looked, setLooked] = useState("");
  useEffect(() => {
    if (!params.has("look")) return;
    void (async () => {
      for (let tries = 0; tries < 100 && !dialog()?.querySelector("textarea"); tries++) await wait(50);
      const open = dialog();
      // The note as it rests, as the shared field is measured.
      open?.querySelector("textarea")?.blur();
      await wait(100);
      const pairs: LookPairs = {
        card: [look(open, "surface"), canon("dialog", "surface")],
        title: [look(open?.querySelector("h2"), "text"), canon("title", "text")],
        line: [look(open?.querySelector("p"), "text"), canon("line", "text")],
        close: [look(button("Close", open ?? document), "button"), canon("icon", "button")],
        note: [look(open?.querySelector("textarea"), "surface"), canon("input", "surface")],
        footer: [look(open?.lastElementChild, "edgeTop"), canon("divider", "edgeTop")],
        // A ghost button, its words grey as the design has them.
        github: [look(button("Write it on GitHub yourself", open ?? document), "button"), muted(canon("ghost", "button"))],
        send: [look(button("Send", open ?? document), "button"), canon("primary", "button")],
      };
      setLooked(JSON.stringify(pairs));
    })();
  }, []);
  useEffect(() => {
    if (!params.has("drive")) return;
    let stopped = false;
    void (async () => {
      const seen: Record<string, unknown> = {};
      const sent = () => [...(dialog()?.querySelectorAll('[aria-label="Sent"] li') ?? [])].map(text);
      const footer = () => [...(dialog()?.lastElementChild?.querySelectorAll("button") ?? [])].map(text);
      const write = async (note: string) => {
        const area = dialog()?.querySelector("textarea");
        if (area) type(area, note);
        await wait(100);
      };
      for (let tries = 0; tries < 100 && !button("Help"); tries++) await wait(50);
      await wait(200);
      button("Help")?.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0 }));
      await wait(200);
      [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((item) => text(item) === "Send feedback…")?.click();
      await wait(300);
      const open = dialog();
      const area = open?.querySelector("textarea");
      seen.opened = open !== null;
      seen.title = text(open?.querySelector("h2"));
      seen.description = text(open?.querySelector("p"));
      seen.kinds = open?.querySelectorAll('[role="radio"]').length;
      seen.placeholder = area?.placeholder;
      seen.focused = document.activeElement === area;
      seen.account = text(area?.nextElementSibling);
      seen.footer = footer();
      seen.sendDisabled = button("Send", open ?? document)?.disabled;
      seen.width = Math.round(open?.getBoundingClientRect().width ?? 0);
      button("Write it on GitHub yourself", open ?? document)?.click();
      await write("The tab keeps spinning after Stop, and the usage bar lags. Also, let me pin a chat.");
      seen.sendEnabled = button("Send", dialog() ?? document)?.disabled === false;
      // The writer takes its time: the dialog says what it's at, and closing it doesn't stop it.
      let release: (() => void) | undefined;
      daemon.hold = new Promise((resolve) => (release = resolve));
      button("Send", dialog() ?? document)?.click();
      await wait(200);
      seen.working = [text(dialog()?.querySelector('[data-slot="progress-line"]')), footer()];
      button("Close", dialog() ?? document)?.click();
      await wait(200);
      seen.closedWhileWorking = [useFeedbackUi.getState().open, useFeedbackUi.getState().sending.phase];
      release?.();
      daemon.hold = null;
      await wait(300);
      openFeedback();
      await wait(300);
      seen.asked = asked.filter((request) => request.method !== "feedbackAccount").map((request) => request.method);
      const writing = asked.find((request) => request.method === "writeUpFeedback");
      const filing = asked.find((request) => request.method === "fileFeedback");
      seen.wrote = writing?.text;
      seen.filed = filing?.items?.map((item) => [item.title, item.commentOn]);
      seen.body = filing?.items?.[0]?.body.split("\n\n").slice(0, 3);
      seen.sentTitle = text(dialog()?.querySelector("h2"));
      seen.sentDescription = text(dialog()?.querySelector("p"));
      seen.rows = sent();
      seen.sentFooter = footer();
      button(`#${601}`, dialog() ?? document)?.click();
      await wait(100);
      seen.links = [...opened];
      button("Done", dialog() ?? document)?.click();
      await wait(300);
      seen.stillOpen = useFeedbackUi.getState().open;

      // Signed out: each new report's form opens; the repeat shows the issue it repeats.
      daemon.signedIn = false;
      opened.length = 0;
      openFeedback();
      await wait(300);
      seen.signedOutAccount = text(dialog()?.querySelector("textarea")?.nextElementSibling);
      await write("The tab keeps spinning after Stop, and the usage bar lags. Also, let me pin a chat.");
      button("Send", dialog() ?? document)?.click();
      await wait(400);
      seen.signedOut = [text(dialog()?.querySelector("p")), sent(), opened.map((url) => new URL(url).searchParams.get("template"))];
      closeFeedback();
      daemon.signedIn = true;

      // Every filing fails: the note stays, Send becomes Try again, which files only those again.
      daemon.failing = true;
      openFeedback();
      await wait(300);
      await write("The tab keeps spinning after Stop.");
      button("Send", dialog() ?? document)?.click();
      await wait(400);
      seen.failed = [dialog()?.querySelector("textarea")?.value, text(dialog()?.querySelector('[role="alert"]')), footer()];
      daemon.failing = false;
      asked.length = 0;
      button("Try again", dialog() ?? document)?.click();
      await wait(400);
      seen.retried = [asked.filter((request) => request.method !== "feedbackAccount").map((request) => request.method), sent()];
      closeFeedback();

      // No writer: the note opens in one form, the bug form when no kind is known.
      daemon.writer = false;
      opened.length = 0;
      openFeedback();
      await wait(300);
      await write("The tab keeps spinning\nafter I stop the session.");
      button("Send", dialog() ?? document)?.click();
      await wait(400);
      seen.fallback = opened.map((url) => Object.fromEntries(new URL(url).searchParams));
      seen.fallbackRows = [text(dialog()?.querySelector("p")), sent()];
      closeFeedback();
      // A draft from elsewhere opens filled in.
      await wait(300);
      openFeedback({ kind: "idea", text: "Let me pin a chat" });
      await wait(300);
      seen.draft = dialog()?.querySelector("textarea")?.value;
      if (!stopped) setResult(JSON.stringify(seen));
    })();
    return () => {
      stopped = true;
    };
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider>
        <div className="bg-background text-foreground flex h-screen w-full items-end">
          <aside className="w-64 shrink-0">
            <SidebarFoot />
          </aside>
        </div>
        <FeedbackDialog />
      </SidebarProvider>
      {result && <pre id="feedback-result">{result}</pre>}
      {params.has("look") && <Canon />}
      {looked && <pre id="feedback-look">{looked}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
