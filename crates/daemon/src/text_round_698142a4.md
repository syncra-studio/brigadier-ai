The worker is still on the composer jump. Below is round 1 on Plugins. I read the current page, its details page and the Library plan (`docs/STRIP-DOWN-PLAN.md` §9.4) first, so these are only the choices that are yours to make.

❓ **Q1 - What "make them available for every agent" does**: Brigadier already has a way to do this per item, "Add to every agent". It moves the item into Brigadier's Library (`~/.brigadier/library/`) and installs it into each CLI through that CLI's own plugin commands. The original copy is removed only once every CLI lists the new one, and the move can be undone. The automatic version would run that same move on every item, in the background, without asking.

➡️ Yes, use that same move. Run it on first launch, and again on every app start so items added later in a terminal get picked up. Existing users get it once on their next update.

---

❓ **Q2 - Items that can't or shouldn't move**: Three kinds exist:
- (a) Skills that came inside one CLI's own plugin, which the other CLIs can't add.
- (b) Skills synced from claude.ai and Codex's built-in system skills. The plan says never touch these.
- (c) Servers set up only in another app, such as Claude Desktop. These sit in "Found in other apps" today.

➡️ Hide (b) completely. Show (a) like any other item. Leave (c) out of the automatic move, and replace its section with an "Import from Claude Desktop" choice in Add. A server set up for another app may not start in a CLI.

---

❓ **Q3 - Turning an item off for one agent**: With the Agents section and the agent switches gone, there's no way to keep an item away from just Codex or just Claude. It's on for every agent or for none.

➡️ Accept that: every agent or none.

---

❓ **Q4 - What the row's switch does**: Without a project picker, the switch can't mean "on in this project". Options:
- (a) An on/off for the whole item that keeps its project choice, so turning it back on restores the same projects.
- (b) Off means it's removed from every project.

➡️ (a).

---

❓ **Q5 - The chip**: You said "the project it was found in". After someone changes where an item is on (on its details page), should the chip show where it was found or where it's on now? And what does an item on in 3 projects show?

➡️ Where it's on now: "Global", the project's name, or "3 projects". Showing where it was found would go stale after the first change.

---

❓ **Q6 - Problems on a row**: Some items need a sign-in, won't start, or have an update. You want nothing on the row beyond icon, title, subtitle, chip and switch.

➡️ The subtitle turns into the problem in warning colour, for example "Needs sign-in" or "Won't start". The details page keeps its banner with the Sign in, Try again or Update button. The list puts items with a problem first, then everything else alphabetically.

---

❓ **Q7 - Subtitle text**:

➡️ The item's description. If it has none, its command, address or folder.

---

❓ **Q8 - Icons**: No icons exist today, just a generic glyph per kind. Real ones are only possible for:
- servers with a web address: the site's favicon;
- a short bundled list of well-known servers (GitHub, Figma, Linear and similar).

Skills never have icons.

➡️ Favicon for servers with an address, the bundled list for well-known ones, and otherwise two new illustrations, one for servers and one for skills.

---

❓ **Q9 - Design first?**: Your Plugins board is settings.pen `kDJe3` and its parts are in foundations.pen `Y8odgz`.

➡️ A worker redraws the list, the details page and both illustrations in those .pen boards. You approve them, then the real build starts.

---

❓ **Q10 - Details page: where it's on**: Today there's a Global switch plus a switch per project, so you can also turn an item off in one project while it's global.

➡️ Two choices, "All projects" and "Only some projects". The second shows the project switches. Drop "off in one project while global".

---

❓ **Q11 - Details page: the open/edit row**: Should it open the item's file in your default editor (the folder's SKILL.md for a skill), reveal it in Finder, or open it inside Brigadier?

➡️ Open in your default editor, with a small "Show in Finder" beside it.

---

❓ **Q12 - Things you didn't mention**: Keep the "Add" button in the page header? On the details page, keep the sign-in, won't-start and update banners at the top? Drop the "Add to every agent" banner, since that now happens automatically? For servers that sign in, does the "Sign in again" row in Advanced go?

➡️ Keep Add. Keep the three banners. Drop the "Add to every agent" banner. Remove "Sign in again" from Advanced; the sign-in banner covers it.

---

A quick "yes to all" works, or answer just the ones you'd change.
