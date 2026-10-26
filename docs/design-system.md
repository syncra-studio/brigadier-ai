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
| `foundations.pen` | Every component, nothing else. The design system boards (Tokens & type, Icons, Primitives · Controls, Primitives · Menus, popovers & dialogs, Glyphs & indicators, Shared app parts), then one board per area of the main window under "Group · App", then one board per area of Settings under "Group · Settings". |
| `app.pen` | The main window's screens only: 15 boards, one per surface, in four groups (App shell, Conversation, Side panel, Setup). No components of its own. |
| `settings.pen` | The Settings window's screens only, the Inspector (a page inside Settings) among them: one board per page in `pages.ts` order. No components of its own. |

## Boards

- `foundations.pen` area boards of the main window: Sidebar & shell, Top bar & tabs, Thread,
  Markdown, code & images, Thread cards, Steps & tool rows, Composer, Composer menus, rail &
  queue, Summary pane, Files, search & source, Changes, review & conflicts, Terminal, Workers,
  Page, Job, mockup & new tab, Output, plan & side chat, Browser, Onboarding & setup, App dialogs.
  Settings area boards are named "Settings · <area>" (Window & page parts, General & system,
  Providers, Plugins, Routing, Browser, Storage & archived chats, and three Inspector boards).
- A component sits on one card: the card's title is its plain name, and its states are cells in
  one row, at most six, each with a short label under it. A line on the card only when it says a
  behaviour the picture can't show.
- `app.pen` and `settings.pen` boards hold screens only, about twelve at most per board: whole
  surfaces in real states, built from instances of foundations components. Hover, tooltips and
  pressed looks are component states on the foundations card, not screens.
- `app.pen` boards: Window & sidebar, Top bar & tabs, Onboarding & setup, App dialogs, Home &
  thread, Composer, Summary pane, and the side panel's Files & source, Changes & review, Terminal,
  Workers, Page, Job, mockup & new tab, Output, plan & side chat, Browser. `settings.pen` boards:
  Settings window, then one per page.
- Boards sit in rows, not columns, left to right in app order, with a label above each group.
  A board is landscape; when a surface needs more room it gets another board beside it.

## How the files link

Every file except `foundations.pen` imports it under the alias `f`, and nothing else. Imports are
one level deep: a foundations component never holds anything from another file, and every part,
used by one window or both, lives in `foundations.pen`.

- Instance of a foundations component: `{ "type": "ref", "ref": "f:<id>" }`.
- Override inside it: every path segment that names a foundations node carries the prefix,
  `"f:<childId>/f:<childId>"`.
- Variable: `"$f:--foreground"`.

## Names

- Components: a plain name, a look after a slash: `Button/Primary/sm`, `Icon / Sparkle`,
  `Project row`, `Add project dialog/Clone`. Settings parts keep their code names
  (`SettingsPage/BackLink`). No numbers and no code paths on cards or screens.
- Every state of a component is drawn and labelled under it; every screen sits in a cell headed
  by its state (`Empty`, `Pasted text`).

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
| `controls/` | Primitives · Controls | button, icon button, badge, input, checkbox, switch, segmented, tabs, toggle, toggle group, key chip, source count badge |
| `overlays/` | Primitives · Menus, popovers & dialogs | dialog, confirm dialog, popover, dropdown and context menus, tooltip, toast |
| `layout/` | ScrollFade (Menus board), Skeleton (Controls board) | sidebar, sidebar layout, collapsible, scroll fade, skeleton |
| `shared/` | Shared app parts | settings card and search, state dot, usage bar, banner, empty state, titlebar button, copy button, model selector, progress line, labeled divider, terminal button, site icon, counts, section |
| `conversation/` | Thread, Markdown, code & images, Thread cards, Steps & tool rows, Composer, Composer menus, rail & queue | the conversation kit: thread, message rail, markdown, code blocks, composer parts, transcript; `kit.ts` is the one door to the chat library |
| `views/<view>/` | Terminal, Changes, review & conflicts, Page, Settings · Inspector · Brain | views built on a drawing library, each its own folder so it loads on demand: `terminal` (xterm), `diff` (@pierre/diffs), `brain-graph` (sigma), `page-editor` (lexical) |

The libraries that draw (the chat library, lexical, xterm, sigma/graphology, @pierre/diffs) are
imported only inside `src/components`; the style check fails them anywhere else.

`apps/desktop/src/app/` holds the screens, one folder per surface as the boards group them:
`shell/` (window, sidebar, top bar and session tabs, app dialogs; parts on Sidebar & shell, Top
bar & tabs, App dialogs), `conversation/` (parts on the Thread, Steps & tool rows, Thread cards
and Composer boards), `side-panel/` (Summary pane, Files, search & source, Changes, review &
conflicts, Terminal, Workers, Page, Job, mockup & new tab, Output, plan & side chat, Browser),
`setup/` (onboarding, computer-use setup; Onboarding & setup), with their screens in `app.pen`;
`settings/` (its pages, Providers and Routing among them) and `inspector/`, parts on the "Settings · …"
boards, screens in `settings.pen`. App-wide wiring (`App.tsx`, shortcuts, smoke) stays at the
top of `app/`.

## Pen component → code

The `foundations.pen` board and component that show each code part, board by board, with the
file (under `apps/desktop/src`) and the export that draws it. "Pen only" means the code has no
such part ("the code was removed in <commit>": the strip-down took it out, the card stays until the
design drops it); "code only" means a component folder holds a part that has no card yet. "Look of X"
means the part has no component of its own: it is a look of X. "Shown inside it" means it is
drawn inside that component, with no card of its own. Parts still in `src/app` (not yet under
`src/components`) are noted "in app"; the per-surface "Design system: … parts as reusable
components" issues track moving them. Cards in Pen carry no code paths: this table is the link.

