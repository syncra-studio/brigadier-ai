import { SummaryRowButton } from "@/components/conversation";

import { hostOf, openLink } from "@/app/conversation/summaryLinks";
import { Link } from "@/components/icons";

export const SourceRow = ({ url }: { url: string }) => (
  <SummaryRowButton title={url} icon={<Link />} onClick={() => openLink(url)}>
    {hostOf(url)}
  </SummaryRowButton>
);
