import { type LexicalEditor, BLOCKS, applyBlock, BlockRow } from "@/components/views/page-editor";

import { DropdownMenuContent, DropdownMenuItem } from "@/components/overlays";

/** The page's Insert menu: every block but a paragraph, put in at the caret. */
export function PageInsertMenu({ editor }: { editor: () => LexicalEditor | null }) {
  return (
    <DropdownMenuContent align="start" className="page-menu page-insert-list" aria-label="Insert options"
      onCloseAutoFocus={(event) => { const current = editor(); if (current) { event.preventDefault(); current.focus(); } }}>
      <div className="page-menu-label">Basic</div>
      {BLOCKS.filter((block) => block.kind !== "paragraph").map((block) => (
        <DropdownMenuItem key={block.kind} className="page-menu-item" onSelect={() => { const current = editor(); if (current) applyBlock(current, block.kind); }}>
          <BlockRow block={block} />
        </DropdownMenuItem>
      ))}
    </DropdownMenuContent>
  );
}
