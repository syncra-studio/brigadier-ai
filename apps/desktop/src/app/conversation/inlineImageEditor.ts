import type { Unstable_DirectiveFormatter } from "@assistant-ui/react";
import { $createDirectiveNodeWithFormatter, DirectiveNode } from "@assistant-ui/react-lexical";
import { mergeRegister } from "@lexical/utils";
import {
  $createTextNode,
  $nodesOfType,
  HISTORY_MERGE_TAG,
  type LexicalEditor,
  TextNode,
} from "lexical";

import {
  IMAGE_UPLOAD,
  INLINE_IMAGE,
  imageToken,
  uploadToken,
  type InlineImages,
} from "@/lib/inlineImages";

/** Transforms restored history and clipboard nodes as well as the live upload placeholder. */
export function registerInlineImages(
  editor: LexicalEditor,
  images: InlineImages,
  formatter: Unstable_DirectiveFormatter,
): () => void {
  return mergeRegister(
    editor.registerNodeTransform(DirectiveNode, (node) => {
      const item = node.getDirectiveItem();
      if (item.type === IMAGE_UPLOAD) {
        const upload = images.uploads.get(item.id);
        if (!upload) {
          node.replace($createTextNode(uploadToken(item.id)));
        } else if (upload.status === "ready") {
          node.replace($createDirectiveNodeWithFormatter(
            { id: upload.ref.id, type: INLINE_IMAGE, label: upload.ref.name }, formatter,
          ));
        } else if (node.getDirectiveText() !== uploadToken(item.id)) {
          node.replace($createDirectiveNodeWithFormatter(item, formatter));
        }
      } else if (item.type === INLINE_IMAGE) {
        if (!images.refs.has(item.id)) node.replace($createTextNode(imageToken(item.id)));
        else if (node.getDirectiveText() !== imageToken(item.id)) {
          node.replace($createDirectiveNodeWithFormatter(item, formatter));
        }
      }
    }),
    editor.registerNodeTransform(TextNode, (node) => {
      if (!node.isSimpleText() || editor.isComposing()) return;
      let at = 0;
      for (const segment of formatter.parse(node.getTextContent())) {
        if (segment.kind === "text") {
          at += segment.text.length;
          continue;
        }
        const length = formatter.serialize(segment).length;
        if (segment.type === INLINE_IMAGE || segment.type === IMAGE_UPLOAD) {
          const parts = node.splitText(at, at + length);
          (at === 0 ? parts[0] : parts[1])?.replace(
            $createDirectiveNodeWithFormatter(segment, formatter),
          );
          break;
        }
        at += length;
      }
    }),
    editor.registerUpdateListener(({ editorState }) => {
      const keys = editorState.read(() => $nodesOfType(DirectiveNode)
        .filter((node) => node.isAttached() && node.getDirectiveItem().type === IMAGE_UPLOAD)
        .map((node) => node.getDirectiveItem().id));
      images.sync(keys);
    }),
    images.subscribe(() => editor.update(() => {
      for (const node of $nodesOfType(DirectiveNode)) {
        if (node.getDirectiveItem().type === IMAGE_UPLOAD) node.markDirty();
      }
    }, { tag: HISTORY_MERGE_TAG })),
  );
}
