import { preloadable } from "@/lib/preloadable";

/**
 * The replies' Markdown view: the markdown stack (remark, micromark, mdast) loads off the cold
 * start, at idle or before a conversation first shows (#500), never with its replies showing.
 */
export const markdownText = preloadable(() =>
  import("@/components/conversation/markdown-text").then((module) => module.MarkdownText),
);
