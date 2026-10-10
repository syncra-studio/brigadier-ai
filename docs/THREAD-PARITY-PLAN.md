# Thread parity plan: a conversation that reads like the target, interview cards, clear endings

> Status: approved 2026-10-09 with the user's answers and the plan review folded in (§8). Branch `thread-parity`
> from `main` at `a9f6924d`.
> It builds on docs/THREAD-UX-PLAN.md (approved 2026-10-08, built). Where the two differ, this plan wins; each
> such change is marked **(changes THREAD-UX-PLAN §n)**. Two of the user's 2026-10-09 decisions supersede older
> ones (§8.2): the merge card replaces THREAD-PLAN §8 decision 6 ("Merging is conversational") for sessions, and
> the plan card replaces PLAN §10.2's plan-only-in-the-context-card rule for a session's proposed plan.
> Paths: `C/` = `apps/desktop/src/app/conversation/`, `E/` = `apps/desktop/src/components/assistant-ui/elements/`,
> `M/` = `crates/core/src/manager/`. File:line references are to `a9f6924d`.
> "Target behaviour" is the measured behaviour of the conversation view the user holds up as the bar. It was measured
> live (DevTools on a real agent thread) and from its code. The numbers below are computed CSS px in the dark theme.

## 1. What the user asked for

The user ran a session in the brigadier-ai project, asked it to grill them about a design, and found these problems:

1. **A follow-up looked like it broke the turn.** A message sent while the thread worked joined the running request,
   but the turn then ended on a plain-text question. So the user saw "Worked for 2m 15s", then their bubble, then a
   new answer.
2. **The questions came as chat text, one per reply.** That made nine questions and nine work blocks, not cards and
   not one block.
3. **Chevrons are not vertically centred** on some rows.
4. **Bare "Thought" rows** say nothing about what is going on.
5. **Content after a work block flickers** when the block collapses.
6. **The ending was confusing.**
   - "Waiting for you" stayed on.
   - A worker-made "Waiting on you" to-do list had Done buttons.
   - A long report and the same merge question came twice.
7. **The workers rail's chevron** above the composer points the wrong way.
8. **Plans** (addendum) should be written, shown and proposed the way the target does it.

The grill settled Q1–Q9 (goal.md, "Grill outcome"). §5 maps every one to a change.

## 2. Target behaviour, measured

Evidence: 27 screenshots and the full numbers in the run dir, `msgs/evidence/target/` (`findings.md`). Brigadier's
own screenshots are in `msgs/evidence/brigadier/`.

### 2.1 Global
- **Column:** max 768 px with 16 px padding, so content is 736 px wide.
- **Fonts:** the system font at weight 430. Prose (answers, bubbles) is 14 / 22.75 px. Rows and headers are 14 / 21 px.
- **Gap:** one 16 px gap between every item of a turn.
- **User bubble:**
  - `rgba(50,50,50,.85)`, radius 22 px, padding 10 × 16 px, max 70 % wide.
  - Copy and Edit show on hover only, 26 px tall.

### 2.2 The work block
- **Live header:** `Working`, then `Working for 12s`, ticking every second.
  - 14 / 21 px, tabular numbers, `rgba(255,255,255,.498)`.
  - No chevron and no shimmer. An 8 px gap, then a 1 px rule `rgba(255,255,255,.082)`.
  - The timer keeps running while a question card waits.
- **Done header:** `Worked for 19s ›`.
  - A ghost button with a 4 px gap. The text turns white on hover; there is no background.
  - Chevron: 14 px, centred on the text, pointing right. It turns 90° in 150 ms `cubic-bezier(.4,0,.2,1)`.
- **Folding:**
  - The block folds by default once done.
  - It folds the moment the final answer starts to stream.
  - The final answer sits outside the block. Messages the user steered in stay visible under the folded header.
- **Opening and closing:**
  - The body is mounted on open and unmounted after close.
  - It animates height (0 ↔ measured), opacity, and an 8 px slide (`translateY(-8px)`).
  - Open takes 300 ms `cubic-bezier(.19,1,.22,1)`; close takes about 160 ms.
  - What follows the block moves with the animated height, frame by frame. Nothing jumps or re-mounts.

### 2.3 Action rows
- **Row:** 21 px tall, content-width (not full-width), inner gap 6 px.
  - Icon: 16 px at `rgba(255,255,255,.6)`, centred on the row's centre line.
  - Text: `.6` white, truncated.
  - Two-tone rows: the whole row is `.6`; the file it acts on is a step fainter, about `.47` (measured on the
    2× screenshots). The `/90` against `/40` split is a row against a summary line, not verb against object.
- **Chevron:**
  - 14 px, 4 px after the text, centred on the same line (`items-center`).
  - Hidden until hover, then shown at once; the text turns white on hover.
  - Open: turned 90°, 300 ms.
- **Live words** (shimmering): `Running {cmd}`, `Searching for {q} in {path}`, `Listing files`, `Editing {file}`,
  `Asking questions`, `Waiting for your answer`, `Writing plan`.
- **Done words:** `Ran {cmd}` (`in 11s` when slow), `Searched for X in .`, `Edited math.js +5 −1` (green/red),
  `Asked 3 questions`.
- **A group** is one row:
  - Comma-joined kinds with the first capitalised and no counts: "Edited a file, read files, ran commands".
  - A group ends at commentary or at a steered bubble.
  - Open, it scrolls at 224 px with a 24 px edge fade; each row is 21 px with 4 px between rows.
- **A command's detail ("Shell" box):**
  - Box: radius 12.5 px, 1 px border `rgba(255,255,255,.157)`, fill `rgba(255,255,255,.08)`.
  - `$ cmd` in mono 13 / 19.5, at most 2 lines.
  - Output: mono 13 / 19.5 at `.7`, max 144 px, pinned to its newest line, with a 32 px fade.
  - Footer: `✓ Success`, right-aligned.

