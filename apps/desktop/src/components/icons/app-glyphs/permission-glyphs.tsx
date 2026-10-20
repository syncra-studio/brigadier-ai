import type { FC, SVGProps } from "react";

/**
 * The permission icons that the icon set lacks: a shield (the outline of
 * apps-sdk-ui's ShieldCheck, MIT) with a prompt inside for Approve for me, and with an
 * exclamation mark for Full access. Drawn like the set's icons (24-unit grid, current
 * colour), sized by the caller.
 */

const SHIELD =
  "M13.203 1.935a3 3 0 0 0-2.405 0l-6 2.625A3 3 0 0 0 3 7.308V13a9 9 0 1 0 18 0V7.308a3 3 0 0 0-1.797-2.748l-6-2.625Zm-1.604 1.832a1 1 0 0 1 .802 0l6 2.625a1 1 0 0 1 .599.916V13a7 7 0 1 1-14 0V7.308a1 1 0 0 1 .6-.916l6-2.625Z";

type GlyphProps = SVGProps<SVGSVGElement>;

const Shield: FC<GlyphProps & { mark: string }> = ({ mark, ...props }) => (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden {...props}>
    <path fillRule="evenodd" clipRule="evenodd" d={SHIELD} />
    <path d={mark} />
  </svg>
);

/** A shield with a prompt (›_): workers act for you, inside the sandbox. */
export const ShieldTerminal: FC<GlyphProps> = (props) => (
  <Shield
    mark="M7.793 9.793a1 1 0 0 1 1.414 0l2 2a1 1 0 0 1 0 1.414l-2 2a1 1 0 1 1-1.414-1.414L9.086 12.5l-1.293-1.293a1 1 0 0 1 0-1.414ZM12 15a1 1 0 0 1 1-1h3a1 1 0 1 1 0 2h-3a1 1 0 0 1-1-1Z"
    {...props}
  />
);

/** A shield with an exclamation mark: Full access, no sandbox. */
export const ShieldExclamation: FC<GlyphProps> = (props) => (
  <Shield
    mark="M12 7.5a1 1 0 0 1 1 1V13a1 1 0 1 1-2 0V8.5a1 1 0 0 1 1-1ZM12 14.75a1.25 1.25 0 1 1 0 2.5a1.25 1.25 0 1 1 0-2.5Z"
    {...props}
  />
);
