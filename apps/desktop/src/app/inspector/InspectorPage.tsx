import { useEffect, useRef } from "react";

import { BrainTab } from "@/app/inspector/brain/BrainTab";
import { EventsTab } from "@/app/inspector/EventsTab";
import { OrchestratorTab } from "@/app/inspector/OrchestratorTab";
import { PerformanceTab } from "@/app/inspector/PerformanceTab";
import { ProcessesTab } from "@/app/inspector/ProcessesTab";
import { ProvidersTab } from "@/app/inspector/providers/ProvidersTab";
import { RoutingTab } from "@/app/inspector/RoutingTab";
import { SettingsPage } from "@/app/settings/parts";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { setInspectorTab } from "@/state/actions";
import { type InspectorTab, useApp } from "@/state/store";

function isTab(value: string): value is InspectorTab {
  return (
    value === "events" ||
    value === "orchestrator" ||
    value === "brain" ||
    value === "processes" ||
    value === "performance" ||
    value === "providers" ||
    value === "routing"
  );
}

/**
 * The Inspector, a page of Settings for developers: the live event stream, the open session's
 * orchestrator context, the Project Brains, processes, metrics against the §4 budgets, raw
 * provider sessions, and a routing preview, a tab each. The tabs fill the page's column down
 * to the window's bottom, each scrolling on its own.
 */
export function InspectorPage() {
  const tab = useApp((s) => s.inspector.tab);
  const tabs = useRef<HTMLDivElement>(null);
  // A narrow window cannot fit every tab; the strip scrolls and keeps the open one in view.
  useEffect(() => {
    tabs.current
      ?.querySelector(`[role="tab"][id$="-trigger-${tab}"]`)
      ?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [tab]);
  return (
    <SettingsPage title="Inspector" wide scrollable={false}>
      <Tabs
        value={tab}
        onValueChange={(value) => isTab(value) && setInspectorTab(value)}
        className="flex min-h-0 flex-1 flex-col gap-0"
      >
        <div className="flex shrink-0 flex-col pt-1 pb-3">
          <TabsList
            ref={tabs}
            className="hide-scrollbar max-w-full min-w-0 justify-start self-start overflow-x-auto"
          >
            <TabsTrigger value="events">Events</TabsTrigger>
            <TabsTrigger value="orchestrator">Orchestrator</TabsTrigger>
            <TabsTrigger value="brain">Brain</TabsTrigger>
            <TabsTrigger value="processes">Processes</TabsTrigger>
            <TabsTrigger value="performance">Performance</TabsTrigger>
            <TabsTrigger value="providers">Providers</TabsTrigger>
            <TabsTrigger value="routing">Routing</TabsTrigger>
          </TabsList>
        </div>
        <TabsContent value="events" className="flex min-h-0 flex-col">
          <EventsTab />
        </TabsContent>
        <TabsContent value="orchestrator" className="flex min-h-0 flex-col">
          <OrchestratorTab />
        </TabsContent>
        <TabsContent value="brain" className="flex min-h-0 flex-col">
          <BrainTab />
        </TabsContent>
        <TabsContent value="processes" className="min-h-0 overflow-y-auto">
          <ProcessesTab />
        </TabsContent>
        <TabsContent value="performance" className="min-h-0 overflow-y-auto">
          <PerformanceTab />
        </TabsContent>
        <TabsContent value="providers" className="flex min-h-0 flex-col">
          <ProvidersTab />
        </TabsContent>
        <TabsContent value="routing" className="flex min-h-0 flex-col">
          <RoutingTab />
        </TabsContent>
      </Tabs>
    </SettingsPage>
  );
}
