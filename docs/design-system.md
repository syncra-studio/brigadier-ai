# Design system

The desktop app's design lives outside the repository, in `~/Documents/brigadier-design/`. It is
taken from the code: the tokens, every component the app renders with all of its states, and the
app's screens as they ship today. Use it to build new UI from the same parts, and keep it in step
when the UI changes.

The files are pen.dev documents (`.pen`). They are encrypted: open and edit them only in Pen or
through its MCP server, never as text.

## The files

| File | What it holds |
|---|---|
| `README.pen` | The Read me board: the file map, how files link, naming, the design rules, how to save. Start here. |
| `foundations.pen` | The design system only: all variables (colours, radii, sizes, type), the `Icon / <Name>` icons, glyphs and indicators, controls (buttons, badges, inputs, switches, the checkbox, tabs…), menu, popover and dialog shells, and the shared app parts that both `app.pen` and `settings.pen` use. |
| `app.pen` | The main window, one board group per surface in app order: App shell, Conversation, Side panel, Setup. Each surface's screens, dialogs and components sit on its own boards. |
| `settings.pen` | The Settings window, including the Inspector (a page inside Settings): boards per page in `pages.ts` order, with every Settings and Inspector screen. |
| `unused.pen` | Components nothing in the app renders today, each with a cleanup note (what to remove in the code, and how sure). Never build from it. |

## Boards

- Every `app.pen` and `settings.pen` board is titled by its surface or page ("Surface · Topic"
  when it has several boards) and holds its zones in this order: ① Screens, ② Dialogs,
  ③ Components. Cards are titled "#NN Name" (the number from the components inventory) and show
  the component's code path.
- Boards sit in rows, not columns, left to right in app order, with a label above each group.
  A board is at most 8,400 × 5,000 and always landscape (the user's two boards in `app.pen` are
  the only exception); when a surface needs more, it gets another board beside it.

## How the files link

Every file except `foundations.pen` imports it under the alias `f`, and nothing else. Imports are
one level deep, so a part both `app.pen` and `settings.pen` use must live in `foundations.pen`.

- Instance of a foundations component: `{ "type": "ref", "ref": "f:<id>" }`.
- Override inside it: every path segment that names a foundations node carries the prefix,
  `"f:<childId>/f:<childId>"`.
- Variable: `"$f:--foreground"`.

## Names

- Components: `Area / Component / Variant`, for example `Button/Primary/sm`, `Icon / Sparkle`,
  `Uninstall/Plan`.
- Screens: `Screen / <Area> / <Name>`, for example `Screen / Settings / Storage › Uninstall`.
- Every state of a component is drawn and captioned under it. Each screen's caption names its
  source file in `apps/desktop/src`.

## Rules for building UI from it

- Use the design-system components. Don't draw a part by hand when one exists; visual details
  (cards, radii, hover, popovers, dialogs) reuse what the app already has.
- Icons are the app's real icons, and the Icons board is their one home in Pen: see Icons below.
  A few spots keep a library icon because the app has none there; the Read me lists them.
