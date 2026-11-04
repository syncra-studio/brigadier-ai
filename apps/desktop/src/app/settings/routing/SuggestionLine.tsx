import { useAction } from "@/app/conversation/useAction";
import { ratingText, rowModelName, type WorkRow, withDismissed, withFirst, withOrder, workName } from "@/app/settings/routing/words";
import { Button } from "@/components/controls";
import { Upgrade } from "@/components/icons";
import type { ModelGroup } from "@/components/shared";
import type { ModelRules, RoutingSuggestion } from "@/ipc/generated";
import { loadModelRules, saveModelRules } from "@/state/modelRules";
import { editSettings } from "@/state/settings";

/**
 * A better-rated model for a kind of work the user ordered: "GPT-6.1 Terra now rates higher for
 * Computer use". Use it puts it first in the order (the only thing that changes the order);
 * Dismiss hides it until that model's rating changes again.
 */
export function SuggestionLine({
  suggestion,
  row,
  rules,
  groups,
}: {
  suggestion: RoutingSuggestion;
  row: WorkRow;
  rules: ModelRules;
  groups: readonly ModelGroup[];
}) {
  const action = useAction();
  const name = rowModelName(groups, suggestion.row.agent, suggestion.row.model);
  const current = rowModelName(groups, row.mine[0]?.agent ?? suggestion.row.agent, suggestion.current);
  const use = () =>
    action.run(() => saveModelRules(rules, withOrder(rules.rows, row.work, withFirst(row.mine, suggestion.row)), rules.rules));
  const dismiss = () =>
    action.run(async () => {
      await editSettings((settings) => ({ ...settings, dismissedSuggestions: withDismissed(settings.dismissedSuggestions, suggestion) }));
      await loadModelRules();
    });
  return (
    <div data-slot="routing-suggestion" className="flex flex-col ps-7.5 pb-3">
      <div className="flex items-center gap-3">
        <Upgrade aria-hidden className="text-link size-icon-sm shrink-0" />
        <span className="text-xs text-foreground min-w-0 flex-1 truncate">
          {name} now rates higher for {workName(row.work)}
          <span className="text-muted-foreground">
            {" "}
            · {ratingText(suggestion.score)} against {current}'s {ratingText(suggestion.currentScore)}
          </span>
        </span>
        <Button variant="link-blue" size="inline" className="text-xs font-medium" disabled={action.busy} onClick={use}>
          Use it
        </Button>
        <Button variant="link-blue" size="inline" className="text-xs text-muted-foreground" disabled={action.busy} onClick={dismiss}>
          Dismiss
        </Button>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive text-xs pt-1">
          {action.error}
        </p>
      )}
    </div>
  );
}
