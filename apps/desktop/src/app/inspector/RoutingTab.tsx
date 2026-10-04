import { ChevronRight, Clock, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { lazy, Suspense, useEffect, useRef, useState } from "react";

import { ExplanationView } from "@/app/conversation/cards/RouteDetails";
import { useAction } from "@/app/conversation/useAction";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { effortLabel } from "@/components/assistant-ui/elements/model-selector";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { useNow } from "@/hooks/use-now";
import { request } from "@/ipc/client";
import type { Area, RoutePreview } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { AREA_LABELS, AREAS, CATEGORY_LABELS, choiceName, formatResetAt } from "@/lib/routing";
import { useModelGroups } from "@/lib/setup";
import { selectedConversation, useApp } from "@/state/store";

// Development builds only: not part of a release bundle.
const FaultControl = import.meta.env.DEV
  ? lazy(() => import("@/app/inspector/FaultControl").then((module) => ({ default: module.FaultControl })))
  : null;

/**
 * What routing would choose for each kind of work right now, with the explanation behind it or
 * what it would wait for (nothing starts); in development builds, a control that makes a
 * provider hit a limit for one task or conversation.
 */
export function RoutingTab() {
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <PreviewSection />
      {FaultControl && (
        <Suspense fallback={null}>
          <FaultControl />
        </Suspense>
      )}
    </div>
  );
}

function PreviewSection() {
  const groups = useModelGroups();
  const projects = useApp((s) => s.projects);
  // What routing depends on besides the question: the providers' state and the user's rules.
  const providers = useApp((s) => s.providers.view?.providers);
  const rules = useApp((s) => s.settings.routingOverrides);
  const rankingsRevision = useApp((s) => s.rankingsRevision);
  // The open conversation's project, to begin with.
  const [projectId, setProjectId] = useState<string | null>(
    () => selectedConversation(useApp.getState())?.projectId ?? null,
  );
  const [areas, setAreas] = useState<Area[]>([]);
  const [routes, setRoutes] = useState<RoutePreview[] | null>(null);
  const preview = useAction();
  // Only the latest question's answer is shown; an older one arriving late is dropped.
  const asked = useRef(0);
  const run = () => {
    const seq = ++asked.current;
    setRoutes(null);
    preview.run(async () => {
      try {
        const { routes: next } = await request({ method: "previewRoutes", projectId, areas });
        if (seq === asked.current) setRoutes(next);
      } catch (error) {
        if (seq === asked.current) throw error;
      }
    });
  };
  // Read again whenever the question, a provider's state, a rule or the models' ratings change.
  // oxlint-disable-next-line react-hooks/exhaustive-deps
  useEffect(run, [projectId, areas, providers, rules, rankingsRevision]);
  const list = Object.values(projects).toSorted((a, b) => a.name.localeCompare(b.name));

  return (
    <section className="flex flex-col gap-2 border-b px-3 py-3">
      <div className="flex items-center gap-2">
        <h3 className="flex-1 text-xs font-medium">Routing preview</h3>
        <Button size="xs" variant="ghost" disabled={preview.busy} onClick={run}>
          <Reload />
          Again
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">
        What each kind of task would run on now, under the current quota and your rules. Nothing
        starts.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="xs" variant="outline">
              <span className="max-w-2xs truncate">
                {projectId ? (projects[projectId]?.name ?? "Project") : "No project"}
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuRadioGroup
              value={projectId ?? ""}
              onValueChange={(value) => setProjectId(value === "" ? null : value)}
            >
              <DropdownMenuRadioItem value="">No project</DropdownMenuRadioItem>
              {list.length > 0 && <DropdownMenuSeparator />}
              {list.map((project) => (
                <DropdownMenuRadioItem key={project.id} value={project.id}>
                  {project.name}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
        <ToggleGroup
          type="multiple"
          size="sm"
          variant="outline"
          spacing="tight"
          aria-label="Areas the task touches"
          className="flex-wrap"
          value={areas}
          onValueChange={(value) => setAreas(value as Area[])}
        >
          {AREAS.map((area) => (
            <ToggleGroupItem key={area} value={area} className="text-xs">
              {AREA_LABELS[area]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      {preview.error && <p className="text-destructive text-xs">{preview.error}</p>}
      {routes && (
        <ul className="flex flex-col divide-y">
          {routes.map((route) => (
            <PreviewRow key={route.category} route={route} groups={groups} />
          ))}
        </ul>
      )}
    </section>
  );
}

function PreviewRow({ route, groups }: { route: RoutePreview; groups: readonly ModelGroup[] }) {
  const now = useNow(30_000);
  const { outcome } = route;
  const category = CATEGORY_LABELS[route.category];
  return (
    <li data-slot="route-preview" className="flex flex-col gap-1 py-2 text-xs">
      <div className="flex items-baseline gap-2">
        <span className="w-28 shrink-0 truncate capitalize">{category}</span>
        {outcome.type === "chosen" ? (
          <span className="min-w-0 flex-1 truncate font-medium">
            {choiceName(groups, outcome.choice)}
            {outcome.choice.effort && ` · ${effortLabel(outcome.choice.effort)}`}
          </span>
        ) : (
          <span className="text-warning flex min-w-0 flex-1 items-center gap-1">
            <Clock aria-hidden className="size-icon-xs shrink-0" />
            Waits
            {outcome.resetsAtMs !== null &&
              ` until ${formatResetAt(outcome.resetsAtMs, now)} (${formatCountdown(outcome.resetsAtMs, now)})`}
          </span>
        )}
      </div>
      <p className="text-muted-foreground ms-30">{outcome.reason}</p>
      {outcome.type === "wait" && outcome.rule && (
        <p className="text-muted-foreground ms-30">Your rule: {outcome.rule}</p>
      )}
      {outcome.type === "wait" && outcome.ranking && (
        <p className="text-muted-foreground ms-30">Kept for the models in {outcome.ranking}</p>
      )}
      {outcome.type === "chosen" && outcome.explanation && (
        <Collapsible className="ms-30">
          <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex items-center gap-1">
            <ChevronRight
              aria-hidden
              className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
            />
            Explanation
          </CollapsibleTrigger>
          <CollapsibleContent className="mt-1.5">
            <ExplanationView explanation={outcome.explanation} groups={groups} />
          </CollapsibleContent>
        </Collapsible>
      )}
    </li>
  );
}
