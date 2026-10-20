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

- Every `app.pen` and `settings.pen` board is titled "Surface · Topic" and reads top to bottom:
  ① Screens, ② Dialogs, ③ Components. Cards are titled "#NN Name" (the number from the
  components inventory) and show the component's code path.
- Boards sit in rows, not columns, left to right in app order, with a label above each group.
  A board is at most 8,400 × 5,000 and always landscape; when a surface needs more, it gets
  another board beside it.

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
- `Icon / <Name>` components are the app's real icons: the names match the UI kit's `Icon`
  component and our own glyphs in `components/glyphs`. A few spots keep a library icon because
  the app has none there; the Read me lists them.
- Screens show what ships, not ideas. 1440×900, dark, realistic sample content.
- Dark only. Normal density only: there is no compact variant.
- Buttons, chips and badges are fully rounded; icon buttons keep a 10px corner; the composer's
  send button is a circle. There is one checkbox: blue (`--toggle-on`) with a white tick.
- Plain words. No names or marks of other chat apps or of the UI kit's vendor.

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
