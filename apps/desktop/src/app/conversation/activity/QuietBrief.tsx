import type { FC, ReactNode } from "react";

import { ToolBox } from "@/components/conversation";

/** What a worker was told or why it stopped, in a quiet box. */
export const QuietBrief: FC<{ children: ReactNode }> = ({ children }) => <ToolBox prose>{children}</ToolBox>;
