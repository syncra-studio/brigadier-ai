import type { FC } from "react";

import { LabeledDivider } from "@/components/shared";
import type { ProviderKind } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { VENDOR_LABELS } from "@/lib/routing";

/**
 * Where a usage limit the turn waited for reset and its work went on (#128): a hairline across
 * the column with "Claude usage limit reset · 9:39 PM" in the middle. It takes the place of the
 * "time passed" line there.
 */
export const QuotaResumed: FC<{ provider: ProviderKind; atMs: number | undefined }> = ({ provider, atMs }) => (
  <LabeledDivider data-slot="quota-resumed">
    {`${VENDOR_LABELS[provider]} usage limit reset${atMs === undefined ? "" : ` · ${formatTime(atMs)}`}`}
  </LabeledDivider>
);
