import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { Button } from "@/components/controls";
import { Folder } from "@/components/icons";
import { Banner, CopyButton, SettingsCard } from "@/components/shared";
import type { AgentItemState, PluginItemView, PluginProjectView, PluginsView, ProviderKind } from "@/ipc/generated";
import { shortPath } from "@/lib/format";
import { agentsIn, isOn, KIND_LABEL, listNames, projectName, shownDetail, spreadOf, wontStartReason, type Tone } from "./model";
import { AgentRow, FoldCard, ItemMark, StateBadge, stateOf } from "./parts";

/** What the details page can ask of the list page that holds it. */
type DetailsActions = {
  busy: boolean;
  onBack: () => void;
  onSwitch: (projectId: string | null, on: boolean) => void;
  /** One agent's switch, where the page looks. */
  onSwitchAgent: (agent: ProviderKind, on: boolean) => void;
  /** Absent while the agents can't be signed in from here. */
  onSignIn?: (() => void) | undefined;
  onUseEverywhere: () => void;
  /** A Library item: gives it to the agents missing it. */
  onAddMissing: () => void;
  onRemove: () => void;
  onRetry: () => void;
  onUpdate: () => void;
  onTrust: (projectId: string) => void;
};

/** D1–D3: one item, how each agent stands with it, the projects it is on in, and Remove. */
export function DetailsPage({ view, item, focus, actions }: { view: PluginsView; item: PluginItemView; focus: PluginProjectView | null; actions: DetailsActions }) {
  const everywhere = item.everywhere;
  const only = !everywhere && item.onIn.length > 0 ? listNames(item.onIn.map((path) => projectName(view, path))) : null;
  return (
    <SettingsPage
      title={item.name}
      description={`${KIND_LABEL[item.kind]} · ${shownDetail(item.detail)}`}
      mark={<ItemMark item={item} large />}
      back={{ label: "Plugins", onBack: actions.onBack }}
      sectionsClassName="gap-9 pt-4"
    >
      {(item.attention || item.updateAvailable) && (
        <div className="flex flex-col gap-2">
          <AttentionBanner view={view} item={item} actions={actions} />
          {item.updateAvailable && (
            <Banner tone="info" text={`A newer version of ${item.name} is ready.`} className="rounded-control border-b-0">
              <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onUpdate}>Update</Button>
            </Banner>
          )}
        </div>
      )}
      <SettingsSection title="Agents" description="Each agent gets it through its own setup. New sessions pick up changes." className="[&>div:first-child]:min-h-0">
        <SettingsCard>
          {view.agents.map((agent) => {
            // Looking at every project, an item kept to some projects is "off" for every agent:
            // each one has it, in those projects.
            const raw = stateOf(item, agent.agent);
            const state: AgentItemState = !focus && only && raw.type === "off" ? { type: "ready" } : raw;
            const note = focus?.notes.find((entry) => entry.agent === agent.agent);
            const [line, tone] = agentLine(item, state, only, focus);
            return (
              <AgentRow key={agent.agent} agent={agent} line={note ? note.message : line} tone={note ? "warning" : tone} dim={state.type === "notSupported"} end={<StateBadge state={state} />} className="px-4 py-3">
                {note && focus && (
                  <Button variant="link-blue" size="inline" className="w-fit" disabled={actions.busy} onClick={() => actions.onTrust(focus.id)}>
                    Trust {focus.name}
                  </Button>
                )}
              </AgentRow>
            );
          })}
        </SettingsCard>
      </SettingsSection>
      <SettingsSection
        title="Projects"
        description={everywhere ? "Where it's on. Turn it off in a project to skip it there." : only ? `Where it's on. It's only in ${only} now; turn on Every project to use it everywhere.` : "Where it's on. It's off in every project now."}
        className="[&>div:first-child]:min-h-0"
      >
        <SettingsCard>
          <ProjectSwitch name="Every project" line={everywhere ? "Also turns on in projects you add later." : "Turn on to use it in all your projects."} on={everywhere} busy={actions.busy} onChange={(on) => actions.onSwitch(null, on)} />
          {view.projects.map((project) => (
            <ProjectSwitch key={project.id} name={project.name} line={shortPath(project.path)} on={isOn(item, project)} busy={actions.busy} onChange={(on) => actions.onSwitch(project.id, on)} />
          ))}
        </SettingsCard>
      </SettingsSection>
      <FoldCard title="Advanced" line={advancedLine(item)} setting="Advanced">
        <>
          <InfoRow title={item.kind === "skill" ? "Folder" : item.remote ? "Address" : "Command"} value={item.detail} />
          {item.remote && item.kind === "server" && (
            <div className="flex items-center gap-6 px-4 py-3">
              <span className="flex min-w-0 flex-1 flex-col gap-1">
                <span className="text-label font-medium">Sign-in</span>
                <span className="text-foreground/65 text-xs">{signInLine(view, item)}</span>
              </span>
              {actions.onSignIn && <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onSignIn}>Sign in again</Button>}
            </div>
          )}
          {item.switchable && view.agents.map((agent) => {
            const state = stateOf(item, agent.agent);
            if (state.type === "missing" || state.type === "notSupported") return null;
            const where = focus ? `in ${focus.name}` : "in every project";
            const on = state.type !== "off";
            return (
              <AgentRow key={agent.agent} agent={agent} line={on ? `On ${where}.` : `Off ${where}.`} className="px-4 py-3"
                end={<SettingsSwitch label={`${item.name} for ${agent.label}`} checked={on} disabled={actions.busy} onCheckedChange={(next) => actions.onSwitchAgent(agent.agent, next)} />} />
            );
          })}
          {item.files.map((file) => <InfoRow key={file} title="Saved in" value={shortPath(file)} />)}
        </>
      </FoldCard>
      <div data-slot="remove-row" className="flex items-center gap-4">
        <Button variant="soft-destructive" size="sm" disabled={actions.busy} onClick={actions.onRemove}>Remove {item.name}</Button>
        <span className="text-foreground/50 text-xs">Removes it from every agent and every project.</span>
      </div>
    </SettingsPage>
  );
}

