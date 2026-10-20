import type { ReactNode } from "react";

const svg = (children: ReactNode) => (
  <svg aria-hidden viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" className="page-block-icon size-icon-sm shrink-0">{children}</svg>
);
const letters = (text: string) => svg(<text x="8" y="11.5" textAnchor="middle" fill="currentColor" stroke="none" fontSize="8.5" fontWeight="600">{text}</text>);

/** Each page block kind's icon (Pen: BlockIcon/<the block's label>), 16-unit stroked line icons. */
export const BLOCK_ICONS = {
  paragraph: letters("Aa"),
  h1: letters("H1"),
  h2: letters("H2"),
  h3: letters("H3"),
  bullet: svg(<><circle cx="3" cy="4.5" r=".9" fill="currentColor" /><circle cx="3" cy="11.5" r=".9" fill="currentColor" /><path d="M6.5 4.5h7M6.5 11.5h7" /></>),
  number: svg(<><path d="M2.5 3.2l1-.7v4M2.3 13.5h2.2l-2.2-2.4c.6-1 2.2-.8 2.2.3" /><path d="M7 4.5h6.5M7 11.5h6.5" /></>),
  check: svg(<><path d="M4.5 2.5h6A2.5 2.5 0 0 1 13 5v6a2.5 2.5 0 0 1-2.5 2.5h-6A2.5 2.5 0 0 1 2 11V5a2.5 2.5 0 0 1 2.5-2.5z" /><path d="M5 8l2 2 3.5-4" /></>),
  quote: svg(<><path d="M3 3.5v9" /><path d="M6.5 5h7M6.5 8h7M6.5 11h4.5" /></>),
  code: svg(<path d="M5.5 4.5L2 8l3.5 3.5M10.5 4.5L14 8l-3.5 3.5" />),
  table: svg(<><path d="M4 2.5h8a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z" /><path d="M2 6.5h12M6.5 6.5v7" /></>),
  divider: svg(<path d="M2 8h12" />),
} as const;
