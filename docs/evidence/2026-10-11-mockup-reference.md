# The mockup tab and the reference app's image tab (2026-10-11)

SESSION-FIXES-PLAN.md Q10 shows a mockup in a tab of the right sidebar and asks it to look and behave like the
reference app's preview tab. Its image tab was studied on 2026-10-11: its web bundle read, and a separate instance on a
throwaway profile driven over CDP (window 1728x1018, light theme forced in the renderer; nothing was sent, the profile
was deleted). **[L]** is measured live, **[C]** read from code only.

## What was measured, and what the mockup tab does

| The reference app | The mockup tab |
|---|---|
| Previews open as a content tab of the side panel; split beside Chat or full view (Chat becomes a tab) [L] | The Mockup tab of the right sidebar (shown while the session has a mockup); "Enter full view" opens it as a session tab over the main area, "Exit full view" returns it to the sidebar |
| Header 48px over the stage; floating 32px capsules (blurred, ringed, round), gap 6px [L] | Same: `h-mockup-header`, `h-mockup-control`, `shadow-mockup-control`, `rounded-capsule`, `gap-1.5` |
| File-type pill, its menu listing the file [L] | The pill names the image shown and the mockup's version and state; its menu lists every screenshot (mockup, live, side by side) |
| Zoom capsule "73%": 25 / 50 / 100 / 150 / 200, separator, "Zoom to fit" (default; never enlarges); pinch and ⌘-scroll, 10–400% [L, C] | Same menu, steps and default; ⌘/pinch zoom with the same factor and clamp; a zoomed image pans by drag (grab cursor) |
| "Focused view" / "Canvas view" toggle at top-left; canvas: a row per turn, tiles at most 292px high, 8px apart, ringed blue when commented [C] | Same toggle; a row per screen: mockup, live capture, side by side; 292px tiles (`max-h` via `--spacing-mockup-tile`), blue ring when the screen has notes; a click focuses the image. A compared mockup opens on Canvas view |
| Image on the stage: hairline border and soft shadow, centred, no padding [L] | `shadow-mockup-image`, centred under the header |
| Bottom toolbar, dark, 40px, radius 15px, 16px above the edge: Markup · Comment · Remove BG · Erase · Resize [L] | Same bar with Comment only: the other tools edit pixels, which a mockup doesn't need |
| Comment mode: "Click on the image to add comments" \| Send (white pill, disabled until a comment) \| × [L] | Same words and controls; "{n} comments" once there are notes, hovering it lists them as "Image 1" + "1. …" [L] |
| A click (8px slop) drops a 30px numbered teardrop marker (radius 50% 50% 50% 3px, blue, white ring, 11px bold) [L] | Same (`rounded-marker`, `size-mockup-marker`, `bg-marker`, `shadow-marker`); a ghost marker follows the pointer, the cursor hidden |
| Note input: 294x44 pill, "Add a comment…", round send arrow once there is text; Enter adds, Escape drops; beside the marker (right at +27px, else left, else below, else above, 8px inside) [L] | Same sizes, words, keys and placement (`notePlacement`, tested) |
| Clicking a marker: 294x120 edit card, Delete / Cancel / Save, ⌘↩ saves [L] | Same |
| Notes go into the composer; the sent text is `Image 1:` then `1. (x: 30%, y: 40%) note` [C] | Sent at once as one message by Send or ⌘↩: the same `Image N` blocks and points, each block naming the attached `mockup-N-notes.png`, which has the markers drawn on it so the worker sees where each note points |
| After sending: "Working"; then the blue "See latest changes \| ×" pill [C] | Same: Working until the worker reports again, then See latest changes (moves to the revision) or × (keeps the version shown until the tab next opens) |
| "Accept all" / "Undo" / "{n} changes" | Not built: in the studied build they are hard-coded disabled with no caller, an unshipped feature |
| Loading "Loading image…"; error "Couldn’t load this preview" with Retry [C] | Same words and Retry |
| Motion: marker scale 1.05 over 150ms, note box 150ms, all with reduced-motion opt-outs [L, C] | Same durations with `ease-enter`, `motion-reduce` |

## Differences kept on purpose

- **Notes don't pass through the composer.** The tab sends them itself, so a note never mixes with a half-typed
  message and the thread gets one batch it can forward whole (the thread's MOCKUPS instruction). No "Additional
  instructions" block.
- **The sent screenshots carry their markers.** The reference app attaches the original image and relies on the
  percentages; a worker finds the place faster from a drawn marker.
- **No Open / Download / Move toolbar.** The images are a worker's outputs, also in its report; the toolbar stays put.
- **Split view is the right sidebar's width** (at most 29.25rem), not the reference app's resizable half: full view is
  one click away.
- **Dark only,** in Brigadier's tokens: the stage is `--mockup-stage`, the controls `bg-popover`.