- Screens show what ships, not ideas. 1440×900, dark, realistic sample content.
- Dark only. Normal density only: there is no compact variant.
- Buttons with words, chips and badges are fully rounded (`rounded-capsule`). Icon buttons keep
  a 10px corner (`rounded-toolbar-button`), except the spots Pen draws round (the composer's
  controls, the browser's toolbar…), which stay round; the composer's send button is a circle.
- There is one checkbox: blue (`--toggle-on`) with a white tick, corner `--radius-checkbox` (5px).
- Plain words. No names or marks of other chat apps or of the UI kit's vendor.
- In the code every colour, length, radius, font and shadow comes from
  `apps/desktop/src/styles/tokens.css`. The UI kit and Radix are used only inside the wrappers in
  `src/components/controls`, `overlays` and `layout`; icons live only in `src/components/icons`;
  the libraries that draw (the chat library, lexical, xterm, sigma/graphology, @pierre/diffs)
  are imported only inside `src/components`.
- The style check enforces all of it: `pnpm lint` runs the oxlint rule `no-raw-design-values`
  (raw colours, lengths and Tailwind arbitrary values in ts/tsx) and `scripts/check-styles.mjs`
  (CSS files, fixed inline styles, kit, Radix and library imports, `<svg>` outside icons,
  components taken from a folder's index), and fast CI fails on it. A value that can't follow
  yet goes in `scripts/check-styles.allow` with its reason; an entry no longer needed fails too.

## Code layout

`apps/desktop/src/components/` holds every reusable part, one folder per foundations board, each
with one `index.ts`. Screens import a part from its folder's index (`@/components/controls`),
never from a file inside it; the style check fails that in `.tsx` files outside
`src/components`. Type-only imports, `import()` of a view that loads on demand, and `.ts` helpers
that draw nothing may name the file.

| Folder | foundations.pen board | What it holds |
|---|---|---|
| `icons/` | Icons | every icon, one folder per section (see Icons below) |
| `controls/` | Primitives · Controls | button, icon button, badge, input, checkbox, switch, segmented, tabs, toggle, toggle group, key chip |
| `overlays/` | Primitives · Menus, popovers & dialogs | dialog, confirm dialog, popover, dropdown and context menus, tooltip, toast |
| `layout/` | ScrollFade (Menus board), Skeleton (Controls board) | sidebar, sidebar layout, collapsible, scroll fade, skeleton |
| `shared/` | Shared app parts | settings card and search, state dot, usage bar, banner, empty state, titlebar button, copy button, model selector, progress line, labeled divider |
| `conversation/` | (app.pen Conversation boards) | the conversation kit: thread, message rail, markdown, code blocks, composer parts, transcript; `kit.ts` is the one door to the chat library |
| `views/<view>/` | (on their surface's boards) | views built on a drawing library, each its own folder so it loads on demand: `terminal` (xterm), `diff` (@pierre/diffs), `brain-graph` (sigma), `page-editor` (lexical) |

The libraries that draw (the chat library, lexical, xterm, sigma/graphology, @pierre/diffs) are
imported only inside `src/components`; the style check fails them anywhere else.

`apps/desktop/src/app/` holds the screens, one folder per surface as the boards group them:
`shell/` (App shell: window, sidebar, top bar and session tabs, app dialogs), `conversation/`,
`side-panel/`, `setup/` (onboarding, computer-use setup) in `app.pen`; `settings/` (its pages,
Providers and Routing among them) and `inspector/` in `settings.pen`. App-wide wiring (`App.tsx`,
shortcuts, smoke) stays at the top of `app/`.

## Pen component → code

Every component in `foundations.pen`, board by board, with the file (under `apps/desktop/src`)
and the export that draws it. "Pen only" means the code has no such part; "code only" means a
component folder holds a part that has no card yet. Parts still in `src/app` (not yet under
`src/components`) are noted "in app"; the per-surface "Design system: … parts as reusable
components" issues track moving them. The card captions in Pen may still show older paths; this
table is the current one.

| Board | Pen component | Code file | Export | Note |
|---|---|---|---|---|
| Icons | `Icon / <Name>`, `FileIcon/*`, `IdGlyph/*`, `BlockIcon/*`, `BrigadierGlyph`, `ProviderGlyph/*` | `components/icons/` | see Icons below | |
| Icons | `Spinner/12`, `/14`, `/16`, `/20` | `components/icons/app-glyphs/spinner.tsx` | `Spinner` (sized by class) | |
| Primitives · Controls | `Badge/Default`, `Secondary`, `Destructive`, `Warning`, `Success`, `Outline`, `Ghost`, `Link` | `components/controls/badge.tsx` | `Badge` (`variant`) | |
| Primitives · Controls | `Badge/Full access` | — | — | Pen only: the app's Full access pill is the access picker's trigger (`text-full-access`) in `app/shell/SetupPickers.tsx` |
| Primitives · Controls | `Button/Primary`, `Secondary`, `Outline`, `Ghost`, `Destructive`, `Link`, each `/xs` and `/sm` | `components/controls/button.tsx` | `Button` (`variant` default…link, `size` default/xs/sm) | |
| Primitives · Controls | — | `components/controls/button.tsx` | `Button` variants `soft`, `soft-destructive`, `link-blue`, sizes `lg`, `inline` | code only (`soft` is drawn as SettingsButton, Shared app parts) |
| Primitives · Controls | `Icon Button`, `Icon Button/xs`, `/sm`, `/lg` | `components/controls/tooltip-icon-button.tsx` | `TooltipIconButton` (`size`, `look` ghost/toolbar/strip, `pressed`, `dot`) | sizes come from `Button` `icon-*` |
| Primitives · Controls | `Send Button` | `components/controls/send-button.tsx` | `SendButton` (`state`, `background`) | |
| Primitives · Controls | `Input` | `components/controls/input.tsx` | `Input` | |
| Primitives · Controls | `Settings Select` | `components/controls/settings-select.tsx` | `SettingsSelect`, `SettingsSelectTrigger`, `selectTrigger` (class string) | |
| Primitives · Controls | `Switch/On`, `Switch/Off` | `components/controls/switch.tsx` | `Switch` | |
| Primitives · Controls | `Checkbox/Off`, `Checkbox/On`, `Checkbox/Mixed` | `components/controls/checkbox.tsx` | `Checkbox`, `CheckboxMark` | |
| Primitives · Controls | `Checkbox Row` | `app/shell/dialogs/fields.tsx` | `CheckboxRow` | in app |
| Primitives · Controls | `Kbd`, `Kbd/Tooltip`, `Kbd/On button` | `components/controls/kbd.tsx` | `Kbd` (`variant` key/chip; the tooltip and button pills follow where it sits) | |
| Primitives · Controls | `KbdGroup` | — | — | Pen only: keys sit side by side in their parent |
| Primitives · Controls | `TabsList`, `TabsList/Line`, `TabsList/Vertical`, `Tab/Active`, `Tab/Inactive`, `Tab/Line Active`, `Tab/Line Inactive` | `components/controls/tabs.tsx` | `Tabs`, `TabsList` (`variant` default/line, orientation), `TabsTrigger`, `TabsContent` | |
| Primitives · Controls | `Toggle/Off`, `Toggle/On` | `components/controls/toggle.tsx` | `Toggle` | |
| Primitives · Controls | `ToggleGroup`, `ToggleGroupItem/On`, `ToggleGroupItem/Off` | `components/controls/toggle-group.tsx` | `ToggleGroup`, `ToggleGroupItem` | |
| Primitives · Controls | `Segmented`, `Segmented/Fill`, `Segment/On`, `Segment/Off` | `components/controls/segmented.tsx` | `Segmented` (`fill`), `Segment`, `SegmentedRoot`, `Choice` | |
| Primitives · Controls | — | `components/controls/slot.tsx` | `Slot` | code only (no look) |
| Primitives · Controls | `Skeleton` | `components/layout/skeleton.tsx` | `Skeleton` | |
| Primitives · Controls | `Separator/Horizontal`, `Separator/Vertical` | — | — | Pen only: the code draws rules with border classes (`separator.tsx` was removed) |
| Primitives · Controls | `RoundBadge/Plain`, `RoundBadge/Done` | `components/controls/round-badge.tsx` | `RoundBadge` (`tone`) | |
| Primitives · Controls | `Dot` | `components/shared/state-dot.tsx` | `StateDot` (`color`) | |
| Primitives · Controls | `ErrorLine` | `app/shell/dialogs/fields.tsx` | `ErrorLine` | in app |
| Primitives · Controls | `Nav Row` | `app/shell/sidebar/nav.tsx` | `navRow` (class string) | in app |
| Primitives · Menus, popovers & dialogs | `Menu`, `Menu Item`, `Menu Item/Check`, `Menu Item/Radio`, `Menu Item/Sub`, `Menu Label`, `Menu Separator` | `components/overlays/dropdown-menu.tsx` | `DropdownMenuContent`, `DropdownMenuItem`, `DropdownMenuCheckboxItem`, `DropdownMenuRadioItem`, `DropdownMenuSubTrigger`, `DropdownMenuLabel` (`variant` caps/heading), `DropdownMenuSeparator` | |
| Primitives · Menus, popovers & dialogs | — | `components/overlays/dropdown-menu.tsx` | `MenuRow` | code only (the two-line menu row) |
| Primitives · Menus, popovers & dialogs | — | `components/overlays/context-menu.tsx` | `ContextMenu*` | code only (same look as the menu) |
| Primitives · Menus, popovers & dialogs | `Tooltip` | `components/overlays/tooltip.tsx` | `Tooltip`, `TooltipTrigger`, `TooltipContent`, `TooltipProvider` | |
| Primitives · Menus, popovers & dialogs | `Dialog`, `Dialog/With close`, `DialogOverlay` | `components/overlays/dialog.tsx` | `DialogContent`, `DialogHeader`, `DialogTitle`, `DialogDescription`, `DialogFooter`, `DialogOverlay`, `DialogPlain*` | |
| Primitives · Menus, popovers & dialogs | — | `components/overlays/confirm-dialog.tsx` | `ConfirmDialog`, `ConfirmForm` | code only |
| Primitives · Menus, popovers & dialogs | `Toast`, `Toast/Success`, `Toast/Error`, `Toast/Neutral` | `components/overlays/toast.tsx` | `Toaster` (each toast is its file-local `ToastItem`) | |
| Primitives · Menus, popovers & dialogs | `PopoverContent` | `components/overlays/popover.tsx` | `Popover`, `PopoverTrigger`, `PopoverAnchor`, `PopoverContent` | |
| Primitives · Menus, popovers & dialogs | `ScrollBar/Vertical`, `ScrollBar/Horizontal` | `styles/globals.css` | — | Pen only as a component: scroll bars are the CSS-styled native ones (`scroll-area.tsx` was removed) |
| Primitives · Menus, popovers & dialogs | `ScrollFade/Top`, `ScrollFade/Bottom` | `components/layout/scroll-fade.tsx` | `ScrollFade` | |
| Primitives · Menus, popovers & dialogs | — | `components/layout/collapsible.tsx`, `sidebar.tsx`, `sidebar-layout.tsx` | `Collapsible*`, `SidebarPanel`, `SidebarProvider`, `SidebarReveal`, `SidebarResizeHandle` | code only (behaviour, no look of their own) |
| Glyphs & indicators | `SidebarGlyph/Open`, `SidebarGlyph/Closed (floating)` | `app/shell/SidebarToggle.tsx` | `SidebarGlyph` | in app |
| Glyphs & indicators | `StepIcon/read` … `StepIcon/stop` | `app/conversation/activity/ActivityGroup.tsx` | `StepIcon` (file-local) | in app |
| Glyphs & indicators | `FileTypeIcon/Image`, `Document`, `Data`, `Code` | `components/conversation/elements/file-type-icon.tsx` | `FileTypeIcon` | |
| Glyphs & indicators | `WorkerGlyph` | `app/conversation/WorkerChip.tsx` | `WorkerGlyph` | in app |
| Glyphs & indicators | `SidebarActivityMark/Working`, `/Unread`, `SidebarError`, `SessionStatusLabel` | `app/shell/SessionStatus.tsx` | `SidebarActivityMark`, `SidebarError`, `SessionStatusLabel` | in app |
| Glyphs & indicators | `StateIcon/Running`, `StateIcon/Paused` | `app/conversation/Activities.tsx` | `StateIcon` (file-local) | in app |
| Glyphs & indicators | `Donut` | `components/conversation/elements/donut.tsx` | `Donut` | |
| Glyphs & indicators | `StepGlyph/Done`, `Failed`, `Stopped`, `Pending` | `components/conversation/elements/step-glyph.tsx` | `StepGlyph` | |
| Glyphs & indicators | `CiGlyph/Passed` | `components/conversation/elements/ci-glyph.tsx` | `CiGlyph` | |
| Glyphs & indicators | `MockupTabIcon` | `app/conversation/MockupTabIcon.tsx` | `MockupTabIcon` | in app |
| Glyphs & indicators | `FaviconGlyph/Globe`, `Favicon`, `Loading`, `Loading with favicon` | `app/conversation/BrowserTabIcon.tsx` | `FaviconGlyph`, `BrowserTabIcon` | in app |
| Glyphs & indicators | `SiteIcon/Globe`, `SiteIcon/Favicon` | `app/side-panel/NewTabTools.tsx` | `SiteIcon` (file-local) | in app |
| Glyphs & indicators | `Counts` | `app/conversation/Counts.tsx` | `Counts` (`dim`, `quiet`) | in app |
| Glyphs & indicators | `SourceCountBadge` | `app/side-panel/WorkspacePane.tsx` | — (inline `data-slot="source-count"` span) | in app |
| Glyphs & indicators | `Share` | `app/shell/BarStatus.tsx` | `Share` (file-local) | in app |
| Shared app parts | `TitlebarNav` | `app/shell/AppSidebar.tsx` | `TitlebarNav` | in app |
| Shared app parts | `RowAction`, `RowActionsSlot` | `app/shell/sidebar/nav.tsx`, `app/shell/AppSidebar.tsx` | `rowAction` (class string), `RowActionsSlot` (file-local) | in app |
| Shared app parts | `ConversationRow` | `app/shell/AppSidebar.tsx` | `ConversationRow` (file-local, on `navRow`) | in app |
| Shared app parts | `SidebarToggle/Open` | `app/shell/SidebarToggle.tsx` | `SidebarToggle` | in app |
| Shared app parts | `TerminalButton` | `app/side-panel/SidePanel.tsx` | `TerminalButton` | in app |
| Shared app parts | `NavHeader`, `NavSection` | `app/shell/sidebar/nav.tsx` | `NavHeader`, `NavSection` | in app |
| Shared app parts | `CopyButton` | `components/shared/copy-button.tsx` | `CopyButton` | |
| Shared app parts | `ModelSelector` | `components/shared/model-selector.tsx` | `ModelSelector` | |
| Shared app parts | `ProgressLine` | `components/shared/progress-line.tsx` | `ProgressLine` | |
| Shared app parts | `Section` | `app/conversation/cards/common.tsx` | `Section` | in app |
| Shared app parts | `SettingsButton` | `components/controls/button.tsx` | `Button variant="soft" size="sm"` | |
| Shared app parts | `SettingsSearch` | `components/shared/settings-search.tsx` | `SettingsSearch` | |
| Shared app parts | `Settings Card`, `Settings Row` | `components/shared/settings-card.tsx`, `settings-row.tsx` | `SettingsCard`, `RowText`, `settingsRows`; `SettingsRow` | |
| Shared app parts | `SettingsTitlebarStrip` | `app/App.tsx` | — (inline strip holding `TerminalButton`) | in app |
| Shared app parts | `LabeledDivider` | `components/shared/labeled-divider.tsx` | `LabeledDivider` | |
| Shared app parts | `MD/Paragraph`, `MD/Bullet item` | `components/conversation/markdown-text.tsx` | `MarkdownText` | |
| Shared app parts | — | `components/shared/banner.tsx`, `empty-state.tsx`, `usage-bar.tsx`, `titlebar-button.tsx` | `Banner`, `EmptyState`, `UsageBar`, `TITLEBAR_BUTTON`, `TitlebarTips` | code only |
| Inspector (settings.pen) | `#1005 OrchestratorTab › session bar` | `app/inspector/SessionBar.tsx` | `SessionBar` | |
| Inspector (settings.pen) | `#1006 OrchestratorLogView` | `app/inspector/OrchestratorLogView.tsx` | `OrchestratorLogView` | |
| Inspector (settings.pen) | `#1007 OrchestratorLogView › load earlier line` | `app/inspector/LoadEarlierLine.tsx` | `LoadEarlierLine` | |
| Inspector (settings.pen) | `#1008 OrchestratorLogView › list switch bar` | `app/inspector/ListSwitchBar.tsx` | `ListSwitchBar` | |
| Inspector (settings.pen) | `#1009 Breaches` | `app/inspector/Breaches.tsx` | `Breaches` | |
| Inspector (settings.pen) | `#1010 ContextMeter` | `app/inspector/ContextMeter.tsx` | `ContextMeter` | |
| Inspector (settings.pen) | `#1011 InjectionRowView` | `app/inspector/InjectionRowView.tsx` | `InjectionRowView` | |
| Inspector (settings.pen) | `#1012 GenerationHeader` | `app/inspector/GenerationHeader.tsx` | `GenerationHeader` | |
| Inspector (settings.pen) | `#1013 InjectionList` | `app/inspector/InjectionList.tsx` | `InjectionList` | |
| Inspector (settings.pen) | `#1020 BlobText` | `app/inspector/BlobText.tsx` | `BlobText` | |
| Inspector (settings.pen) | `#1021 RebirthDetail` | `app/inspector/RebirthDetail.tsx` | `RebirthDetail` | |
| Inspector (settings.pen) | `#1022 RebirthDetail › briefing sections table` | `app/inspector/BriefingSectionsTable.tsx` | `BriefingSectionsTable` | |
| Inspector (settings.pen) | `#1023 RebirthRowView` | `app/inspector/RebirthRowView.tsx` | `RebirthRowView` | |
| Inspector (settings.pen) | `#1039 Section (Brain)` | `app/inspector/brain/BrainSection.tsx` | `BrainSection` | |
| Inspector (settings.pen) | `#1040 Detail (Brain)` | `app/inspector/brain/BrainDetail.tsx` | `BrainDetail` | |
| Inspector (settings.pen) | `#1041 Progress` | `app/inspector/brain/BrainProgress.tsx` | `BrainProgress` | |
| Inspector (settings.pen) | `#1043 BrainOverviewView › nodes-by-kind list` | `app/inspector/brain/NodesByKindList.tsx` | `NodesByKindList` | |
| Inspector (settings.pen) | `#1044 indexState` | `app/inspector/brain/IndexState.tsx` | `IndexState` | |
| Inspector (settings.pen) | `#1045 IndexSection` | `app/inspector/brain/IndexSection.tsx` | `IndexSection` | |
| Inspector (settings.pen) | `#1046 embedderState` | `app/inspector/brain/EmbedderState.tsx` | `EmbedderState` | |
| Inspector (settings.pen) | `#1047 EmbedderSection` | `app/inspector/brain/EmbedderSection.tsx` | `EmbedderSection` | |
| Inspector (settings.pen) | `#1048 WorkersSection` | `app/inspector/brain/WorkersSection.tsx` | `WorkersSection` | |
| Inspector (settings.pen) | `#1049 JobRow` | `app/inspector/brain/JobRow.tsx` | `JobRow` | |
| Inspector (settings.pen) | `#1050 JobsSection` | `app/inspector/brain/JobsSection.tsx` | `JobsSection` | |
| Inspector (settings.pen) | `#1051 DeveloperActions` | `app/inspector/brain/DeveloperActions.tsx` | `DeveloperActions` | |
| Inspector (settings.pen) | `#1054 BrainSearch › query bar` | `app/inspector/brain/QueryBar.tsx` | `QueryBar` | |
| Inspector (settings.pen) | `#1055 BrainSearch › result summary line` | `app/inspector/brain/ResultSummary.tsx` | `ResultSummary` | |
| Inspector (settings.pen) | `#1052 HitRow` | `app/inspector/brain/HitRow.tsx` | `HitRow` | |
| Inspector (settings.pen) | `#1056 BrainSearch › “What the orchestrator reads” disclosure` | `app/inspector/brain/OrchestratorReadsDisclosure.tsx` | `OrchestratorReadsDisclosure` | |
| Inspector (settings.pen) | `#1058 BrainTab › toolbar` | `app/inspector/brain/BrainToolbar.tsx` | `BrainToolbar` | |
| Inspector (settings.pen) | `#1060 ConversationName` | `app/inspector/brain/ConversationName.tsx` | `ConversationName` | |
| Inspector (settings.pen) | `#1065 NodeDetail › node link` | `app/inspector/brain/NodeLink.tsx` | `NodeLink` | |
| Inspector (settings.pen) | `#1067 ActionError` | `app/inspector/providers/ActionError.tsx` | `ActionError` | |
| Inspector (settings.pen) | `#1073 StartSessionForm` | `app/inspector/providers/StartSessionForm.tsx` | `StartSessionForm` | |
| Inspector (settings.pen) | `#1074 StartSessionForm › Record toggle` | `app/inspector/providers/RecordToggle.tsx` | `RecordToggle` | |
| Inspector (settings.pen) | `#1075 SessionList` | `app/inspector/providers/SessionList.tsx` | `SessionList` | |
| Inspector (settings.pen) | `#1076 SessionList › session row` | `app/inspector/providers/SessionRow.tsx` | `SessionRow` | |
| Inspector (settings.pen) | `#1077 FixtureList` | `app/inspector/providers/FixtureList.tsx` | `FixtureList` | |
| Inspector (settings.pen) | `#1078 FixtureList › fixture row` | `app/inspector/providers/FixtureRow.tsx` | `FixtureRow` | |
| Inspector (settings.pen) | `#1080 RawSessionView › approvals tray` | `app/inspector/providers/ApprovalsTray.tsx` | `ApprovalsTray` | |
| Inspector (settings.pen) | `#1081 SessionHeader` | `app/inspector/providers/SessionHeader.tsx` | `SessionHeader` | |
| Inspector (settings.pen) | `#1082 TranscriptList` | `app/inspector/providers/TranscriptList.tsx` | `TranscriptList` | |
| Inspector (settings.pen) | `#1083 ApprovalCard (RawSessionView)` | `app/inspector/providers/RawApprovalCard.tsx` | `RawApprovalCard` | |
| Inspector (settings.pen) | `#1084 Composer (raw session)` | `app/inspector/providers/RawSessionComposer.tsx` | `RawSessionComposer` | |
| Conversation (app.pen) | `#276 ApprovalAction › inline: Reply… button` | `app/conversation/ReplyButton.tsx` | `ReplyButton` | |
| Conversation (app.pen) | `#279 CardDictate` | `app/conversation/CardDictate.tsx` | `CardDictate` | |
| Conversation (app.pen) | `#280 ChoiceCard` | `app/conversation/ChoiceCard.tsx` | `ChoiceCard` | |
| Conversation (app.pen) | `#281 QuestionAction` | `app/conversation/QuestionAction.tsx` | `QuestionAction` | |
| Conversation (app.pen) | `#283 ImplementPlanAction` | `app/conversation/ImplementPlanAction.tsx` | `ImplementPlanAction` | |
| Conversation (app.pen) | `#284 PlanAction` | `app/conversation/PlanAction.tsx` | `PlanAction` | |
| Conversation (app.pen) | `#285 PhasePlanAction` | `app/conversation/PhasePlanAction.tsx` | `PhasePlanAction` | |
| Conversation (app.pen) | `#286 OvernightAction` | `app/conversation/OvernightAction.tsx` | `OvernightAction` | |
| Conversation (app.pen) | `#364 ChangesCard › FileRow` | `app/conversation/ChangesFileRow.tsx` | `ChangesFileRow` | |
| Conversation (app.pen) | `#366 ChangesCard › inline: merge warning row` | `app/conversation/MergeWarningRow.tsx` | `MergeWarningRow` | |
| Conversation (app.pen) | `#367 ChangesCard › inline: revert confirm row` | `app/conversation/RevertConfirmRow.tsx` | `RevertConfirmRow` | |
| Settings (settings.pen) | `#684 SettingsPage › back link` | `app/settings/BackLink.tsx` | `BackLink` | |
| Settings (settings.pen) | `#607 BrowserDownloads › inline: download row` | `app/settings/DownloadRow.tsx` | `DownloadRow` | |
| Settings (settings.pen) | `#610 BrowserHistory › inline: day group (collapsible card)` | `app/settings/DayGroup.tsx` | `DayGroup` | |
| Settings (settings.pen) | `#611 VisitRow` | `app/settings/VisitRow.tsx` | `VisitRow` | |
| Settings (settings.pen) | `#612 FilterMenu (history)` | `app/settings/FilterMenu.tsx` | `FilterMenu` | |
| Settings (settings.pen) | `#614 BrowserImportPage › inline: status line (error / none found)` | `app/settings/ImportStatusLine.tsx` | `ImportStatusLine` | |
| Settings (settings.pen) | `#615 BrowserImportPage › inline: browser-off note` | `app/settings/BrowserOffNote.tsx` | `BrowserOffNote` | |
| Settings (settings.pen) | `#616 ListingRows` | `app/settings/ListingRows.tsx` | `ListingRows` | |
| Settings (settings.pen) | `#617 BrowserIcon` | `app/settings/BrowserIcon.tsx` | `BrowserIcon` | |
| Settings (settings.pen) | `#618 BrowserRow` | `app/settings/BrowserRow.tsx` | `BrowserRow` | |
| Settings (settings.pen) | `#619 BrowserRow › inline: status dot + text` | `app/settings/BrowserStatus.tsx` | `BrowserStatus` | |
| Settings (settings.pen) | `#620 FlowHeader` | `app/settings/FlowHeader.tsx` | `FlowHeader` | |
| Settings (settings.pen) | `#622 ImportBar` | `app/settings/ImportBar.tsx` | `ImportBar` | |
| Settings (settings.pen) | `#623 ChooseStep` | `app/settings/ChooseStep.tsx` | `ChooseStep` | |
| Settings (settings.pen) | `#624 ProfilePick` | `app/settings/ProfilePick.tsx` | `ProfilePick` | |
| Settings (settings.pen) | `#626 BrowserPage › inline: off note` | `app/settings/BrowserOffNote.tsx` | `BrowserOffNote` | |
| Settings (settings.pen) | `#627 LocationRow` | `app/settings/LocationRow.tsx` | `LocationRow` | |
| Settings (settings.pen) | `#628 BrowserSwitch` | `app/settings/BrowserSwitch.tsx` | `BrowserSwitch` | |
| Settings (settings.pen) | `#629 LinkRow` | `app/settings/LinkRow.tsx` | `LinkRow` | |
| Settings (settings.pen) | `#632 BrowserSiteSettings › inline: site row` | `app/settings/SiteRow.tsx` | `SiteRow` | |
| Settings (settings.pen) | `#634 ClearForm` | `app/settings/ClearForm.tsx` | `ClearForm` | |
| Settings (settings.pen) | `#635 ClearForm › inline: tick row` | `app/settings/TickRow.tsx` | `TickRow` | |
| Conversation (app.pen) | `#289 ConversationComposer › inline: composer card` | `app/conversation/ComposerCard.tsx` | `ComposerCard` | |
| Conversation (app.pen) | `#290 ComposerInput` | `app/conversation/ComposerInput.tsx` | `ComposerInput` | |
| Conversation (app.pen) | `#292 ComposerContextRing` | `app/conversation/ComposerContextRing.tsx` | `ComposerContextRing` | |
| Conversation (app.pen) | `#293 FullAccessNotice` | `app/conversation/FullAccessNotice.tsx` | `FullAccessNotice` | |
| Conversation (app.pen) | `#294 UtilityBar` | `app/conversation/UtilityBar.tsx` | `UtilityBar` | |
| Conversation (app.pen) | `#295 SendControls` | `app/conversation/SendControls.tsx` | `SendControls` | the button itself is `SendButton` (Primitives · Controls) |
| Conversation (app.pen) | `#296 DictationNote` | `app/conversation/DictationNote.tsx` | `DictationNote` | |
| Conversation (app.pen) | `#297 DictateButton` | `app/conversation/DictateButton.tsx` | `DictateButton` | |
| Conversation (app.pen) | `#298 ComposerFooter` | `app/conversation/ComposerFooter.tsx` | `ComposerFooter` | |
| Conversation (app.pen) | `#299 DictationBar` | `app/conversation/DictationBar.tsx` | `DictationBar` | |
| Conversation (app.pen) | `#300 DictationBar › inline: waveform` | `app/conversation/DictationWaveform.tsx` | `DictationWaveform` | |
| Conversation (app.pen) | `#301 ComposerHint` | `app/conversation/ComposerHint.tsx` | `ComposerHint` | |
| Settings (settings.pen) | `#645 AppearanceSection` | `app/settings/AppearanceSection.tsx` | `AppearanceSection` | |
| Settings (settings.pen) | `#646 NameRow` | `app/settings/NameRow.tsx` | `NameRow` | |
| Settings (settings.pen) | `#647 ComposerSection (general)` | `app/settings/ComposerSection.tsx` | `ComposerSection` | |
| Settings (settings.pen) | `#648 NotificationsSection` | `app/settings/NotificationsSection.tsx` | `NotificationsSection` | |
| Settings (settings.pen) | `#649 NotificationsSection › inline: dimmed row (notifications off)` | `app/settings/DimmedRow.tsx` | `DimmedRow` | |
| Settings (settings.pen) | `#650 PowerSection` | `app/settings/PowerSection.tsx` | `PowerSection` | |
| Settings (settings.pen) | `#651 OpenAtLoginRow` | `app/settings/OpenAtLoginRow.tsx` | `OpenAtLoginRow` | |
| Settings (settings.pen) | `#652 AboutSection` | `app/settings/AboutSection.tsx` | `AboutSection` | |
| Settings (settings.pen) | `#653 AboutSection › inline: version value` | `app/settings/VersionValue.tsx` | `VersionValue` | |
| Settings (settings.pen) | `#654 HibernateRow` | `app/settings/HibernateRow.tsx` | `HibernateRow` | |
| Settings (settings.pen) | `#675 Picture (sidebar closed preview)` | `app/settings/SidebarClosedPicture.tsx` | `SidebarClosedPicture` | |
| Conversation (app.pen) | `#302 Pill (ComposerCapsule)` | `app/conversation/CapsulePill.tsx` | `CapsulePill` | |
| Conversation (app.pen) | `#303 StepList` | `app/conversation/StepList.tsx` | `StepList` | |
| Conversation (app.pen) | `#305 RunCapsule` | `app/conversation/RunCapsule.tsx` | `RunCapsule` | |
| Conversation (app.pen) | `#306 Capsule` | `app/conversation/Capsule.tsx` | `Capsule` | |
| Conversation (app.pen) | `#307 RequestCapsule` | `app/conversation/RequestCapsule.tsx` | `RequestCapsule` | |
| Conversation (app.pen) | `#308 DiffPill` | `app/conversation/DiffPill.tsx` | `DiffPill` | |
| Conversation (app.pen) | `#310 Toggle (ComposerStatusSlot)` | `app/conversation/StatusSlotToggle.tsx` | `StatusSlotToggle` | |
| Conversation (app.pen) | `#311 CiPill` | `app/conversation/CiPill.tsx` | `CiPill` | |
| Conversation (app.pen) | `#312 MergePill` | `app/conversation/MergePill.tsx` | `MergePill` | |
| Conversation (app.pen) | `#314 ComposerStatusSlot › inline: folded count badge` | `app/conversation/FoldedCountBadge.tsx` | `FoldedCountBadge` | |

## Icons

Every icon the app draws lives in `apps/desktop/src/components/icons/`, one folder per set, and
app code imports them from `@/components/icons` only. The style check fails `<svg>` markup
anywhere else and the kit's icons imported anywhere but `icons/kit.ts`; a drawing made from live
values (a chart, a progress ring) is listed in `check-styles.allow` as `<svg` with its reason.

In `foundations.pen` the Icons board holds every icon component, one section per folder, and a
code name is its component's name:

| Icons board section | Folder | Pen names → code |
|---|---|---|
| UI kit | `icons/kit.ts` | `Icon / <Name>` → the kit's `<Name>` |
| App glyphs | `icons/app-glyphs/` | `Icon / <Name>` → `<Name>` (codicons, diff, queue, help, feedback, shields, sidebar status, routing line icons, page drag handle, plan partial, spinner) |
| File types | `icons/file-types/` | `FileIcon/<kind>` → `FileIcon` for that kind (`fileKind`) |
| Worker shapes | `icons/worker-shapes/` | `IdGlyph/<shape>` → `IdGlyph`'s shape of that name |
| Page blocks | `icons/page-blocks/` | `BlockIcon/<label>` → `BLOCK_ICONS[<kind>]` of the block with that label |
| Marks | `icons/marks/` | `BrigadierGlyph`, `ProviderGlyph/<agent>` → the same names |

An icon is drawn once, as a component on the Icons board; everything else (the Glyphs &
indicators board, controls, the other files) places it as a ref.

## Keeping it in sync

- When the UI changes, update the component and all its states first, then the screens that show
  it. Parts both windows use go in `foundations.pen`; parts used by one window go on
  its surface's or page's board in that window's file.
- When code removes a component, delete it from `unused.pen` too (and its foundations copy if its
  note says so).
- Agents editing through the MCP: pass the file's full real path, and make sure the file is open
  in Pen, or the call silently works on whichever file is active. Never `Replace` a child of an
  instance of a reusable component (insert the new part, move it, delete the old one). After a
  failed edit, close the file without saving and reopen it.
- Save with ⌘S in that file's own Pen window and check the file's modified time. No backup copies.

## Updating tokens

`apps/desktop/src/styles/tokens.css` takes its design tokens from `foundations.pen`: the
`@theme static` block between `/* generated from foundations.pen … */` and `/* end generated */`
is written by `apps/desktop/scripts/gen-tokens.mjs`, one declaration per Pen variable. Don't edit
that block by hand: change the variable in Pen, then:

1. Open `foundations.pen` in Pen.
2. Export its variables (an agent, through the pencil MCP, read-only): `execute` with
   `filePath` `~/Documents/brigadier-design/foundations.pen` written out in full, and the snippet
   `Print(JSON.stringify(GetVariables().variables))`. Write the printed object, unchanged, to
   `apps/desktop/scripts/pen-vars.json`.
3. In `apps/desktop`, run `node scripts/gen-tokens.mjs`, then commit `pen-vars.json` and
   `tokens.css` together.

How values come out:

- A token whose Pen value matches its value in the block keeps its text and its comment, so
  `oklch(… var(--tint))`, `color-mix()` and rem stay as written. Colours match within one step
  per channel (Pen stores 8-bit hex); lengths match when they're the same in px.
- A changed token takes Pen's value: numbers as rem (px ÷ 16), colours as hex, strings as they
  are, a Pen variable that points at another as `var(--other)`. A new variable goes at the block's
  end; one removed from Pen leaves the block.
- Pen holds colours as plain values, not as mixes of others. Change `--foreground` or
  `--background` in Pen alone and the code's `color-mix()` colours built on it no longer match
  Pen's, so they are written as Pen's hex: change those in Pen too if they should follow.
- Some Pen variables aren't written, listed in the script's `SKIP`: the font variables (Pen holds
  stand-ins for the system stacks), `--radius-capsule` (999 in Pen, 9999px in code, both fully
  round), and `--wash-10`, `--warning-fill`, `--success-fill` (Pen's stand-ins for opacity
  modifiers).
- Pen can't put an opacity on a variable, so a token at an opacity (the code's
  `border-foreground/16`) is drawn in Pen as that colour's hex with alpha (`#FAFAF929`). Shadow
  colours and icon artwork are raw hex in Pen too. None of these are Pen variables.
- Outside the block, `tokens.css` stays hand-written: Tailwind's resets and `@theme inline`
  aliases, the font stacks, values built with `calc()`, `min()`, `clamp()` or em/vh/lh, and
  platform overrides.

`pnpm lint` runs `node scripts/gen-tokens.mjs --check`, which fails when the block differs from
the committed `pen-vars.json` (CI can't read Pen, so it checks against that export).
