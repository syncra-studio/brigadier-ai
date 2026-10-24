import { SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { SettingsCard } from "@/components/shared";
import { usedAnywhere, usedIn, type Scope } from "./adapter";
import { changeNote, InfoRow, IntegrationNote, IntegrationSection, listNames, ownFiles, SESSION_NOTE, SkillMark, WhereUsedSection, WorkerRow } from "./parts";
import { switchLabel } from "./ServerPage";
import type { IntegrationProject, IntegrationSkill } from "./types";

/** `scope` is where it's used; `project`, the project whose view the page was opened from. */
export function SkillPage({ skill, skills, scope, projects, project, busy, onEnabled, onInProject, onEverywhere, onBack, onShowPath }: {
  skill: IntegrationSkill; skills: IntegrationSkill[]; scope: Scope; projects: IntegrationProject[]; project?: string | undefined; busy?: boolean;
  onEnabled: (enabled: boolean) => void; onInProject: (projectId: string, enabled: boolean | null) => void; onEverywhere: (on: boolean) => void; onBack: () => void; onShowPath: (path: string) => void;
}) {
  const always = !skill.canSwitchOff && skill.nativeAgents.includes("claude");
  const duplicate = skills.find((other) => other.name === skill.name && other.id !== skill.id);
  const locations: NonNullable<IntegrationSkill["locationDetails"]> = skill.locationDetails ?? skill.locations.map((path) => ({ path, agents: skill.nativeAgents }));
  const { origin } = scope;
  // Whether workers get it: in the project the page is shown for, else in any project.
  const shownFor = project ?? (scope.stays ? origin?.id : undefined);
  const used = shownFor ? usedIn(scope, shownFor) : usedAnywhere(scope);
  // Where workers get it, when that's only some projects: its own, or the ones turning it on.
  const only = scope.stays ? origin?.name : !project && !scope.on && scope.exceptions.length ? listNames(scope.exceptions.map((item) => item.name)) : undefined;
  return <SettingsPage title={skill.name} description={skill.description} mark={<SkillMark id={skill.id} large />} back={{ label: "Plugins", onBack }} sectionsClassName="gap-9 pt-4" actions={<label className="text-foreground/65 flex items-center gap-2.5 text-label">{always ? "Codex workers" : switchLabel(scope, scope.on)}<SettingsSwitch label={always ? `${skill.name}: Codex workers` : skill.name} checked={scope.on} disabled={!!busy || (!skill.canSwitchOff && !always)} onCheckedChange={onEnabled} /></label>}>
    {always && <IntegrationNote>Claude can't hide a skill in ~/.claude/skills for one session, so Claude workers always have this one. The switch decides whether Brigadier gives it to Codex workers.</IntegrationNote>}
    {duplicate && <IntegrationNote>Another skill is called {skill.name} too{duplicate.project ? `, in ${duplicate.project.split("/").pop()}` : ""}. They're two separate skills, each with its own switch{skill.claudeOffByName ? ", but Claude Code switches skills off by name: switching this one off hides both from Claude workers" : ""}.</IntegrationNote>}
    {skill.canSwitchOff && <WhereUsedSection name={skill.name} scope={scope} file={ownFiles(skill)} projects={projects} busy={!!busy} onInProject={onInProject} onEverywhere={onEverywhere} />}
    <IntegrationSection title="Folders" aside={locations.length > 1 ? `One skill, in ${locations.length} folders` : undefined}><SettingsCard>{locations.map((location) => <InfoRow key={location.path} title={location.path} titleMono value={`${location.plugin ? `From the ${location.plugin} plugin · ` : ""}${location.linkedTo ? `Linked to ${location.linkedTo} · ` : location.path.includes("/.agents/") ? "Shared by several agents · " : ""}${location.agents.length ? `${location.agents.map((agent) => agent === "claude" ? "Claude" : "Codex").join(" and ")} reads this folder` : "Brigadier supplies this folder when enabled"}`} mono={false} show="folder" onShow={() => onShowPath(location.path)} />)}</SettingsCard></IntegrationSection>
    <IntegrationSection title="Workers" description={always || only ? undefined : "Workers get this skill where you've enabled it. It costs a worker almost nothing until it uses it."}><SettingsCard>{(["claude", "codex"] as const).map((agent) => {
      const native = skill.nativeAgents.includes(agent);
      const locked = agent === "claude" && always;
      const name = agent === "claude" ? "Claude" : "Codex";
      const location = locations.find((folder) => folder.agents.includes(agent))?.path ?? locations[0]?.path ?? "";
      const parent = location.slice(0, location.lastIndexOf("/"));
      const where = only ? `In ${only}: ` : "";
      const line = locked ? `Find it in ${parent}, and Claude can't hide it for one session` : !used ? `Off: Brigadier doesn't give it to ${name} workers` : native ? `${where}${where ? "find" : "Find"} it in ${parent}` : `${where}${name} doesn't read ${parent}, so Brigadier gives it to them`;
      return <WorkerRow key={agent} agent={agent} line={line} status={!used && !locked ? "Off" : native ? "Finds it" : "Given by Brigadier"} tone={used && native ? "success" : "secondary"} always={locked} dim={!used && !locked} />;
    })}</SettingsCard></IntegrationSection>
    <p className="text-foreground/50 text-xs leading-integration">{always ? SESSION_NOTE : changeNote("skill")}</p>
  </SettingsPage>;
}