function AttentionBanner({ view, item, actions }: { view: PluginsView; item: PluginItemView; actions: DetailsActions }) {
  const attention = item.attention;
  if (!attention) return null;
  if (attention.type === "wontStart") {
    return (
      <Banner tone="error" text={`${item.name} won't start: ${wontStartReason(attention.message)}. No agent can use it until the setup is fixed.`}>
        <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onRetry}>Try again</Button>
      </Banner>
    );
  }
  if (attention.type === "signIn") {
    const who = listNames(agentsIn(view, item, "needsSignIn"));
    return (
      <Banner tone="warning" text={`${who} ${agentsIn(view, item, "needsSignIn").length === 1 ? "needs" : "need"} you to sign in to ${item.name}. Until then, ${who} sessions can't use it.`}>
        {actions.onSignIn && <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onSignIn}>Sign in</Button>}
      </Banner>
    );
  }
  const spread = spreadOf(item);
  if (!spread) return <Banner tone="info" text={`${attention.message} It came in that agent's own plugin, so the other agents can't add it.`} />;
  if (spread === "addMissing") {
    return (
      <Banner tone="info" text={`${attention.message} Add it to every agent and they all get it.`}>
        <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onAddMissing}>Add to every agent</Button>
      </Banner>
    );
  }
  return (
    <Banner tone="info" text={`${attention.message} Use it everywhere and every agent gets it.`}>
      <Button variant="soft" size="sm" disabled={actions.busy} onClick={actions.onUseEverywhere}>Use everywhere</Button>
    </Banner>
  );
}

/** An agent's line on the details page, and its colour. */
function agentLine(item: PluginItemView, state: AgentItemState, only: string | null, focus: PluginProjectView | null): [string, Tone] {
  switch (state.type) {
    case "ready": return [item.remote && item.kind === "server" ? "Signed in." : only ? `Ready in ${only}.` : "Ready.", "muted"];
    case "needsSignIn": return ["Waiting for you to sign in.", "warning"];
    case "wontStart": return ["Couldn't start it.", "error"];
    case "off": return [focus ? `Off in ${focus.name}.` : "Off in every project.", "muted"];
    case "missing": return ["Doesn't have it yet.", "muted"];
    case "notSupported": return [state.reason, "muted"];
    case "working": return ["Working on it…", "muted"];
  }
}

/** What Advanced holds, in a line. */
function advancedLine(item: PluginItemView): string {
  const one = item.switchable ? ", turn off for one agent" : "";
  if (item.kind === "skill") return `Folder${one}, where it's saved.`;
  return item.remote ? `Address, sign-in details${one}, where it's saved.` : `Command${one}, where it's saved.`;
}

function signInLine(view: PluginsView, item: PluginItemView): string {
  const signed = agentsIn(view, item, "ready");
  const waiting = agentsIn(view, item, "needsSignIn");
  if (waiting.length) return `${listNames(waiting)} still ${waiting.length === 1 ? "needs" : "need"} to sign in.`;
  return signed.length ? `Each agent keeps its own sign-in: ${listNames(signed)}.` : "No agent has signed in yet.";
}

function ProjectSwitch({ name, line, on, busy, onChange }: { name: string; line: string; on: boolean; busy: boolean; onChange: (on: boolean) => void }) {
  return (
    <div data-slot="project-switch" className="flex items-center gap-3 px-4 py-3">
      <Folder aria-hidden className="text-foreground/65 size-icon-md shrink-0" />
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-label font-medium">{name}</span>
        <span className="text-foreground/65 truncate text-xs">{line}</span>
      </span>
      <SettingsSwitch label={name} checked={on} disabled={busy} onCheckedChange={onChange} />
    </div>
  );
}

function InfoRow({ title, value }: { title: string; value: string }) {
  return (
    <div className="flex items-center gap-6 px-4 py-3">
      <span className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="text-label font-medium">{title}</span>
        <span className="text-foreground/65 font-mono text-xs break-all">{value}</span>
      </span>
      <CopyButton text={value} size="xs" />
    </div>
  );
}
