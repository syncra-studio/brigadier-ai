import type { FC } from "react";

import { LabeledDivider } from "@/components/shared";

/**
 * A moment of the conversation that is no work of its own (#506): a usage limit, a move to
 * another model, provider or account, the chosen model back. A divider says it where it happened.
 */
export const Moment: FC<{ text: string }> = ({ text }) => (
  <LabeledDivider data-slot="moment" role="status">
    <span className="min-w-0 text-center">{text}</span>
  </LabeledDivider>
);
