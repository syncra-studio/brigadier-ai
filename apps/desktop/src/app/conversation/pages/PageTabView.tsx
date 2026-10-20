import type { LexicalEditor } from "lexical";
import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { Banner } from "@/components/shared/banner";
import { Button } from "@/components/controls/button";
import { openLink } from "@/state/links";
import { pageEditing, usePageEdits } from "@/state/pageEditing";
import { useApp } from "@/state/store";
import { ensurePage } from "@/state/pageTabs";
import { changeSessionTab, closeTab, openPageTab, type PageTabState } from "@/state/sessionTabs";

import { EmojiButton } from "./EmojiButton";
import { focusPageStart, PageEditor } from "../../../components/views/page-editor/PageEditor";
import { PageTopBar } from "./PageTopBar";
import "./PageTab.css";

/** A page in a session tab: its title, its emoji and its body, saved as they are typed. */
export function PageTabView({ conversationId, tab, active }: { conversationId: string; tab: PageTabState; active: boolean }) {
  const [attempt, setAttempt] = useState(0);
  if (!tab.pageId) return <MakePage key={attempt} tab={tab} onRetry={() => setAttempt((count) => count + 1)} />;
  return <OpenPage conversationId={conversationId} tab={tab} pageId={tab.pageId} active={active} />;
}

/** Waits for the daemon to make the tab's page. */
function MakePage({ tab, onRetry }: { tab: PageTabState; onRetry: () => void }) {
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    ensurePage(tab).catch((error: unknown) => { if (live) setFailure(error instanceof Error ? error.message : String(error)); });
    return () => { live = false; };
  }, [tab]);
  return failure ? (
    <div className="page-tab">
      <Banner tone="error" text={`Could not make the page: ${failure}`}>
        <Button size="sm" variant="ghost" onClick={onRetry}>Try again</Button>
      </Banner>
    </div>
  ) : <div className="page-tab" aria-busy />;
}

function OpenPage({ conversationId, tab, pageId, active }: { conversationId: string; tab: PageTabState; pageId: string; active: boolean }) {
  const editing = pageEditing();
  const mac = useApp((s) => s.info?.platform === "macos");
  const page = usePageEdits((s) => s.pages[pageId]);
  const editor = useRef<LexicalEditor | null>(null);
  const title = useRef<HTMLTextAreaElement | null>(null);
  useEffect(() => {
    void editing.open(pageId);
    return () => { void editing.close(pageId); };
  }, [editing, pageId]);
  // The tab shows the page's title, also after a restart before the page loads.
  const shownTitle = page?.status === "ready" ? page.content.title : null;
  useEffect(() => {
    if (shownTitle === null) return;
    changeSessionTab(conversationId, tab.id, (current) =>
      current.kind === "page" && current.title !== shownTitle ? { ...current, title: shownTitle } : current);
  }, [conversationId, tab.id, shownTitle]);
  // A new page starts in its title.
  const fresh = page?.status === "ready" && !page.content.title && !page.content.body;
  useEffect(() => {
    if (active && fresh) title.current?.focus();
    // Only when the page first shows.
    // oxlint-disable-next-line react-hooks/exhaustive-deps
  }, [active, page?.status]);
  useLayoutEffect(() => {
    const field = title.current;
    if (!field) return;
    field.style.height = "auto";
    field.style.height = `${field.scrollHeight}px`;
  }, [page?.content.title]);

  if (!page || page.status === "loading") return <div className="page-tab" aria-busy />;
  if (page.status === "failed") {
    return (
      <div className="page-tab">
        <Banner tone="error" text={`Could not open the page: ${page.error ?? "unknown error"}`}>
          <Button size="sm" variant="ghost" onClick={() => void editing.open(pageId, false)}>Try again</Button>
        </Banner>
      </div>
    );
  }
  const change = (content: Partial<typeof page.content>) => {
    const now = usePageEdits.getState().pages[pageId];
    if (now) editing.edit(pageId, { ...now.content, ...content });
  };
  const keepBoth = async () => {
    const copy = await editing.keepBoth(pageId);
    openPageTab(conversationId, copy, page.content.title ? `${page.content.title} (copy)` : "");
  };
  return (
    <div className="page-tab" data-slot="page-tab">
      <PageTopBar conversationId={conversationId} pageId={pageId} content={page.content} editor={() => editor.current} mac={mac} active={active}
        onRename={() => { const field = title.current; if (!field) return; field.focus(); field.select(); }} />
      {page.conflict && (
        <Banner tone="warning" text="This page changed elsewhere. Which version do you want to keep?">
          <Button size="sm" variant="ghost" onClick={() => void editing.keepMine(pageId)}>Keep mine</Button>
          <Button size="sm" variant="ghost" onClick={() => editing.takeTheirs(pageId)}>Use theirs</Button>
          <Button size="sm" variant="ghost" onClick={() => void keepBoth().catch(console.error)}>Keep both</Button>
        </Banner>
      )}
      {page.status === "missing" && (page.dirty ? (
        <Banner tone="warning" text="This page was deleted elsewhere. Your text is still here.">
          <Button size="sm" variant="ghost" onClick={() => void keepBoth().catch(console.error)}>Keep as a new page</Button>
        </Banner>
      ) : (
        <Banner tone="warning" text="This page was deleted.">
          <Button size="sm" variant="ghost" onClick={() => closeTab(conversationId, tab.id)}>Close tab</Button>
        </Banner>
      ))}
      {page.error && page.status === "ready" && !page.conflict && (
        <Banner tone="error" text={`Your last changes aren't saved yet: ${page.error}. They're kept here and saved with your next edit.`}>
          <Button size="sm" variant="ghost" onClick={() => void editing.flush(pageId)}>Try again</Button>
        </Banner>
      )}
      <div className="page-scroll">
        <div className="page-column">
          <div className="page-heading">
            <EmojiButton emoji={page.content.emoji} onChange={(emoji) => change({ emoji })} />
            <textarea
              ref={title}
              aria-label="Page title"
              placeholder="Untitled page"
              rows={1}
              spellCheck
              className="page-title"
              value={page.content.title}
              onChange={(event) => change({ title: event.target.value.replaceAll("\n", " ") })}
              onKeyDown={(event) => {
                const field = event.currentTarget;
                const atEnd = field.selectionStart === field.value.length;
                if (event.key === "Enter" || (event.key === "ArrowDown" && atEnd)) {
                  event.preventDefault();
                  if (editor.current) focusPageStart(editor.current);
                }
              }}
            />
          </div>
          <PageEditor
            key={`${pageId}:${page.version}`}
            body={page.content.body}
            label={page.content.title || "Untitled page"}
            editorRef={editor}
            onChange={(body) => change({ body })}
            onOpenLink={(url) => void openLink(url, { conversationId })}
          />
        </div>
      </div>
    </div>
  );
}