| Board | Pen component | Code file | Export | Note |
|---|---|---|---|---|
| Icons | `Icon / <Name>`, `FileIcon/*`, `IdGlyph/*`, `BlockIcon/*`, `BrigadierGlyph`, `ProviderGlyph/*` | `components/icons/` | see Icons below | |
| Icons | `Spinner/12`, `/14`, `/16`, `/20` | `components/icons/app-glyphs/spinner.tsx` | `Spinner` (sized by class) | |
| Icons | — | `components/icons/app-glyphs/plugin-art.tsx` | `ServerArt`, `SkillArt` | code only: Settings › Plugins' illustrations for an item with no icon of its own |
| Icons | — | `components/icons/marks/server-brands.tsx` | `ServerBrand`, `SERVER_BRANDS` | code only: well-known MCP servers' makers' marks (Simple Icons, CC0) |
| Icons | `Icon / FileImage`, `Icon / FileDocument`, `Icon / FileCode`, `Icon / File` | `app/side-panel/FileGlyph.tsx` | `FileGlyph` | picks one of these icons by file type; no component of its own |
| Primitives · Controls | `Badge/Default`, `Secondary`, `Destructive`, `Warning`, `Success`, `Outline`, `Ghost`, `Link` | `components/controls/badge.tsx` | `Badge` (`variant`) | |
| Primitives · Controls | `Badge/Full access` | — | — | Pen only: the app's Full access pill is the access picker's trigger (`text-full-access`) in `app/shell/SetupPickers.tsx` |
| Primitives · Controls | `Button/Primary`, `Secondary`, `Outline`, `Ghost`, `Destructive`, `Link`, each `/xs` and `/sm` | `components/controls/button.tsx` | `Button` (`variant` default…link, `size` default/xs/sm) | |
| Primitives · Controls | — | `components/controls/button.tsx` | `Button` sizes `lg`, `inline` | code only (`soft`, `soft-destructive`, `link-blue` are drawn on Shared app parts) |
| Primitives · Controls | `Icon Button`, `Icon Button/xs`, `/sm`, `/lg` | `components/controls/tooltip-icon-button.tsx` | `TooltipIconButton` (`size`, `look` ghost/toolbar/strip, `pressed`, `dot`) | sizes come from `Button` `icon-*` |
| Primitives · Controls | `Send Button` | `components/controls/send-button.tsx` | `SendButton` (`state`, `background`) | |
| Primitives · Controls | `Input` | `components/controls/input.tsx` | `Input` | |
| Primitives · Controls | `Settings Select` | `components/controls/settings-select.tsx` | `SettingsSelect`, `SettingsSelectTrigger`, `selectTrigger` (class string) | |
| Primitives · Controls | `Switch/On`, `Switch/Off` | `components/controls/switch.tsx` | `Switch` | |
| Primitives · Controls | `Checkbox/Off`, `Checkbox/On`, `Checkbox/Mixed` | `components/controls/checkbox.tsx` | `Checkbox`, `CheckboxMark` | |
| Primitives · Controls | `Checkbox Row` | `components/controls/checkbox-row.tsx` | `CheckboxRow` | |
| Primitives · Controls | `Kbd`, `Kbd · chip`, `Kbd/Tooltip`, `Kbd/On button` | `components/controls/kbd.tsx` | `Kbd` (`variant` key/chip; the tooltip and button pills follow where it sits) | |
| Primitives · Controls | `TabsList`, `TabsList/Line`, `TabsList/Vertical`, `Tab/Active`, `Tab/Inactive`, `Tab/Line Active`, `Tab/Line Inactive` | `components/controls/tabs.tsx` | `Tabs`, `TabsList` (`variant` default/line, orientation), `TabsTrigger`, `TabsContent` | |
| Primitives · Controls | `Toggle/Off`, `Toggle/On` | `components/controls/toggle.tsx` | `Toggle` | |
| Primitives · Controls | `ToggleGroup`, `ToggleGroupItem/On`, `ToggleGroupItem/Off` | `components/controls/toggle-group.tsx` | `ToggleGroup`, `ToggleGroupItem` | |
| Primitives · Controls | `Segmented`, `Segmented/Fill`, `Segment/On`, `Segment/Off` | `components/controls/segmented.tsx` | `Segmented` (`fill`), `Segment`, `SegmentedRoot`, `Choice` | |
| Primitives · Controls | — | `components/controls/slot.tsx` | `Slot` | code only (no look) |
| Primitives · Controls | `Skeleton` | `components/layout/skeleton.tsx` | `Skeleton` | |
| Primitives · Controls | `Separator/Horizontal`, `Separator/Vertical` | — | — | Pen only: the code draws rules with border classes (`separator.tsx` was removed) |
| Primitives · Controls | `RoundBadge/Plain`, `RoundBadge/Done` | `components/controls/round-badge.tsx` | `RoundBadge` (`tone`) | |
| Primitives · Controls | `StateDot` | `components/shared/state-dot.tsx` | `StateDot` (`color`) | |
| Primitives · Controls | `ErrorLine` | `components/controls/error-line.tsx` | `ErrorLine` | |
| Primitives · Controls | `Nav Row` | `components/controls/nav-row.tsx` | `NavRow` | |
| Primitives · Controls | `Button/Ghost` | `app/conversation/ReplyButton.tsx` | `ReplyButton` | look of `Button/Ghost` |
| Primitives · Controls | `Icon Button/sm` | `app/conversation/CardDictate.tsx` | `CardDictate` | look of `Icon Button/sm` |
| Primitives · Controls | `Icon Button` | `app/conversation/DictateButton.tsx` | `DictateButton` | look of `Icon Button` |
| Primitives · Controls | `Button/Ghost/sm` | `app/setup/onboarding/SelectAllToggle.tsx` | `SelectAllToggle` | look of `Button/Ghost/sm` |
| Primitives · Controls | `Button/Primary` | `app/setup/onboarding/ContinueButton.tsx` | `ContinueButton` | look of `Button/Primary` |
| Primitives · Controls | `ErrorLine` | `app/inspector/providers/ActionError.tsx` | `ActionError` | look of `ErrorLine` |
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
| Glyphs & indicators | `SidebarGlyph/Open`, `SidebarGlyph/Closed (floating)` | `components/shared/sidebar-toggle.tsx` | `SidebarGlyph` | |
| Glyphs & indicators | `StepIcon/read` … `StepIcon/stop` | `components/conversation/elements/step-icon.tsx` | `StepIcon`, `stepIconName` | |
| Glyphs & indicators | `FileTypeIcon/Image`, `Document`, `Data`, `Code` | `components/conversation/elements/file-type-icon.tsx` | `FileTypeIcon` | |
| Glyphs & indicators | `WorkerGlyph` | `components/conversation/elements/worker-glyph.tsx` | `WorkerGlyph` | |
| Glyphs & indicators | `SidebarActivityMark/Working`, `/Unread`, `SidebarError`, `SessionStatusLabel` | `components/shared/session-status.tsx` | `SidebarActivityMark`, `SidebarError`, `SessionStatusLabel` | |
| Glyphs & indicators | `StateIcon/Running`, `StateIcon/Paused` | `components/conversation/elements/state-icon.tsx` | `StateIcon` | |
| Glyphs & indicators | `Donut` | — | — | Pen only: the code was removed in cb94ccca9 |
| Glyphs & indicators | `StepGlyph/Done`, `Failed`, `Stopped`, `Pending` | — | — | Pen only: the code was removed in cb94ccca9 |
| Glyphs & indicators | `CiGlyph/Passed` | — | — | Pen only: the code was removed in cb94ccca9 |
| Glyphs & indicators | `MockupTabIcon` | `components/conversation/elements/mockup-tab-icon.tsx` | `MockupTabIcon` (with `useMockup`, `mockupBusy`) | the tab's own hooks stay in `app/conversation/MockupTab.tsx` |
| Glyphs & indicators | `FaviconGlyph/Globe`, `Favicon`, `Loading`, `Loading with favicon` | `components/conversation/elements/favicon-glyph.tsx` | `FaviconGlyph` | its look is `favicon-glyph.css`; the browser tab's `BrowserTabIcon` stays in `app/conversation/BrowserTabIcon.tsx` |
| Glyphs & indicators | `SiteIcon/Globe`, `SiteIcon/Favicon` | `components/shared/site-icon.tsx` | `SiteIcon` | its look is the New tab's `new-tab-site-icon` class (`app/side-panel/NewTabTools.css`) |
| Glyphs & indicators | `Counts` | `components/shared/counts.tsx` | `Counts` (`dim`, `quiet`) | |
| Glyphs & indicators | `SourceCountBadge` | `components/controls/source-count-badge.tsx` | `SourceCountBadge` (`count`) | |
| Glyphs & indicators | `Share` | `components/shared/share.tsx` | `Share` | |
| Shared app parts | `TitlebarNav` | `components/shared/titlebar-nav.tsx` | `TitlebarNav` | |
| Shared app parts | `RowAction`, `RowActionsSlot` | `components/shared/row-action.tsx` | `RowAction`, `RowActionsSlot` | |
| Shared app parts | `ConversationRow` | `components/shared/conversation-row.tsx` | `ConversationRow` (on `NavRow`), `RowStatus` | |
| Shared app parts | `SidebarToggle/Open` | `components/shared/sidebar-toggle.tsx` | `SidebarToggle` | |
| Shared app parts | `TerminalButton` | `components/shared/terminal-button.tsx` | `TerminalButton` | |
| Shared app parts | `NavHeader`, `NavSection` | `components/shared/nav.tsx` | `NavHeader`, `NavSection` (with `NavFold`, `NavList`) | |
| Shared app parts | `CopyButton` | `components/shared/copy-button.tsx` | `CopyButton` | |
| Shared app parts | `ModelSelector` | `components/shared/model-selector.tsx` | `ModelSelector` | |
| Shared app parts | `Model picker`, `Model picker/Automatic`, `Model picker/Routing` | `components/shared/model-picker.tsx` | `ModelPicker`, `ModelPickerPanel` | foundations.pen `HLTDr`, `Dy5IF`, `Kq0Ar` |
| Shared app parts | `ProgressLine` | `components/shared/progress-line.tsx` | `ProgressLine` | |
| Shared app parts | `Section` | `components/shared/section.tsx` | `Section` | |
| Shared app parts | `Button/Soft`, `Button/Soft destructive` | `components/controls/button.tsx` | `Button variant="soft"` / `"soft-destructive"`, `size="sm"` | |
| Shared app parts | `Button/Link blue · chevron`, `Button/Link blue · icon` | `components/controls/button.tsx` | `Button variant="link-blue"` | |
| Shared app parts | `EmptyState/Line`, `EmptyState/Card`, `EmptyState/Centered` | `components/shared/empty-state.tsx` | `EmptyState` (`variant` line/card/centered) | |
| Shared app parts | `SettingsSearch` | `components/shared/settings-search.tsx` | `SettingsSearch` | |
| Shared app parts | `Settings Card`, `Settings Row` | `components/shared/settings-card.tsx`, `settings-row.tsx` | `SettingsCard`, `RowText`, `settingsRows`; `SettingsRow` | |
| Shared app parts | `SettingsTitlebarStrip` | `components/shared/settings-titlebar-strip.tsx` | `SettingsTitlebarStrip` (holds `TerminalButton`) | |
| Shared app parts | `LabeledDivider` | `components/shared/labeled-divider.tsx` | `LabeledDivider` | |
| Shared app parts | `MD/Paragraph`, `MD/Bullet item` | `components/conversation/markdown-text.tsx` | `MarkdownText` | |
| Shared app parts | — | `components/shared/banner.tsx`, `usage-bar.tsx`, `titlebar-button.tsx` | `Banner`, `UsageBar`, `TITLEBAR_BUTTON`, `TitlebarTips` | code only |
| Shared app parts | — | `components/shared/code-editor.tsx` | `CodeFileHeader`, `CodeEditor` | code only: the file tab's header and lines; `CodeEditor` draws a small file whole, read only or editable (Settings › Plugins' Open) |
| Shared app parts | `LabeledDivider` | `app/conversation/UsageLimitDivider.tsx` | `UsageLimitDivider` | look of `LabeledDivider` |
| Shared app parts | `LabeledDivider` | `app/conversation/activity/QuotaResumed.tsx` | `QuotaResumed` | look of `LabeledDivider` |
| Sidebar & shell | `Page corners` | `app/shell/PageCorners.tsx` | `PageCorners` | |
| Sidebar & shell | `Page surface` | `app/shell/PageSurface.tsx` | `PageSurface` | |
| Sidebar & shell | `Sidebar header` | `app/shell/SidebarHeader.tsx` | `SidebarHeader` | |
| Sidebar & shell | `Sidebar` | `app/shell/NewChatRow.tsx` | `NewChatRow` | shown inside it |
| Sidebar & shell | `Projects empty prompt` | `app/shell/ProjectsEmptyPrompt.tsx` | `ProjectsEmptyPrompt` | |
| Sidebar & shell | `Project row` | `app/shell/ProjectRow.tsx` | `ProjectRow` | |
| Sidebar & shell | `Projects flyout`, `Chats flyout` | `app/shell/Flyout.tsx` | `Flyout` | its lists (`ProjectsFlyout`, `ChatsFlyout`, `FlyoutConversation`) stay in `app/shell/AppSidebar.tsx` |
| Sidebar & shell | `Provider usage`, `Provider usage/Open`, `Provider usage/Limit`, `Provider usage/Sign in` | `app/shell/ProviderUsage.tsx` | `ProviderUsage` | |
| Sidebar & shell | `Help menu` | `app/shell/HelpMenu.tsx` | `HelpMenu` | |
| Sidebar & shell | `Update row` | `app/shell/UpdateRow.tsx` | `UpdateRow` | |
| Top bar & tabs | `Bar divider` | `app/shell/BarDivider.tsx` | `BarDivider` | |
| Top bar & tabs | `Chat tab`, `Chat tab/Inactive` | `app/shell/ChatTab.tsx` | `ChatTab` | |
| Top bar & tabs | `Session tab`, `Session tab/Inactive` | `app/shell/SessionTab.tsx` | `SessionTab` | |
| Top bar & tabs | `Tab menu` | `app/shell/TabMenu.tsx` | `TabMenu` | |
| Thread | `Load earlier`, `Load earlier/Failed` | `app/conversation/LoadEarlier.tsx` | `LoadEarlier` | |
| Thread | `User attachments` | `app/conversation/UserAttachments.tsx` | `UserAttachments` | |
| Thread | `Archived bar` | `app/conversation/AboveComposer.tsx` | `AboveComposer` | draws `Archived bar` only |
| Thread | `Archived bar` | `app/conversation/ArchivedBar.tsx` | `ArchivedBar` | |
| Thread cards | `Question card` | `app/conversation/ChoiceCard.tsx` | `ChoiceCard` | |
| Thread cards | `Action card/Question` | `app/conversation/QuestionAction.tsx` | `QuestionAction` | |
| Thread cards | `Action card/Plan` | — | — | Pen only: the code was removed in cb94ccca9 |
| Thread cards | `Phase plan action` | — | — | Pen only: the code was removed in cb94ccca9 |
| Thread cards | `Changes file row` | `app/conversation/ChangesFileRow.tsx` | `ChangesFileRow` | |
| Thread cards | `Merge warning row` | — | — | Pen only: the code was removed in b1014e45c |
| Thread cards | `Revert confirm row` | `app/conversation/RevertConfirmRow.tsx` | `RevertConfirmRow` | |
| Thread cards | `Open pill` | `app/conversation/activity/OpenPill.tsx` | `OpenPill` | |
| Thread cards | `Proofs` | `app/conversation/activity/Proofs.tsx` | `Proofs` | |
| Thread cards | `Plan frame` | — | — | Pen only: the code was removed in cb94ccca9 |
| Thread cards | `Plan clip` | — | — | Pen only: the code was removed in cb94ccca9 |
| Thread cards | `Plan text` | — | — | Pen only: the code was removed in cb94ccca9 |
| Steps & tool rows | `Update text` | `app/conversation/UpdateText.tsx` | `UpdateText` | parts: `ReplyText`, `ReportText` (+ `AnswerDetails`), `DetailsFold`, `PhaseOutcome`, each in its own file |
| Steps & tool rows | `Steer bubble` | `app/conversation/SteerBubble.tsx` | `SteerBubble` | |
| Steps & tool rows | `Block error` | `app/conversation/BlockError.tsx` | `BlockError` | |
| Steps & tool rows | `Joined answer` | `app/conversation/JoinedAnswer.tsx` | `JoinedAnswer` | |
| Steps & tool rows | `Model changed` | `app/conversation/ModelChanged.tsx` | `ModelChanged` | |
| Steps & tool rows | `Answer actions/Chat`, `Answer actions/Session`, `Answer actions/Joined` | `app/conversation/AnswerActions.tsx` | `AnswerActions` | parts: `JoinedAnswerActions`, `ReadAloud`, each in its own file |
| Steps & tool rows | `Continued from`, `Continued from/Gone` | `app/conversation/ContinuedFrom.tsx` | `ContinuedFrom` | |
| Steps & tool rows | `Work header` | `app/conversation/WorkHeader.tsx` | `WorkHeader` | |
| Steps & tool rows | `Compaction block` | `app/conversation/CompactionBlock.tsx` | `CompactionBlock` | |
| Steps & tool rows | `Activity items` | `app/conversation/ActivityItems.tsx` | `ActivityItems` | |
| Steps & tool rows | `Repeats row` | `app/conversation/activity/RepeatsRow.tsx` | `RepeatsRow` | |
| Steps & tool rows | `Activity group` | `app/conversation/activity/GroupStepList.tsx` | `GroupStepList` | shown inside it |
| Steps & tool rows | `File link` | `app/conversation/activity/FileLink.tsx` | `FileLink` | |
| Steps & tool rows | `File counts` | `app/conversation/activity/FileCounts.tsx` | `FileCounts` | |
| Steps & tool rows | `Machine row` | `app/conversation/activity/MachineRow.tsx` | `MachineRow` | |
| Steps & tool rows | `Decided row` | `app/conversation/activity/DecidedRow.tsx` | `DecidedRow` | |
| Steps & tool rows | `Merged row` | `app/conversation/activity/MergedRow.tsx` | `MergedRow` | |
| Steps & tool rows | `Peer link` | `app/conversation/activity/PeerLink.tsx` | `PeerLink` | |
| Steps & tool rows | `Inline diff` | `app/conversation/activity/Diffs.tsx` | `Diffs` | a stack of `Inline diff` |
| Steps & tool rows | `Lead detail` | — | — | Pen only: the code was removed in 19f382faa |
| Steps & tool rows | `Lead detail` | `app/conversation/activity/LeadItemDetail.tsx` | `LeadItemDetail` | look of `Lead detail` |
| Steps & tool rows | `Team glyphs` | `app/conversation/activity/Glyphs.tsx` | `Glyphs` | |
| Steps & tool rows | `Briefs` | `app/conversation/activity/Briefs.tsx` | `Briefs` | |
| Steps & tool rows | `Quiet` | `app/conversation/activity/QuietBrief.tsx` | `QuietBrief` | |
| Steps & tool rows | `Turn end` | `app/conversation/TurnEnd.tsx` | `TurnEnd` | |
| Composer | `Composer card` | `app/conversation/ComposerCard.tsx` | `ComposerCard` | |
| Composer | `Composer input` | `app/conversation/ComposerInput.tsx` | `ComposerInput` | |
| Composer | `Context ring` | `app/conversation/ComposerContextRing.tsx` | `ComposerContextRing` | |
| Composer | `Full access notice` | `app/conversation/FullAccessNotice.tsx` | `FullAccessNotice` | |
| Composer | `Utility bar` | `app/conversation/UtilityBar.tsx` | `UtilityBar` | |
| Composer | `Send controls` | `app/conversation/SendControls.tsx` | `SendControls` | the button itself is `SendButton` (Primitives · Controls) |
| Composer | `Dictation note` | `app/conversation/DictationNote.tsx` | `DictationNote` | |
| Composer | `Composer footer` | `app/conversation/ComposerFooter.tsx` | `ComposerFooter` | |
| Composer | `Dictation bar` | `app/conversation/DictationBar.tsx` | `DictationBar` | |
| Composer | `Waveform` | `app/conversation/DictationWaveform.tsx` | `DictationWaveform` | |
| Composer | `Composer hint` | `app/conversation/ComposerHint.tsx` | `ComposerHint` | |
| Composer menus, rail & queue | `Capsule pill` | `app/conversation/CapsulePill.tsx` | `CapsulePill` | |
| Composer menus, rail & queue | `Step list` | — | — | Pen only: the code was removed in cb94ccca9 |
| Composer menus, rail & queue | `Run capsule` | — | — | Pen only: the code was removed in cb94ccca9 |
| Composer menus, rail & queue | `Capsule` | `app/conversation/Capsule.tsx` | `Capsule` | |
| Composer menus, rail & queue | `Request capsule` | `app/conversation/RequestCapsule.tsx` | `RequestCapsule` | |
| Composer menus, rail & queue | `Diff pill` | `app/conversation/DiffPill.tsx` | `DiffPill` | |
| Composer menus, rail & queue | `Moved notice` | — | — | Pen only: the code was removed in efa933d5a |
| Composer menus, rail & queue | `Rail pill` | `app/conversation/RailPill.tsx` | `RailPill` | |
| Composer menus, rail & queue | `Stand-in pill` | `app/shell/StandInPill.tsx` | `StandInPill` | |
| Composer menus, rail & queue | `Waiting pill` | `app/shell/WaitingPill.tsx` | `WaitingPill` | |
| Composer menus, rail & queue | `Status row` | `app/conversation/StatusCardRow.tsx` | `StatusCardRow` | |
| Composer menus, rail & queue | `Limit bar` | `app/conversation/LimitBar.tsx` | `LimitBar` | |
| Composer menus, rail & queue | `Strip row` | `app/conversation/activity/StripRow.tsx` | `StripRow` | |
| Composer menus, rail & queue | `Picker row` | `app/conversation/PickerRow.tsx` | `PickerRow` | |
| Composer menus, rail & queue | `Usage remaining` | `app/conversation/UsageRemaining.tsx` | `UsageRemaining` | |
| Summary pane | `Summary content` | `app/conversation/SummaryContent.tsx` | `SummaryContent` | |
| Summary pane | `Floating summary` | `app/conversation/FloatingSummary.tsx` | `FloatingSummary` | |
| Summary pane | `Source row` | `app/conversation/SourceRow.tsx` | `SourceRow` | |
| Summary pane | `Branch row` | `app/conversation/BranchRow.tsx` | `BranchRow` | |
| Summary pane | `JobStack` | — | — | Pen only: the code was removed in cb94ccca9 |
| Summary pane | `Links popover` | `app/conversation/AllLinksPopover.tsx` | `AllLinksPopover` | |
| Files, search & source | `View tabs` | `app/side-panel/PaneViewTabs.tsx` | `PaneViewTabs` | |
| Files, search & source | `File browser` | `app/side-panel/FileBrowser.tsx` | `FileBrowser` | |
| Files, search & source | `Source header` | `app/side-panel/SourceHeader.tsx` | `SourceHeader` | |
| Files, search & source | `Source group` | `app/side-panel/SourceGroup.tsx` | `SourceGroup` | |
| Files, search & source | `Commit box` | `app/side-panel/CommitBox.tsx` | `CommitBox` | |
| Files, search & source | `Commit actions menu` | `app/side-panel/CommitActionsMenu.tsx` | `CommitActionsMenu` | |
| Files, search & source | `More actions menu` | `app/side-panel/SourceMoreMenu.tsx` | `SourceMoreMenu` | |
| Changes, review & conflicts | `Rebase step` | `app/side-panel/RebaseStep.tsx` | `RebaseStep` | |
| Changes, review & conflicts | `Review steps/Changes`, `Review steps/Files` | `app/side-panel/ReviewSteps.tsx` | `ReviewSteps` | |
| Changes, review & conflicts | `Open in menu` | `app/side-panel/OpenInMenu.tsx` | `OpenInMenu` | |
| Changes, review & conflicts | `Find box` | `app/side-panel/FindLineBox.tsx` | `FindLineBox` | |
| Workers | `Agent row` | `app/side-panel/AgentRow.tsx` | `AgentRow` | |
| Workers | `Worker list` | `app/side-panel/WorkerList.tsx` | `WorkerList` | |
| Workers | `Worker header` | `app/side-panel/WorkerHeader.tsx` | `WorkerHeader` | |
| Workers | `Output file row` | `app/side-panel/WorkerFileRow.tsx` | `WorkerFileRow` | |
| Workers | `Entry view/Received`, `Entry view/Reply`, `Entry view/Approval`, `Entry view/Notice` | `app/side-panel/EntryView.tsx` | `EntryView` | |
| Workers | `Brief` | `app/side-panel/WorkerBrief.tsx` | `WorkerBrief` | |
| Workers | `Worker answer` | — | — | Pen only: the code was removed in fb6ff046b |
| Page | `Show pages dialog` | `app/side-panel/pages/ShowPages.tsx` | `ShowPages` | |
| Page | `Page file menu` | `app/side-panel/pages/PageFileMenu.tsx` | `PageFileMenu` | |
| Page | `Page insert menu` | `app/side-panel/pages/PageInsertMenu.tsx` | `PageInsertMenu` | |
| Job, mockup & new tab | `Images menu` | `app/side-panel/ImagesMenu.tsx` | `ImagesMenu` | |
| Job, mockup & new tab | `Notes list` | `app/side-panel/NotesList.tsx` | `NotesList` | |
| Output, plan & side chat | `Run output` | — | — | Pen only: the code was removed in 71f10dc45 |
| Browser | `Browser nav` | `app/side-panel/NavigationCapsule.tsx` | `NavigationCapsule` | |
| Browser | `Site info button/Not secure` | `app/side-panel/SiteInfo.tsx` | `SiteInfo` | secure and local are looks of `Icon Button` |
| Browser | `Site info` | `app/side-panel/SiteInfoPopover.tsx` | `SiteInfoPopover` | |
| Browser | `Find bar` | `app/side-panel/FindBar.tsx` | `FindBar` | |
| Browser | `Browser options` | `app/side-panel/BrowserOptions.tsx` | `BrowserOptions` | |
| Onboarding & setup | `Setup step/Pending`, `Setup step/Current`, `Setup step/Done` | `app/setup/computerSetup/StepRailItem.tsx` | `StepRailItem` | |
| Onboarding & setup | `Setup panel` | `app/setup/computerSetup/SetupPanel.tsx` | `SetupPanel` | |
| Onboarding & setup | `Permission step/Accessibility`, `Permission step/Screen Recording` | `app/setup/computerSetup/PermissionStep.tsx` | `PermissionStep` | |
| Onboarding & setup | `Helper card` | `app/setup/computerSetup/HelperCard.tsx` | `HelperCard` | |
| Onboarding & setup | `Capture step` | `app/setup/computerSetup/CaptureStep.tsx` | `CaptureStep` | |
| Onboarding & setup | `Agent card` | `app/setup/onboarding/AgentCard.tsx` | `AgentCard` | |
| Onboarding & setup | `Onboarding/Intro`, `Onboarding/Name`, `Onboarding/Agents`, `Onboarding/Projects` | `app/setup/onboarding/Onboarding.tsx` | `Onboarding` | |
| Onboarding & setup | `Onboarding top bar` | `app/setup/onboarding/OnboardingTopBar.tsx` | `OnboardingTopBar` | |
| Onboarding & setup | `Found counter` | `app/setup/onboarding/FoundCounter.tsx` | `FoundCounter` | |
| Onboarding & setup | `Project card` | `app/setup/onboarding/ProjectCard.tsx` | `ProjectCard` | |
| Onboarding & setup | `Name form` | `app/setup/onboarding/NameForm.tsx` | `NameForm` | |
| App dialogs | `Add project dialog/Clone` | `app/shell/dialogs/CloneView.tsx` | `CloneView` | the Add project parts it shares are in `app/shell/dialogs/addProjectParts.tsx` |
| App dialogs | `Add project dialog/New repository` | `app/shell/dialogs/NewRepoView.tsx` | `NewRepoView` | |
| App dialogs | `Field` | `app/shell/dialogs/Field.tsx` | `Field` | |
| App dialogs | `Remove project dialog/Form` | `app/shell/dialogs/RemoveForm.tsx` | `RemoveForm` | its head and actions row are in `app/shell/dialogs/removeProjectParts.tsx` |
| App dialogs | `Remove project dialog/Removed` | `app/shell/dialogs/Removed.tsx` | `Removed` | |
| Settings · Window & page parts | `SettingsPage/BackLink` | `app/settings/BackLink.tsx` | `BackLink` | |
| Settings · Window & page parts | `SettingsSelectTrigger/Icon` | `app/settings/FilterMenu.tsx` | `FilterMenu` | look of `SettingsSelectTrigger/Icon` |
| Settings · General & system | `AppearanceSection` | `app/settings/AppearanceSection.tsx` | `AppearanceSection` | |
| Settings · General & system | `NameRow` | `app/settings/NameRow.tsx` | `NameRow` | |
| Settings · General & system | `ComposerSection` | `app/settings/ComposerSection.tsx` | `ComposerSection` | |
| Settings · General & system | `NotificationsSection` | `app/settings/NotificationsSection.tsx` | `NotificationsSection` | |
| Settings · General & system | `NotificationsSection` | `app/settings/DimmedRow.tsx` | `DimmedRow` | shown inside it |
| Settings · General & system | `PowerSection` | `app/settings/PowerSection.tsx` | `PowerSection` | |
| Settings · General & system | `OpenAtLoginRow` | `app/settings/OpenAtLoginRow.tsx` | `OpenAtLoginRow` | |
| Settings · General & system | `AboutSection` | `app/settings/AboutSection.tsx` | `AboutSection` | |
| Settings · General & system | `VersionRow` | `app/settings/VersionValue.tsx` | `VersionValue` | shown inside it |
| Settings · General & system | `HibernateRow` | `app/settings/HibernateRow.tsx` | `HibernateRow` | |
| Settings · General & system | `Picture/Strip`, `Picture/None` | `app/settings/SidebarClosedPicture.tsx` | `SidebarClosedPicture` | |
| Settings · Providers | `Installation` | `app/settings/providers/Installation.tsx` | `Installation` | |
| Settings · Providers | `AgentModels` | `app/settings/providers/AgentModels.tsx` | `AgentModels` | |
| Settings · Providers | `AgentModelRow` | `app/settings/providers/AgentModelRow.tsx` | `AgentModelRow` | |
| Settings · Providers | `AgentRow` | `app/settings/providers/ProviderAgentRow.tsx` | `ProviderAgentRow` | |
| Settings · Routing | `RankingView` | `app/conversation/cards/RankingView.tsx` | `RankingView` | |
| Settings · Routing | `More › kind of work row` | `app/settings/routing/KindOfWorkRow.tsx` | `KindOfWorkRow` | |
| Settings · Routing | `More › loading line` | `app/settings/routing/LoadingLine.tsx` | `LoadingLine` | |
| Settings · Routing | `More › strength row` | `app/settings/routing/StrengthRow.tsx` | `StrengthRow` | |
| Settings · Routing | `More › ratings footer` | `app/settings/routing/RatingsFooter.tsx` | `RatingsFooter` | |
| Settings · Routing | `More › steps list` | `app/settings/routing/StepsList.tsx` | `StepsList` | |
| Settings · Routing | `MoreRowHead` | `app/settings/routing/MoreRowHead.tsx` | `MoreRowHead` | |
| Settings · Routing | `KindRow` | `app/settings/routing/KindRow.tsx` | `KindRow` | |
| Settings · Routing | `NowPick` | `app/settings/routing/NowPick.tsx` | `NowPick` | |
| Settings · Routing | `WhoPicks` | `app/settings/routing/WhoPicks.tsx` | `WhoPicks` | |
| Settings · Routing | `AutoPicks` | `app/settings/routing/AutoPicks.tsx` | `AutoPicks` | |
| Settings · Routing | `OrderPlace` | `app/settings/routing/OrderPlace.tsx` | `OrderPlace` | |
| Settings · Routing | `OrderList` | `app/settings/routing/OrderList.tsx` | `OrderList` | foundations.pen `AL8pa` › Order list |
| Settings · Routing | `Rules`, `Rule row` | `app/settings/routing/Rules.tsx` | `Rules` | foundations.pen `jlV2S`, `QlVJ4` › Rule row |
| Settings · Routing | `Draft row` | `app/settings/routing/Rules.tsx` | `Rules` | shown inside it; foundations.pen `QlVJ4` › Draft row |
| Settings · Routing | `Model picker/Routing` | `app/settings/routing/pickers.tsx` | `TargetPicker`, `useTargetPanel` | foundations.pen `Kq0Ar` |
| Settings · Browser | `DownloadRow` | `app/settings/DownloadRow.tsx` | `DownloadRow` | |
| Settings · Browser | `HistoryDay` | `app/settings/DayGroup.tsx` | `DayGroup` | |
| Settings · Browser | `VisitRow` | `app/settings/VisitRow.tsx` | `VisitRow` | |
| Settings · Browser | `BrowserImportPage` | `app/settings/ImportStatusLine.tsx` | `ImportStatusLine` | shown inside it |
| Settings · Browser | `BrowserOffNote` | `app/settings/BrowserOffNote.tsx` | `BrowserOffNote` | |
| Settings · Browser | `ListingRows` | `app/settings/ListingRows.tsx` | `ListingRows` | |
| Settings · Browser | `BrowserIcon` | `app/settings/BrowserIcon.tsx` | `BrowserIcon` | |
| Settings · Browser | `BrowserRow` | `app/settings/BrowserRow.tsx` | `BrowserRow` | |
| Settings · Browser | `ImportStatus/Ready`, `ImportStatus/Attention`, `ImportStatus/Idle` | `app/settings/BrowserStatus.tsx` | `BrowserStatus` | |
| Settings · Browser | `FlowHeader` | `app/settings/FlowHeader.tsx` | `FlowHeader` | |
| Settings · Browser | `ImportBar` | `app/settings/ImportBar.tsx` | `ImportBar` | |
| Settings · Browser | `ChooseStep` | `app/settings/ChooseStep.tsx` | `ChooseStep` | |
| Settings · Browser | `ProfilePick` | `app/settings/ProfilePick.tsx` | `ProfilePick` | |
| Settings · Browser | `BrowserOffNote` | `app/settings/BrowserOffNote.tsx` | `BrowserOffNote` | |
| Settings · Browser | `LocationRow` | `app/settings/LocationRow.tsx` | `LocationRow` | |
| Settings · Browser | `BrowserSwitch` | `app/settings/BrowserSwitch.tsx` | `BrowserSwitch` | |
| Settings · Browser | `LinkRow` | `app/settings/LinkRow.tsx` | `LinkRow` | |
| Settings · Browser | `SiteRow` | `app/settings/SiteRow.tsx` | `SiteRow` | |
| Settings · Browser | `ClearForm` | `app/settings/ClearForm.tsx` | `ClearForm` | |
| Settings · Browser | `ClearTickRow` | `app/settings/TickRow.tsx` | `TickRow` | |
| Settings · Storage & archived chats | `ProjectRow`, `ProjectRow/Compact` | `app/settings/storage/ProjectRows.tsx` | `ProjectRows` | |
| Settings · Storage & archived chats | `ExpandableRow` | `app/settings/storage/ExpandableRow.tsx` | `ExpandableRow` | |
| Settings · Storage & archived chats | `DetailRow` | `app/settings/storage/DetailRow.tsx` | `DetailRow` | |
| Settings · Storage & archived chats | `Size` | `app/settings/storage/StorageSize.tsx` | `StorageSize` | |
| Settings · Storage & archived chats | `UnusedRow` | `app/settings/storage/UnusedRow.tsx` | `UnusedRow` | |
| Settings · Storage & archived chats | `UsageCard` | `app/settings/storage/UsageCard.tsx` | `UsageCard` | |
| Settings · Inspector · Events & orchestrator | `SessionBar` | `app/inspector/SessionBar.tsx` | `SessionBar` | |
| Settings · Inspector · Events & orchestrator | `ListSwitchBar` | `app/inspector/ListSwitchBar.tsx` | `ListSwitchBar` | |
| Settings · Inspector · Events & orchestrator | `GenerationHeader` | `app/inspector/GenerationHeader.tsx` | `GenerationHeader` | |
| Settings · Inspector · Events & orchestrator | `BlobText` | `app/inspector/BlobText.tsx` | `BlobText` | |
| Settings · Inspector · Events & orchestrator | `RebirthDetail` | `app/inspector/RebirthDetail.tsx` | `RebirthDetail` | |
| Settings · Inspector · Events & orchestrator | `BriefingSectionsTable` | `app/inspector/BriefingSectionsTable.tsx` | `BriefingSectionsTable` | |
| Settings · Inspector · Events & orchestrator | `RebirthRowView` | `app/inspector/RebirthRowView.tsx` | `RebirthRowView` | |
| Settings · Inspector · Brain | `Detail`, `Detail/Wrap`, `Detail/Busy` | `app/inspector/brain/BrainDetail.tsx` | `BrainDetail` | |
| Settings · Inspector · Brain | `Progress` | `app/inspector/brain/BrainProgress.tsx` | `BrainProgress` | |
| Settings · Inspector · Brain | `KindCounts` | `app/inspector/brain/NodesByKindList.tsx` | `NodesByKindList` | |
| Settings · Inspector · Brain | `IndexSection` | `app/inspector/brain/IndexState.tsx` | `IndexState` | shown inside it |
| Settings · Inspector · Brain | `IndexSection` | `app/inspector/brain/IndexSection.tsx` | `IndexSection` | |
| Settings · Inspector · Brain | `EmbedderSection` | `app/inspector/brain/EmbedderState.tsx` | `EmbedderState` | shown inside it |
| Settings · Inspector · Brain | `EmbedderSection` | `app/inspector/brain/EmbedderSection.tsx` | `EmbedderSection` | |
| Settings · Inspector · Brain | `WorkersSection` | `app/inspector/brain/WorkersSection.tsx` | `WorkersSection` | |
| Settings · Inspector · Brain | `JobRow` | — | — | Pen only: the code was removed in a041169da |
| Settings · Inspector · Brain | `JobsSection` | — | — | Pen only: the code was removed in a041169da |
| Settings · Inspector · Brain | `DeveloperActions` | `app/inspector/brain/DeveloperActions.tsx` | `DeveloperActions` | |
| Settings · Inspector · Brain | `BrainQueryBar` | `app/inspector/brain/QueryBar.tsx` | `QueryBar` | |
| Settings · Inspector · Brain | `ResultSummary` | `app/inspector/brain/ResultSummary.tsx` | `ResultSummary` | |
| Settings · Inspector · Brain | `HitRow` | `app/inspector/brain/HitRow.tsx` | `HitRow` | |
| Settings · Inspector · Brain | `OrchestratorReads` | `app/inspector/brain/OrchestratorReadsDisclosure.tsx` | `OrchestratorReadsDisclosure` | |
| Settings · Inspector · Brain | `BrainToolbar` | `app/inspector/brain/BrainToolbar.tsx` | `BrainToolbar` | |
| Settings · Inspector · Brain | `ConversationName` | `app/inspector/brain/ConversationName.tsx` | `ConversationName` | |
| Settings · Inspector · Brain | `NodeLink` | `app/inspector/brain/NodeLink.tsx` | `NodeLink` | |
| Settings · Inspector · Brain | — | `app/inspector/brain/BrainSection.tsx` | `BrainSection` | shown inside `IndexSection`, `WorkersSection` and `EmbedderSection` |
| Settings · Inspector · Processes & providers | `StartSessionForm` | `app/inspector/providers/StartSessionForm.tsx` | `StartSessionForm` | |
| Settings · Inspector · Processes & providers | `RecordToggle/Off`, `RecordToggle/On` | `app/inspector/providers/RecordToggle.tsx` | `RecordToggle` | |
| Settings · Inspector · Processes & providers | `SessionList` | `app/inspector/providers/SessionList.tsx` | `SessionList` | |
| Settings · Inspector · Processes & providers | `RawSessionRow` | `app/inspector/providers/SessionRow.tsx` | `SessionRow` | |
| Settings · Inspector · Processes & providers | `FixtureList` | `app/inspector/providers/FixtureList.tsx` | `FixtureList` | |
| Settings · Inspector · Processes & providers | `FixtureRow` | `app/inspector/providers/FixtureRow.tsx` | `FixtureRow` | |
| Settings · Inspector · Processes & providers | `ApprovalsTray` | `app/inspector/providers/ApprovalsTray.tsx` | `ApprovalsTray` | |
| Settings · Inspector · Processes & providers | `SessionHeader` | `app/inspector/providers/SessionHeader.tsx` | `SessionHeader` | |
| Settings · Inspector · Processes & providers | `TranscriptList` | `app/inspector/providers/TranscriptList.tsx` | `TranscriptList` | |
| Settings · Inspector · Processes & providers | `ApprovalCard/Raw` | `app/inspector/providers/RawApprovalCard.tsx` | `RawApprovalCard` | |
| Settings · Inspector · Processes & providers | `RawComposer` | `app/inspector/providers/RawSessionComposer.tsx` | `RawSessionComposer` | |
| Steps & tool rows | `Tool result › Raw link`, `Tool box` | `app/conversation/activity/RawToolCallDialog.tsx` | `RawToolCallDialog` | code only: the Raw link on `Tool result` opens it, a plain dialog holding two `Tool box` parts (Input, Output) |
| Composer menus, rail & queue | `Rail item/Card` | `app/conversation/Notices.tsx` | `Notices` | look of `Rail item/Card` (the tray); its notice rows are code only |
| — | `TabPage` | `app/side-panel/TabPage.tsx` | `TabPage` | code only (layout, no look of its own) |

