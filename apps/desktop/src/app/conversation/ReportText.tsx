import { createContext, type FC, useContext } from "react";

import { DetailsFold } from "@/app/conversation/DetailsFold";
import { splitReport } from "@/app/conversation/blocks";
import { MarkdownBlock, MessageText, type TextMessagePartProps } from "@/components/conversation";

/**
 * Whether the finished answer keeps its `### Details`: a run's morning report and a turn that
 * changed code do; an answer that changed nothing (a question, research) doesn't (Q79).
 */
export const AnswerDetails = createContext(true);

/**
 * A finished answer, a run's morning report too: what it came to in view, and a run's report or
 * a turn that changed code keeps its `### Details` (the checks run, review findings, what wasn't
 * tested) folded under "Details", the answer's one fold (#198). Sources are the links in its
 * text, where it uses them (Q87).
 */
export const ReportText: FC<TextMessagePartProps> = (props) => {
  const keepDetails = useContext(AnswerDetails);
  const report = splitReport(props.text);
  const details = keepDetails ? report?.details : undefined;
  if (!report || !details) {
    return report ? <MarkdownBlock text={report.head} plainLeads /> : <MessageText {...props} />;
  }
  return (
    <div className="flex flex-col gap-3">
      <MarkdownBlock text={report.head} plainLeads />
      <DetailsFold>
        <MarkdownBlock text={details} />
      </DetailsFold>
    </div>
  );
};