### 2.4 Thinking
- **Live:** a single "Thinking" line at the bottom of the running block.
  - Shimmer: base `rgba(255,255,255,.385)`, highlight `#ffffffbf`.
  - The sweep takes 1 s; the first starts after 600 ms, then one every 4 s.
  - Label, in order: "Waiting for your answer", else the newest thought's **heading**, else "Thinking".
  - It goes when the next item arrives and comes back between tool calls.
- **After:**
  - A thought with a summary is a row: "Thought for 4s", or "Thought" when it was short.
  - It opens to the summary at `/60` in a 140 px scroller.
  - A thought with no summary shows nothing at all. In practice almost no thought rows appear: the work rows tell
    the story.

### 2.5 A message sent while the agent works (steer)
- It shows at once as a normal bubble inside the running turn, and the same block and header continue below it.
- Once done: folded header, then the bubble, then the answer. Unfolded, the bubble sits between the rows where it was
  sent.

### 2.6 Question cards
- **The card takes the composer's place.**
  - Radius 25 px, 1 px border `.082`, fill `rgb(45,45,45)`, as wide as the column.
  - The thread shows `? Asking questions` and a shimmering `Waiting for your answer`.
- **Header:**
  - The question: 14 / 20 px, weight 500, white.
  - Pager: `‹ 1 of 3 ›`, 12 / 16 px at `.498`, with 24 px buttons.
  - A 26 px ✕.
- **One question shows at a time.**
- **Options:**
  - Each is a radio: min 32 px tall, padding 6 × 8 px, radius 15 px.
  - A 20 px bordered number badge, then a 13 / 500 title, then a 13 px description at `.498`.
  - The recommended one has a `Recommended` pill: 12 px, radius 7.5 px, `.05` fill. It is selected at first.
  - Selected: a `.05` fill and a trailing → arrow.
  - Keys 1–9 pick and move on.
- **Free text:**
  - A pencil and the field "No, and tell … what to do differently".
  - The button reads Skip, then **Next** once something is typed, then **Submit** on the last question.
  - Next and Submit are a white pill 28 px tall with dark text.