## Inline visuals

Top-level Chats draw `brigadier-table`, `brigadier-chart` and `brigadier-html` fences inline.
Sessions, workers, side chats and tab chats keep the code card. Tables reuse the Markdown
table's scrolling, copy and expand controls. Charts use the app's colour tokens, a bounded
container and a text description of the values. Invalid data shows a plain error; open table
and chart fences show “Drawing…”.

The shell seals network APIs and native IPC lookups before model scripts run. The native
startup hook applies the same restrictions to child realms; the shell CSP denies child loads.
On macOS a WebKit content rule blocks network resources and connection hints from visual
frames. The frame response waits for that rule to be installed, and fails closed if it cannot
be installed. The parent frame policy is also applied when developing through Vite.

HTML previews update at most once a second with scripts removed. A closed fence gets a fresh,
offline frame with the app's text, surface, border, spacing and control tokens. The frame can
resize from 50 to 4000 pixels. A follow-up click puts text in the composer for the user to send;
it is ignored while a turn runs. HTTPS links follow the app's existing link path.

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
  it. Every component lives in `foundations.pen`, on its area's board; `app.pen` and
  `settings.pen` only place instances of it.
- Agents editing through the MCP: pass the file's full real path, and make sure the file is open
  in Pen, or the call silently works on whichever file is active. Never `Replace` a child of an
  instance of a reusable component (insert the new part, move it, delete the old one). After a
  failed edit, close the file without saving and reopen it.
- A new foundations component shows up in `app.pen` or `settings.pen` only after that file's tab
  is closed and opened again, or Pen is restarted (save every file first).
- Pen (1.2.19 on) opens every file as a tab of one window. Save acts on the front tab: bring the
  file's tab to the front (File › Open Recent › the file switches to it), check the window title
  names that file, then ⌘S, and check the file's modified time changed. A newly started MCP
  server may reach only the front tab, so switch first there too. No backup copies.

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