- **After Submit:**
  - The card goes and the composer comes back.
  - The same block shows `? Asked 3 questions ›`, which opens to each question (`/60`) and its answer (`/30`, "X
    (Recommended)"). There are 4 px inside a pair and 12 px between pairs.
  - The turn carries on in the same block.

### 2.7 The end of a turn
- **Answer:** 14 / 22.75 white. Then the diff card ("Edited 2 files +7 −2 · Undo · View changes").
- **Action bar:** 6 px below, always shown. 26 px buttons with 16 px icons at `.498`: Copy, Rate, Fork.
- **Spacing:**
  - 48 px from the user bubble to the header (the bubble's 26 px hover row included).
  - 16 px from the rule to the first item.

### 2.8 Above the composer
- Notices, a "2 files changed" pill, and the question and plan cards in the composer's place.
- Long-running processes live in the side panel, not above the composer.
- A tray's chevron sits **right of its label**, 14 px in a 24 px box. It **points right when collapsed** and turns
  90° to point **down when open**, over 300 ms. It is the same chevron as the work header's.

### 2.9 Plans
- **Plan mode:** turned on from the + menu. A "Plan" chip shows, and the placeholder becomes "Describe your task to
  generate a plan…".
- **Writing the plan:** it streams inside a card headed "Writing plan", with a bulb icon and a shimmer.
- **What a plan says:**
  - An H1 title, then a one-line summary.
  - Then H2 sections, e.g. "Implementation", "Tests", "Assumptions", each a short list.
  - About 8–15 lines in all.
- **The card:**
  - Radius 12.5 px, 1 px border, faint fill. Clipped at 200 px with a 64 px fade.
  - Header: 40 px tall, "Plan" with a bulb. On the right: Download, Copy, Rate and Open in side panel, each 24 px.
  - Type: H1 21 / 28 px / 600, H2 17.5 / 24.5 / 600, list items 14 / 22.75.
  - Clicking it opens the plan in a side-panel tab. The summary panel lists "Plan · {title}".
- **"Implement this plan?":**
  - Shown in the composer's place, in the question-card style.
  - The choices: "Yes, implement this plan", or free text "No, and tell … what to do differently". Plus Skip and ✕.
  - Yes sends "Yes, implement this plan" as the user's message and leaves plan mode. ✕ also leaves plan mode.
  - Free text revises the plan.
- **Progress:** a "Step 2 / 3" pill with a filling ring. Its tooltip lists the steps with their state.

## 3. Brigadier today: the gaps

| Area | Today | Gap |
| --- | --- | --- |
| Live header | `C/RequestBlock.tsx:125-140` words match; hidden for the first 2 s (`:152`, `:173`) | Show from the first moment; 8 px to the rule; `/50` → `.498` (same) |
| Done header | `C/RequestBlock.tsx:195-212`: `ChevronRight size-icon-xs` | Chevron 14 px, centred; hover white |
| Fold motion | Keyframes on `grid-template-rows` (`styles/globals.css:64-93`) with a 150 ms timer that unmounts (`C/RequestBlock.tsx:635-658`) and `preserveAnchor` scroll fixes (`components/assistant-ui/preserve-anchor.ts`) | **Flicker source 1:** while closing, steered bubbles are inside the fold; once closed they re-render outside it (`C/RequestBlock.tsx:548-573`), so everything below jumps by their height in one frame. **Source 2:** the keyframe's last frame and the unmount timer are separate clocks, so a frame at full height or a 0-height gap can show. **Source 3:** the anchor fix scrolls the thread while the content moves, so what is below can shift twice. |
| Rows | `ROW` (`E/activity-row.tsx:20`): full-width, 20 px; chevron always shown at `/40` (`:26-27`) | Content-width, 21 px; the chevron stays always visible (the user, 2026-10-09) but quieter, and white with the text on hover |
| Chevron off-centre (image 3) | `TeamSentence` sets `items-start` (`C/activity/TeamSentence.tsx:132`), so the 12 px chevron sits at the top of a 20 px line | `items-center` everywhere; one chevron size (14 px) |
| Thinking | Live: a 2-line snippet (`C/ThinkingRow.tsx:24-30`). Settled: a row "Thought" under 1 s (`:33-38`). Timing runs from the first to the last streamed piece (`state/board.ts:459-470`), so a thought that arrives whole lasts 0 s | Live: one line with the newest heading (or first sentence). Settled: no row without text; a real duration; the row says what it was about (§4.3) |
| Steer | Already a bubble in the block (`C/blocks.ts:589-628`, `C/RequestBlock.tsx:342-365`) | Bubble 10 × 16 px padding, 22 px radius; never folds away; the block must not end because the lead asked in text (§5 Q7) |
| Question card | One question per card (`crates/core/src/work.rs:872-892`). In the composer: `ChoiceCard` (`C/ActionCards.tsx:667-868`), options are bare labels. In the thread: a large "A question for you" card kept out of the fold (`C/cards/QuestionCardView.tsx:28-61`, `C/blocks.ts:423` `keep: true`) | A round per card with a pager, option descriptions, the Recommended pill, Next/Submit. Answered, it becomes a row, "Asked 3 questions", inside the work |
| Ending | "Waiting on you" under the answer (`C/RequestBlock.tsx:371-393`) and in the side panel with Done (`C/PinnedSummary.tsx:210-294`, button `:238-247`). The request stays "waiting" while items are open (`M/requests.rs:401-423`). The merge is a question in words (`M/prompts.rs:201`), so it was asked twice | Remove the to-do list for sessions; a decision card for the merge; a short answer with a folded Details |
| Live line | "Waiting for you · {item}" from waiting items (`C/liveStatus.ts:47-49`) and "Waiting for your answer" for a text question (`:58-59`) | "Waiting for your answer" only while a card is open |
| Workers rail | `C/activity/WorkersStrip.tsx:97`: `-rotate-90` closed (points up), `rotate-90` open (points down) | Right when closed, down when open; 14 px chevron right of the label in a 24 px box |
| Action bar | `C/RequestBlock.tsx:699-739`: shown, gap 4 px, min-h 30 px | 26 px buttons, gap 2 px, 6 px below the answer |
| Plans | Plans are phases only (`work.rs:1052-1065`, `plan_phases`). The plan-mode note asks the thread to show a lead's outline "in plain words" (`M/conversation.rs:87`). The plan shows as a link on the rail (`C/ActionCards.tsx:944-981`), as phase rows in the side panel (`C/cards/PlanSection.tsx`) and as a "Phase N / M" pill (`C/ComposerCapsule.tsx:133-138`) | No written plan document, no plan card in the thread, no "Implement this plan?" choice card |

## 4. The design: the conversation view

### 4.1 The turn
1. **Order:** the user's bubble, the work header, the work, the answer, the diff card, the action bar.
2. **Gaps:** 16 px between every item (the existing `--spacing-activity` token).
3. **The column** stays 52rem (the user, 2026-10-09).
4. **The header:**
   - It shows from the first event; the 2 s delay goes (`C/RequestBlock.tsx:152`).
   - Live, it has no chevron. Waiting on a question card, it still says `Working for …`; the card and the live line
     carry the "waiting" (Q6).
   - Done, it is the ghost button with a 14 px chevron.
5. **The time runs on through question rounds.**
   - Today a wait for the user closes the request's work span (`UserRequest::moved_to`, `crates/core/src/work.rs:1628-1657`),
     and `turnTime` then shows the time waited (`C/blocks.ts:207-215`).
   - New: a wait on an open question card keeps the span open, as a quota wait does: `moved_to` takes a `card_wait`
     flag next to `quota_wait`.
   - So "Working for …" keeps counting while the user answers, and the final "Worked for …" is the wall time from the
     request's start to its end.
   - Other waits close the span, as today: an approval, a proposed plan, a takeover, a paused worker.
   - Test: a request works 10 s, waits 30 s on a card, then works 5 s. It ends as "Worked for 45s".
6. **The block folds the moment the final answer starts** (today it does only when workers ran,
   `C/RequestBlock.tsx:493-498`), and it is folded by default once done.
7. **Every size is a design token** (`styles/tokens.css`), with its Compact value where the density changes it. No
   raw values: `brigadier/no-raw-design-values` must pass. New tokens:
   - `--spacing-row` (21 px, Compact 20 px), the row's height and line;
   - `--spacing-row-gap` (4 px), between the rows of an open group;
   - `--size-chevron` (14 px);
   - `--max-height-group` (224 px), `--max-height-shell` (144 px), `--max-height-thought` (140 px);
   - `--radius-bubble` (22 px), `--radius-question-card` (25 px), `--radius-option` (15 px);
   - `--color-row-chevron`, the quieter chevron colour.

### 4.2 Folding without flicker
- **One component, `WorkFold`,** replaces the grid keyframes and the unmount timer.
  - It measures its body's height, animates `height` in px, `opacity` and `translateY(-8px)` with the Web Animations
    API, and unmounts in the animation's `finish` handler.
  - So the height, the content below and the unmount share one clock. It is 300 ms open and 160 ms closed, with the
    curves of §2.2; reduced motion makes it an instant change.
- **Steered bubbles never move between parents.** The work is cut at each bubble into segments, and each segment is
  its own `WorkFold`. The bubbles stay mounted between them in both states:
  - Folded: header, bubble, answer.
  - Open: segment, bubble, segment.
  - So nothing pops in or out when the fold settles.
- **Anchoring:** `preserveAnchor` runs only when the header is above the viewport's top edge. That is the only case
  where the content below is not what the user is looking at. Otherwise the content below moves with the height, as
  in the target.
- **The test** samples the next turn's `getBoundingClientRect().top` every frame while the block closes and opens, in
  headless Chromium:
  - every step moves in one direction;
  - no frame moves more than the animation's own step;
  - the final position equals the closed height, ±0.5 px.

### 4.3 Rows, groups and thinking
- **`ROW`** becomes `inline-flex min-h-row items-center gap-1.5 self-start text-sm leading-row`, `.6` white, from the
  tokens of §4.1.
  - The icon is 16 px; the hit area is the whole row.
  - The chevron is `size-chevron` with `ms-1`, centred by `items-center`.
  - It stays **always visible** (THREAD-UX-PLAN §3.7, kept by the user on 2026-10-09). It is quieter: the
    `--color-row-chevron` token, about `/25`, against today's `/40`. On hover or focus it turns white with the text.
  - "Always visible" means once there is something to open: a row still running has no chevron and doesn't open
    (target 22). A live group saying its running step hides its chevron until it is open, and still opens to the
    steps already done.
  - `TeamSentence`'s `items-start` goes. A sentence too long for one line truncates; the full names are in its detail.
- **Two-tone words:** the verb in the row's tone (`/60`) and what it acts on (a file, a command, a pattern, a host)
  at `--color-row-object` (48 %), both white on hover. Steps carry their object in `StepWords.object`. Running rows
  keep one tone under their shimmer. `Asked 3 questions` stays one tone, as in the target. Team sentences keep their
  own two tones: names at `/90`, verbs in the row's tone.
- **Group rows** keep THREAD-UX-PLAN §3.2's table, with no counts.
  - Open: 4 px between rows, max 224 px with a 24 px edge fade.
  - The Shell box takes §2.3's numbers (`C/activity/StepDetail.tsx`).
- **Thinking:**
  - **Timing:** a thought's duration comes only from the provider's own reasoning events: its first `ReasoningDelta`
    to its complete `Reasoning` (`crates/providers/src/model.rs:538-546`), as the board records them today
    (`state/board.ts:459-470`).
    - A thought that arrived in one piece has no measured duration, so its row shows none.
    - A duration is never inferred from the rows around it.
  - **Live:** one line, never two. It shows "Waiting for your answer" (a card is open); else the newest thought's
    heading, i.e. a leading `**…**` line (one model writes these); else its first sentence, cut to the line;
    else "Thinking".
    - The shimmer's cadence follows §2.4: a 1 s sweep, then every 4 s, the first after 600 ms.
    - It replaces the two-line snippet (`C/ThinkingRow.tsx:24-30`).
  - **After:**
    - A thought with no text has no row.
    - Otherwise the row says what it was about: `Thought for 4s · {heading or first sentence}`, or
      `Thought · {…}` when no duration was measured (or it was under 1 s). The second part is at `/40` and
      truncated.
    - It opens to the text, max 140 px (as today).
    - This replaces the bare "Thought" (image 4).
- **Live group row:** as built: the current step, shimmering.

### 4.4 Steer bubbles
- **Bubble styling** (also the normal user bubble): padding 10 × 16 px, radius 22 px, max 70 %. The time and Copy
  show on hover (as today).
- **A steered bubble ends the group before it**, as in the target. That is already true: it is a "break" in `group.ts`.
- **What fixes images 1–2 is §5 Q7:** the lead no longer ends the request with a plain-text question, so a steered
  message stays inside a "Working for …" block until the work is truly done.

### 4.5 Question cards (UI)
- **`ChoiceCard`** (`C/ActionCards.tsx:667`) grows a round mode.
  - The header holds the question, the `‹ 1 of 3 ›` pager and ✕.
  - Options show a title and an optional description.
  - The recommended option is selected at first, with a `Recommended` pill.
  - The button reads Skip, then Next once text is typed, then Submit on the last question. One `answerQuestion` call
    sends every answer.
  - Card: radius 25 px, `.082` border, `rgb(45,45,45)`-equivalent surface token.
  - Keys: 1–9 pick and advance, ←/→ page, Esc puts the card aside (as today).
- **In the thread** a question card is no longer kept out of the fold (`C/blocks.ts:423`, `keep: true` → `false`). It
  becomes an activity row:
  - open: `? Asking questions`;
  - answered: `? Asked 3 questions ›`, which opens to question/answer pairs (`/60` and `/30`, 4 px within a pair,
    12 px between pairs);
  - withdrawn: `Asked 3 questions · withdrawn`.
  - `QuestionCardView`'s big card goes.
- **The live line** reads "Waiting for your answer" while a card is open. Waiting items no longer feed it
  (`C/liveStatus.ts:47-49`), and the text-question fallback (`:58-59`) goes (§5 Q6).

### 4.6 The ending (UI)
- **The answer** is the request's last reply. Earlier replies of the same request fold into the work. A late update
  (§5 Q9) therefore replaces the ending in place rather than adding a second one.
- **`### Details`:** a session answer that has a `### Details` section shows its head, then a folded "Details" row
  (the overnight report's `splitReport`, `C/phaseView.ts:13-19`; `ReportText`, `C/RequestBlock.tsx:253-269`), for
  every session answer.
- **"Waiting on you"** goes from under the answer (`C/RequestBlock.tsx:371-393`, `:590`). It goes from the side panel
  for sessions (`C/PinnedSummary.tsx:210-294`).
- **An overnight run keeps its list** (the user, 2026-10-09):
  - It is in plain wording, and it no longer has "Done" buttons (`C/PinnedSummary.tsx:238-247`).
  - An item ends the way it does today without the user: its card settles, its task stops or reports again
    without it, or the change it held lands. It also ends when the run ends.
  - The daemon's `resolveWaiting` stays for the run card's own flows.
- **The action bar** follows §2.7: 26 px, gap 2 px, 6 px below the answer.

### 4.7 The workers rail
- **The chevron** (`C/activity/WorkersStrip.tsx:97`) points right when collapsed and down when open, 300 ms. It sits
  right after the label in a 24 px box.
- **The whole label row** is the toggle, as today.

## 5. The daemon and the instructions (grill Q1–Q9)

### Q1, Q7, Q8: the interview runs inside the thread, as one block
- **The thread's instructions** (`M/prompts.rs:97`, `:104-105`) say:
  - Never ask the user a decision as plain text. Every question goes through `ask_user`, one round per call.
  - After `ask_user`, reply with exactly `[quiet]`. The answers come back as an `[answer]` message in the same request.
  - (Today the instructions say "in your reply, or with ask_user when a task must wait".)
- **This is already one block in the daemon:**
  - An open card keeps the request waiting (`M/requests.rs:409-411`).
  - `answer_question` delivers the answer to the card's request (`M/cards.rs:305-355`, `deliver_for` with
    `question.request_id`), and the request works again (`M/conversation.rs:1284-1286`).
  - What split the blocks was the plain-text question: `asks_user` (`M/conversation.rs:4428`) ended the request, and
    the user's reply opened a new one.
- **A guard:** when a reply still ends on a question (`asks_user` is true) and no card is open, the daemon sends the
  thread one note: "Ask that with ask_user (a card), then reply [quiet]". The request stays working, and the thread
  re-asks with a card. One retry; after that, today's behaviour. Tested.
- **Grilling is built in** (Q8). This is a section of the thread's instructions modelled on the user's grilling skill:
  - **When:** the user invites questions in any wording ("grill me", "/grill-me", "ask me", "interview me", …), or a request is too vague to start.
  - **How:** a design tree worked in rounds. Each round is the **frontier**: every question whose prerequisites are
    settled.
  - **One `ask_user` call per round**, every question with 2–4 options and a recommendation.
  - **Facts** come from `query_brain`, `code_search` or a scout, never from the user. Questions that depend on a running
    scout wait for the next round.
  - **Done** when the frontier is empty. The answer is then a short summary: what was decided, and what happens next.
  - Each answered round is kept in the Brain, as `ask_user` answers are today (`M/cards.rs:331-339`).
- **No interviewer worker** (Q1): the questions and answers stay verbatim in the thread.

### Q2: a card holds a round
- **`AskUser`** (`crates/core/src/tools.rs:229-244`) becomes:
  `{ questions: [{ question, options: [{ label, description? }], recommended? }], task? }`.
  - 1–6 questions per round, 2–4 options each.
  - `recommended` is required when there are options.
  - The catalog text (`crates/mcp-server/src/catalog.rs:48-51`) says so.
- **`Question`** (`work.rs:872-892`) gains `items: Vec<QuestionItem>` and `answers: Vec<String>`.
  - A card stored before reads as a round of one (serde default from `text`, `options`, `recommended`).
  - Workers' questions (`ask_orchestrator`) and the uncommitted-changes question stay rounds of one.
- **The `answerQuestion` IPC** takes `answers: string[]`, one per item: the picked option's label, or the free text.
  - The thread gets one envelope:
    `[answer] You asked the user:\n1. {q} → {a}\n2. …`.
  - The Brain learns each pair.
  - Then `gen-ts`.

### Q6: no to-do list; "Waiting for you" only on an open card
- **In a session, nothing becomes a waiting item any more:**
  - a worker's `needs_user` (`M/workers.rs:2946-2956`, `sync_waiting`);
  - a landing's checks;
  - `note_for_user` with kind `waiting` (`M/tools.rs:643-681`). The kind **stays in the schema**
    (`tools.rs:572-580`) because overnight runs use it. Outside a run the call is refused with "Say it in your
    answer instead"; `decided` stays.
- **What decides it is the run, not the session:**
  - It is the request's or the task's run (`task.run`, or `isRunRequest`), as `prompts.rs:741-746` already decides
    it.
  - An overnight run keeps today's behaviour: nobody is there to ask, and its morning list is the point
    (PLAN.md §10.11). Its list keeps the request waiting, as today.
- **The thread's and workers' new instructions follow the same split:**
  - "Say it in your answer", the `needs_user` rule and the report label below apply only outside a run.
  - A run's instructions keep their wording (`setting_texts`, `M/prompts.rs:155-193`).
- **Only these waits go: session to-do items and plain-text questions.**
  - `waits_on_user` (`M/requests.rs:401-423`) ignores `board.waiting` for a request outside a run.
  - The text-question wait (`asked_user`, `M/requests.rs:31`) goes, because questions are cards now (Q1).
  - These still make a request wait, as today:
    - a quota wait;
    - a takeover in the user's terminal;
    - a paused worker;
    - an open approval, plan or question card;
    - a change waiting to land under "Ask for approval".
- **Workers test by hand themselves.**
  - `LEAD_STEPS` (`M/prompts.rs:630`) and the `needs_user` rule (`:741-746`) say: a "check it in the app" step is the
    worker's job. Use the scripted UI checks (fixture pages in headless Chromium, `apps/desktop/scripts/capture-*`)
    until computer use lands.
  - What truly can't be checked goes under `risks` as `Not checked: …`, never `needs_user`.
  - `needs_user` is only for a key, an account or a paid signup.
- **In the lead's ending,** a key or account becomes one plain line, "You'll need to: add STRIPE_KEY to .env". What
  couldn't be checked becomes one line, "To check: …". Neither has a button or a waiting state.
- **The report envelope's label** "Needs the user (already listed for them under Waiting on you)"
  (`M/prompts.rs:862`) becomes "Needs the user (say it in your answer)".
- **Merge: a decision card, asked once.** This is the user's grill decision Q6 (2026-10-09). It supersedes
  THREAD-PLAN §8 decision 6 ("Merging is conversational", 2026-10-08) for sessions. That line now points here.
  - A new tool, `propose_merge`, opens a question of kind `Merge { branch, base }` with two options: "Merge into
    {base}" and "Not yet".
  - **The card's answer is consent in the daemon's consent contract.**
    - `merge_consent` (`M/landing.rs:1221-1266`) takes either kind of proof.
    - **The card:** a merge card for this branch and base, answered "Merge into {base}", with no user message,
      queued follow-up or other merge since. Its id becomes `asked_in`.
    - **Words:** the latest user message, checked by `merge_consent::check` as today. A typed "merge it" stays valid.
    - Each `asked_in` merges once, as today (`:1252-1258`).
  - Overnight runs keep their own Merge on the run's card (`MergeOvernight`), unchanged.
  - **Once:** a second `propose_merge` is refused while one is open for the session. After "Not yet" it is refused too,
    until the user writes again.
  - **The instructions** (`M/prompts.rs:201`, "there is no card") are rewritten for the card.
  - Tests: consent from the card, refusal on "Not yet", and no second card.

### Q9: short endings that update instead of repeating
- **The final answer** (the user's Q9 decision; it holds whatever the Short replies setting says, and that setting
  stays as it is for everything else):
  - At most about 5 short lines: what changed and what to know.
  - Then the "To check:" and "You'll need to:" lines.
  - Then `propose_merge` when the work waits on that decision.
  - The full report (tests, review findings, what wasn't tested) goes under `### Details`, which the UI folds (§4.6).
- **A late review:** when its fix lands after the answer, the thread writes the ending again, updated, and the UI shows
  only the request's last reply as its answer (§4.6). The instructions say: never repeat a closing, never ask the
  merge again (its card is already open or answered).
- **A clean late review** stays a "Reviewed …" row in the folded work (`M/review_runs.rs:68-82`). It doesn't wake the
  thread, as today.


### One short opening line (the user, 2026-10-09)
- The thread may write one short line as it starts work that will take more than a moment, e.g. "I'll check how the
  tabs work today, then ask you a few questions." It shows as commentary at the top of the block.
- After that, the THREAD-PLAN Q1 voice rules and `[quiet]` hold as before: no narration between tool calls.
- `M/prompts.rs:104` changes from "Never write text before or between tool calls" to allow that one line.

### Instruction contract
- Every instruction change above is in the thread's or the workers' instructions. A resumed CLI keeps the
  instructions it started with (PLAN §7 "What a resumed session is told").
- So `prompts::CONTRACT` (`M/prompts.rs:305`) goes from 3 to 4 in phase 1, and to 5 in phase 2. Each bump comes in
  the phase that changes the thread's instructions.
- A session whose CLI started on an older contract starts over from its transcript instead of resuming
  (`role_outdated`, `M/prompts.rs:313-315`). That reseeding exists; the bump uses it.
- Tests, for both providers: a Claude session and a Codex session told contract 3 start over on their next turn,
  with the new instructions; one told contract 4 resumes. These extend
  `a_session_whose_cli_started_before_the_threads_instructions_starts_over` (`M/prompts.rs:1267`).
- Workers start fresh per task and need no bump.

### Q3, Q5
- **Q3** is §4.
- **Q5:** the branches `brigadier/9a2b00c9/session` and `computer-use` are not touched.

## 6. Plans (addendum)

**Target behaviour:** §2.9. **Today:** §3, last row. A Brigadier plan is a list of phases. A lead's outline lives on
its task (`PlanStep.outline`, `work.rs:907-909`). Under plan mode the thread is told to "show the outline in plain
words" (`M/conversation.rs:87`).

**This supersedes part of PLAN §10.2** (the user's addendum, 2026-10-09). A session's proposed plan shows as a plan
card in the thread, and "Implement this plan?" takes the composer's place, where §10.2 kept the plan only in the
context card. What stays from §10.2:
- The context card's Plan section stays: it lists the plan's phases and their state, and opens the plan's document.
- Message entry stays: the composer card has free text, Skip and ✕, and Esc puts it aside.
- An overnight run's phase plans stay inside its run card. Nothing here changes runs.
- §10.2's paragraph gets a line pointing here.

**The design**
1. **A plan is a document.**
   - `Plan` (`work.rs:1052`) gains `body: Option<String>`: the markdown plan.
   - A new thread tool, `propose_plan { title, body, phases? }`, records it as a Proposed plan in the request.
     `phases` are the existing phases (`Plan.steps`): progress uses them, and there is no separate steps model.
   - Under plan mode, the thread's instructions (`M/conversation.rs:87`) say: look around (yourself or scouts), then
     call `propose_plan` and reply `[quiet]`.
   - **The plan's shape is guidance, not a cap:**
     - an H1 title, then a one-line summary;
     - then H2 sections: **Changes** (a short list, files named), **Checks** (how each part will be verified) and
       **Assumptions** (decisions taken);
     - plain words, short where it can be.
     - The card keeps a long plan compact by clipping it; the whole text is one click away.
     - Plans are exempt from Short replies, as today.
2. **A lead's outline uses the same card, and the same single approval.**
   - When a lead's outline needs the user's go-ahead (under "Ask for approval"), the outline is shown as this card:
     its text is the body.
   - Its decision runs through the existing outline approval: `ApprovalSubject::Outline` → `go_ahead`. That is the
     path that clears the worker's block and moves its phase to Building. It does not go through `decide_plan`.
   - So "Yes, implement this plan" on an outline card resolves that approval. One approval releases the worker; there
     is never a second card.
   - Free text goes back to the lead as corrections, through the same path.
   - Tested: one Yes leaves no open card, and the worker runs.
3. **The plan card in the thread:** §2.9's card.
   - While the thread writes the plan, there is a "Writing plan…" placeholder row (shimmering) from the moment the
     `propose_plan` call starts until the whole document arrives.
   - Tool input arrives only once complete (`crates/providers/src/model.rs:547-555`), so nothing streams in this pass.
   - Done: clipped at 200 px with a 64 px fade. The header holds Copy and Open.
   - Clicking it opens the plan in a side-panel tab (rendered markdown).
   - It replaces the rail link (`C/ActionCards.tsx:944-981`) and the "plan" card in the thread (`C/blocks.ts:428-436`).
4. **"Implement this plan?"** is a `ChoiceCard` in the composer's place.
   - The choice is "Yes, implement this plan", or free text "No, and tell Brigadier what to do differently". Plus Skip
     and ✕.
   - **A thread's own plan:**
     - Yes approves it and leaves plan mode (`decide_plan`, `M/cards.rs:450-457`). The thread hears it.
     - Free text rejects it with that message; the thread revises and proposes again.
   - **A lead's outline:** as in item 2.
   - **✕** puts the card aside; the plan stays proposed and the side panel still offers it.
5. **Progress** uses the phase model as it is:
   - The composer pill (`C/ComposerCapsule.tsx:133-156`) keeps "Phase 2 / 3" and its ring.
   - A tooltip lists the phases with their marks.
   - The side panel's Plan item reads `Plan · {title}` and opens the plan's document.

## 7. Phases

Each phase lands as small commits on `thread-parity`.

**Every phase's "done when" includes:**
- `tools/full-checks.sh` passing on the integrated tree. That covers workspace Rust tests, clippy, `cargo fmt`,
  `gen-ts` with no diff, and the desktop typecheck, lint (`brigadier/no-raw-design-values` included), tests, build and
  real sidecar staging.
- **Native dev-app checks** on a dev build under its own identity and data dir, never the installed app:
  - folding and unfolding a work block;
  - Compact and Normal density;
  - a narrow window;
  - cards restored after the app restarts.
  Screenshots go in the phase report.
- One review by the other vendor.
- After each phase: commit, `report.md` for that phase only, status `done`, and stop. A fresh verifier checks it
  before the next phase starts.

**Screenshots** come from the fixture gallery in headless Chromium at 1728 × 1024, DPR 2, dark theme. The
before/after set is from the user's own session: its events are extracted into a fixture, the way
`thread-t1-2026-10-08.events.json` was. Each screenshot sits next to the matching target screenshot in
`msgs/evidence/compare/`. Live checks use the dev build.

### Phase 1: questions and decisions as cards, one block per request (daemon and UI)
- **Daemon:**
  - Q2's rounds: `AskUser` and `Question`, and `answerQuestion` with answers.
  - Q1/Q7/Q8's instructions, grilling, the text-question guard and the opening line.
  - The card wait that keeps "Working for" counting (§4.1).
  - Q6's `propose_merge` and the consent change.
  - `CONTRACT` 4.
- **UI:** round mode in `ChoiceCard`; the answered row "Asked N questions" in the work; the "Merge into main / Not yet"
  card; the live line "Waiting for your answer" while a card is open.
- **Done when:**
  - Rust tests:
    - a round of three answered with one call gives one envelope, and the request continues;
    - a card stored before reads as a round of one;
    - a reply ending on a question with no card gets the note, and the request stays working;
    - 10 s of work, a 30 s card wait and 5 s more work end as 45 s worked;
    - merge consent comes from the card; a typed "merge it" still works; "Not yet" refuses; a second
      `propose_merge` is refused;
    - contract 3 sessions (Claude and Codex) start over and contract 4 sessions resume.
  - `group.test.ts`: a request with two rounds and a steer renders one block, with "Asked 3 questions" rows inside it.
  - **Live, dev build:** "grill me about adding a dark-mode toggle" gives one "Working for …" block. Inside it come at
    least two card rounds with pager, Recommended and Next/Submit, then a short summary. A screenshot of the card and
    of the answered rows sits next to target 16/19.
  - The common checks above.

### Phase 2: endings without a to-do list (daemon and UI)
- **Daemon:**
  - Q6's waiting items removed outside runs; `note_for_user` kind `waiting` refused outside a run.
  - Workers test it themselves; the `needs_user` rules, scoped by run.
  - Q9's short ending and the Details fold.
  - `CONTRACT` 5.
- **UI:**
  - The request's last reply is its answer.
  - "Waiting on you" is removed for sessions. A run's list stays in plain words with no Done buttons.
- **Done when:**
  - Rust tests:
    - a worker report with `needs_user` adds no waiting item in a session, and adds one in an overnight run;
    - `note_for_user` kind `waiting` is refused in a session and accepted in a run;
    - a session request with only those items is not "waiting", while quota, takeover, a paused worker and an open
      card still make it wait.
  - `blocks.test.ts`: a request with an answer and a later updated answer shows only the second, the first in the
    fold; an answer with `### Details` folds it.
  - The user's session fixture, after: no "Waiting on you", one closing, a merge card. A screenshot sits next to
    image 5.
  - The common checks above.

### Phase 3: the conversation view at parity (desktop)
- §4.1–§4.4 and §4.7: the header, `WorkFold`, rows and the quieter chevrons, thinking, bubbles, the action bar, and
  the workers rail. The column stays 52rem.
- **Done when:**
  - **No flicker:** a frame-sampled Chromium script (`scripts/check-fold-motion.mjs`) passes the §4.2 test on the
    session fixture with and without steers. Checked in the native dev app too.
  - **Chevrons centred:** for every visible row in the fixture, the chevron's centre and the text's centre differ by
    ≤ 0.5 px, in Normal and in Compact (script check).
  - **Thinking:** `group.test.ts` shows:
    - no thought row without text;
    - a thought with text gets a row that says what it was about;
    - a thought that arrived whole shows no duration;
    - the live line is one line.
  - **Side-by-side screenshots** next to target 00, 03, 04, 06, 07, 10, 13, 15 and 22:
    - live working;
    - done and folded;
    - unfolded;
    - a group open;
    - a command open;
    - a steer, live and done;
    - the workers rail closed and open (chevron right, then down).
  - Measured header, row, gap and bubble sizes match §2 within 1 px (the script prints them). Every value comes from
    a token.
  - The common checks above.

### Phase 4: plans as documents (daemon and UI)
- §6: `Plan.body`, `propose_plan`, the plan-mode instructions, the "Writing plan…" placeholder, the plan card and its
  side-panel tab, "Implement this plan?" for a thread's plan and a lead's outline, and the phase tooltip.
- **Done when:**
  - Rust tests:
    - `propose_plan` records a Proposed plan with its body;
    - "Yes" approves it and turns plan mode off;
    - free text rejects it and the next `propose_plan` supersedes it;
    - an outline card's Yes resolves `ApprovalSubject::Outline` through `go_ahead`, the worker runs, and no second
      card opens.
  - **Live, dev build:** plan mode, "add a --version flag to the CLI", gives the placeholder, then a plan card (title,
    summary, Changes, Checks, Assumptions), then the Implement card. Yes starts the work, and the pill shows the
    phases.
  - Screenshots next to target 23–26.
  - The common checks above.

The phases run in order.

## 8. Decisions

### 8.1 The user's answers (2026-10-09)
1. **Row chevrons** stay always visible (the 10-08 ruling), in a quieter tone. Not hover-only.
2. **The column** stays 52rem.
3. **One short opening line** per request, when the work will take more than a moment (§5).
4. **Overnight runs** keep their "Waiting on you" list, in plain wording and with no "Done" buttons.
5. **Two choices this plan made are confirmed:**
   - "You'll need to: …" for a key or an account;
   - one card per round, with a pager and one send.

### 8.2 Decisions this plan supersedes
- **THREAD-PLAN §8 decision 6** ("Merging is conversational", 2026-10-08) is replaced for sessions by the merge card
  (grill Q6, 2026-10-09). A typed "merge it" stays valid consent. Overnight runs keep their own Merge.
- **PLAN §10.2**, "a session's own plan is a Plan section of the context card", is extended by the plan card in the
  thread and the composer's "Implement this plan?" (the addendum, 2026-10-09). The context card keeps its Plan section,
  and an overnight run's phase plans stay in the run card.

### 8.3 From the plan review (2026-10-09)
The thirteen points were folded in:
- the superseded decisions recorded (§8.2);
- the run-scoped waiting (§5 Q6);
- the contract bump (§5);
- the outline approval path (§6.2);
- the "Writing plan…" placeholder (§6.3);
- phases for progress (§6.5);
- continuous time (§4.1);
- the narrowed waiting rule (§5 Q6);
- tokens (§4.1);
- measured thought durations only (§4.3);
- plan length as guidance and Short replies untouched (§5 Q9, §6.1);
- `full-checks.sh` and native checks (§7).

### 8.4 From building phase 2 (2026-10-10)
- **The merge card holds nothing up.** It comes after the request's answer, so an open merge card no longer
  keeps its request waiting (phase 1 had it wait like a question round). The request is done: its work folds,
  its newest reply is the answer, and "Worked for" stops counting while the card waits in the composer's place,
  possibly overnight. The user's answer makes the request work again. `propose_merge` tells the thread to write
  its final answer with the card. A round of `ask_user` questions still keeps its request waiting with its time
  running (§4.1).
- **An overnight run's items end with the run** (§4.6): with no Done buttons nothing else would end a
  `Run`-source item; the run's report keeps the list. Items listed in a session before this build are resolved
  the next time its requests settle.
- **A question in the thread's text** makes no request wait, in runs too (§5 Q6): runs never ask the user.

### 8.5 From building phase 4 (2026-10-10)
- **The user's answer stays in its request.** "Yes, implement this plan" reaches the thread as a `[decision]`, not as
  a new user message, so a request keeps one work block (Q7). The plan card stays in view above the work it
  started; the work's own answer comes after it.
- **The plan card holds nothing up**, like the merge card (§8.4). Once the plan is proposed the request is done:
  its work folds and "Worked for" stops counting. While no reply follows it, the plan is the request's answer, and a
  line written before it folds into the work.
- **Typed changes:**
  - On the thread's own plan, they reject it with that message. The thread revises it and proposes again; the new
    revision supersedes the old one. Only a request's newest plan stays in view. Earlier revisions fold into the
    work and read "Earlier plan".
  - On a lead's outline, they are the go-ahead, with the user's words as its corrections (§6.2). There is no
    second card.
- **In plan mode**, the thread proposes a lead's outline with `propose_plan`, and the user's Yes starts that lead.
  - The lead's phase and outline move into each revision the thread proposes, and the plan it outlined under gives
    way. So after the Yes, its progress, verifier and reviews still know the lead and its outline.
  - The Yes reaches the lead with the plan the user approved, which wins over its outline. A revision made on the
    user's changes then gets built, not the first outline.
  - While the lead waits on the proposed plan, its request is done: the card holds nothing up here either.
- **Skip and ✕** both put "Implement this plan?" aside. The plan stays proposed.
- **A plan written as a document is always the user's to decide**, at every permission level. The context card's
  Plan section shows its title with a bulb and opens it, and does not ask a second time. A single-phase plan shows
  no phase row of its own.
- **A plan the thread builds itself** (no phase went to a worker) takes its progress from its request. Its first
  phase is at work while the request works, and all its phases are done once the request is. Without this rule it
  read "Approved, not started" forever.
- **While the side panel shows a plan**, its card in the thread is only its header, with the button that closes it
  there.
- **The card's header holds Copy and Open.** The target's Download and Rate are left out: Copy covers the text, and
  a plan has nothing to rate.
- **Where the writing guidance lives.** The plan's shape is in the `propose_plan` tool description and the
  plan-mode note, not in the thread prompt, which has a size test. Live runs showed one rule worth stating: one
  short line per bullet, leaving out detail the code will show anyway.
