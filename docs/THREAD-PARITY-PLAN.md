# Thread parity plan: a conversation that reads like the target, interview cards, clear endings

> Status: draft for review, 2026-10-09. Branch `thread-parity` from `main` at `a9f6924d`.
> It builds on docs/THREAD-UX-PLAN.md (approved 2026-10-08, built). Where the two differ, this plan wins; each
> such change is marked **(changes THREAD-UX-PLAN §n)**.
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
  - Two-tone rows: the verb at about 90 % of the tertiary colour, the object at 40 %.
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
| Rows | `ROW` (`E/activity-row.tsx:20`): full-width, 20 px; chevron always shown (`:26-27`) | Content-width, 21 px; chevron on hover **(changes THREAD-UX-PLAN §3.7, "always visible")** |
| Chevron off-centre (image 3) | `TeamSentence` sets `items-start` (`C/activity/TeamSentence.tsx:132`), so the 12 px chevron sits at the top of a 20 px line | `items-center` everywhere; one chevron size (14 px) |
| Thinking | Live: a 2-line snippet (`C/ThinkingRow.tsx:24-30`). Settled: a row "Thought" under 1 s (`:33-38`). Timing runs from the first to the last streamed piece (`state/board.ts:459-470`), so a thought that arrives whole lasts 0 s | Live: one line with the newest heading (or first sentence). Settled: no row without text; a real duration; the row says what it was about (§4.3) |
| Steer | Already a bubble in the block (`C/blocks.ts:589-628`, `C/RequestBlock.tsx:342-365`) | Bubble 10 × 16 px padding, 22 px radius; never folds away; the block must not end because the lead asked in text (§5 Q7) |
| Question card | One question per card (`crates/core/src/work.rs:872-892`). In the composer: `ChoiceCard` (`C/ActionCards.tsx:667-868`), options are bare labels. In the thread: a large "A question for you" card kept out of the fold (`C/cards/QuestionCardView.tsx:28-61`, `C/blocks.ts:423` `keep: true`) | A round per card with a pager, option descriptions, the Recommended pill, Next/Submit. Answered, it becomes a row, "Asked 3 questions", inside the work |
| Ending | "Waiting on you" under the answer (`C/RequestBlock.tsx:371-393`) and in the side panel with Done (`C/PinnedSummary.tsx:210-294`, button `:238-247`). The request stays "waiting" while items are open (`M/requests.rs:401-423`). The merge is a question in words (`M/prompts.rs:201`), so it was asked twice | Remove the to-do list for sessions; a decision card for the merge; a short answer with a folded Details |
| Live line | "Waiting for you · {item}" from waiting items (`C/liveStatus.ts:47-49`) and "Waiting for your answer" for a text question (`:58-59`) | "Waiting for your answer" only while a card is open |
| Workers rail | `C/activity/WorkersStrip.tsx:97`: `-rotate-90` closed (points up), `rotate-90` open (points down) | Right when closed, down when open; 14 px chevron right of the label in a 24 px box |
| Action bar | `C/RequestBlock.tsx:699-739`: shown, gap 4 px, min-h 30 px | 26 px buttons, gap 2 px, 6 px below the answer |
| Column | `--container-thread: 52rem` (`styles/tokens.css:293`) | 48rem (768 px) **(changes THREAD-UX-PLAN §3.7, "stays 52rem")** |
| Plans | Plans are phases only (`work.rs:1052-1065`, `plan_phases`). The plan-mode note asks the thread to show a lead's outline "in plain words" (`M/conversation.rs:87`). The plan shows as a link on the rail (`C/ActionCards.tsx:944-981`), as phase rows in the side panel (`C/cards/PlanSection.tsx`) and as a "Phase N / M" pill (`C/ComposerCapsule.tsx:133-138`) | No written plan document, no plan card in the thread, no "Implement this plan?" choice card |

## 4. The design: the conversation view

### 4.1 The turn
1. **Order:** the user's bubble, the work header, the work, the answer, the diff card, the action bar.
2. **Gaps:** 16 px between every item (the existing `--spacing-activity` token).
3. **The column** narrows to 48rem.
4. **The header:**
   - It shows from the first event; the 2 s delay goes (`C/RequestBlock.tsx:152`).
   - Live, it has no chevron. Waiting on a card, it still says `Working for …`; the card and the live line carry the
     "waiting" (Q6).
   - Done, it is the ghost button with a 14 px chevron.
5. **The block folds the moment the final answer starts** (today it does only when workers ran,
   `C/RequestBlock.tsx:493-498`), and it is folded by default once done.

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
- **`ROW`** becomes `inline-flex min-h-[21px] items-center gap-1.5 self-start text-sm leading-[21px]`, `.6` white.
  - The icon is 16 px; the hit area is the whole row.
  - The chevron is 14 px with `ms-1` and `items-center`. It is hidden until hover or focus (`opacity-0
    group-hover:opacity-100 group-focus-visible:opacity-100`) and shown while open.
  - `TeamSentence`'s `items-start` goes. A sentence too long for one line truncates; the full names are in its detail.
- **Two-tone words:** the verb at `/90` of the tertiary colour and the object at `/40`, for `Asked 3 questions`,
  `Edited x.ts +5 −1` (green/red) and lifecycle sentences (`Analyze right panel` / `started working`).
- **Group rows** keep THREAD-UX-PLAN §3.2's table, with no counts.
  - Open: 4 px between rows, max 224 px with a 24 px edge fade.
  - The Shell box takes §2.3's numbers (`C/activity/StepDetail.tsx`).
- **Thinking:**
  - **Timing:** a thought runs from the end of the item before it (the previous tool's end, or the turn's start) to
    the start of the item after it. So a thought that arrives whole still gets its real length. This is computed in
    `group.ts` from neighbours, not from streamed pieces (`state/board.ts:459-470` stays).
  - **Live:** one line, never two. It shows "Waiting for your answer" (a card is open); else the newest thought's
    heading, i.e. a leading `**…**` line (one model writes these); else its first sentence, cut to the line;
    else "Thinking".
    - The shimmer's cadence follows §2.4: a 1 s sweep, then every 4 s, the first after 600 ms.
    - It replaces the two-line snippet (`C/ThinkingRow.tsx:24-30`).
  - **After:**
    - A thought with no text, or under 2 s, has no row.
    - Otherwise the row reads `Thought for 4s · {heading or first sentence}`, the second part at `/40` and
      truncated. It opens to the text, max 140 px (as today).
    - This replaces the bare "Thought" (image 4): every thought row now says what it was about.
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
  for sessions (`C/PinnedSummary.tsx:210-294`); an overnight run keeps it (§5 Q6).
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
  - **When:** "grill me", "/grill-me", or a request too vague to start.
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
  - `note_for_user` with kind `waiting` (`M/tools.rs:643-681`). That kind goes from the tool's schema
    (`tools.rs:572-580`); `decided` stays.
  - `waits_on_user` (`M/requests.rs:401-423`) drops `board.waiting` for session requests.
  - An overnight run keeps today's behaviour: nobody is there to ask, and its morning list is the point
    (PLAN.md §10.11).
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
- **Merge: a decision card, asked once.**
  - A new tool, `propose_merge`, opens a question of kind `Merge { branch, base }` with two options: "Merge into
    {base}" and "Not yet".
  - **Consent:** `merge_consent` (`M/landing.rs:1221-1266`) accepts a merge card answered "Merge" after the latest user
    message, with `asked_in` = the card's id. The words check stays for a merge asked in words.
  - **Once:** a second `propose_merge` is refused while one is open for the session. After "Not yet" it is refused too,
    until the user writes again.
  - **The instructions** (`M/prompts.rs:201`, "there is no card") are rewritten for the card.
  - Tests: consent from the card, refusal on "Not yet", and no second card.

### Q9: short endings that update instead of repeating
- **The final answer:**
  - At most about 5 short lines: what changed and what to know.
  - Then the "To check:" and "You'll need to:" lines.
  - Then `propose_merge` when the work waits on that decision.
  - The full report (tests, review findings, what wasn't tested) goes under `### Details`, which the UI folds (§4.6).
- **A late review:** when its fix lands after the answer, the thread writes the ending again, updated, and the UI shows
  only the request's last reply as its answer (§4.6). The instructions say: never repeat a closing, never ask the
  merge again (its card is already open or answered).
- **A clean late review** stays a "Reviewed …" row in the folded work (`M/review_runs.rs:68-82`). It doesn't wake the
  thread, as today.

### Q3, Q5
- **Q3** is §4.
- **Q5:** the branches `brigadier/9a2b00c9/session` and `computer-use` are not touched.

## 6. Plans (addendum)

**Target behaviour:** §2.9. **Today:** §3, last row. A Brigadier plan is a list of phases. A lead's outline lives on
its task (`PlanStep.outline`, `work.rs:907-909`). Under plan mode the thread is told to "show the outline in plain
words".

**The design**
1. **A plan is a document.**
   - `Plan` (`work.rs:1052`) gains `body: Option<String>`: the markdown plan.
   - A new thread tool, `propose_plan { title, body, phases? }`, records it as a Proposed plan in the request.
   - Under plan mode, the thread's instructions (`M/conversation.rs:87`) say: look around (yourself or scouts), then
     call `propose_plan` and reply `[quiet]`.
   - The plan's shape: an H1 title, then a one-line summary. Then H2 sections: **Changes** (a short list, files named),
     **Checks** (how each part will be verified) and **Assumptions** (decisions taken). At most ~15 lines, plain words.
   - A lead's outline that the user must approve (under "Ask for approval") is shown the same way. Its outline becomes
     the plan's body.
2. **The plan card in the thread:** §2.9's card.
   - While the tool's arguments stream: "Writing plan", shimmering.
   - Done: clipped at 200 px with a 64 px fade. The header holds Copy and Open.
   - Clicking it opens the plan in a side-panel tab (rendered markdown).
   - It replaces the rail link (`C/ActionCards.tsx:944-981`) and the "plan" card in the thread (`C/blocks.ts:428-436`).
3. **"Implement this plan?"** is a `ChoiceCard` in the composer's place.
   - The choice is "Yes, implement this plan", or free text "No, and tell Brigadier what to do differently". Plus Skip
     and ✕.
   - **Yes** sends "Yes, implement this plan" as the user's message, approves the plan, and leaves plan mode (as
     `decide_plan` does, `M/cards.rs:450-457`).
   - **Free text** is sent as the user's message: the thread revises and proposes again, and the old plan is
     superseded.
   - **✕** leaves plan mode and the plan stays proposed.
4. **Progress:**
   - The composer pill keeps its ring (`C/ComposerCapsule.tsx:140-156`) and reads `Step 2 / 3` when the plan has
     steps. It keeps `Phase 2 / 3` for phases.
   - A tooltip lists the steps with their marks.
   - The side panel's Plan item reads `Plan · {title}` and opens the plan tab.

## 7. Phases

Each phase lands as small commits on `thread-parity`. Every phase ends with:
- `cargo fmt --check`;
- `cargo clippy` on the touched crates;
- `gen-ts` when Rust types change;
- the desktop `pnpm` typecheck, lint and tests;
- one review by the other vendor.

**Screenshots** come from the fixture gallery in headless Chromium at 1728 × 1024, DPR 2, dark theme. The
before/after set is from the user's own session: its events are extracted into a fixture, the way
`thread-t1-2026-10-08.events.json` was. Each screenshot sits next to the matching target screenshot in
`msgs/evidence/compare/`. Live checks use a dev build under its own identity and data dir.

### Phase 1: questions and decisions as cards, one block per request (daemon and UI)
- **Daemon:** Q2's `AskUser` and `Question` rounds, and `answerQuestion` with answers. Q1/Q7/Q8's instructions,
  grilling and the text-question guard. Q6's `propose_merge` and the consent change.
- **UI:** round mode in `ChoiceCard`; the answered row "Asked N questions" in the work; the "Merge into main / Not yet"
  card; the live line "Waiting for your answer" only on an open card.
- **Done when:**
  - Rust tests:
    - a round of three answered with one call gives one envelope, and the request continues;
    - a card stored before reads as a round of one;
    - a reply ending on a question with no card gets the note, and the request stays working;
    - merge consent comes from the card; "Not yet" refuses; a second `propose_merge` is refused.
  - `group.test.ts`: a request with two rounds and a steer renders one block, with "Asked 3 questions" rows inside it.
  - **Live, dev build:** "grill me about adding a dark-mode toggle" gives one "Working for …" block. Inside it come at
    least two card rounds with pager, Recommended and Next/Submit, then a short summary. A screenshot of the card and
    of the answered rows sits next to target 16/19.

### Phase 2: endings without a to-do list (daemon and UI)
- Q6 without waiting items (sessions); workers' "test it yourself" and `needs_user` rules; Q9's short ending and
  Details fold; the request's last reply as its answer; the "Waiting on you" UI removed for sessions.
- **Done when:**
  - Rust tests:
    - a worker report with `needs_user` adds no waiting item in a session, and does in an overnight run;
    - `note_for_user` rejects kind `waiting`;
    - a session request with no open card is never "waiting".
  - `blocks.test.ts`: a request with an answer and a later updated answer shows only the second, the first in the
    fold; an answer with `### Details` folds it.
  - The user's session fixture, after: no "Waiting on you", one closing, a merge card. A screenshot sits next to
    image 5.

### Phase 3: the conversation view at parity (desktop)
- §4.1–§4.4 and §4.7: the header, `WorkFold`, rows and chevrons, thinking, bubbles, the action bar, the column, and the
  workers rail.
- **Done when:**
  - **No flicker:** a frame-sampled Chromium script (`scripts/check-fold-motion.mjs`) passes the §4.2 test on the
    session fixture with and without steers.
  - **Chevrons centred:** for every visible row in the fixture, the chevron's centre and the text's centre differ by
    ≤ 0.5 px (script check).
  - **Thinking:** `group.test.ts` shows no thought row without text, and none under 2 s. A thought that arrives whole
    between two tools gets their gap as its time. The live line is one line.
  - **Side-by-side screenshots** next to target 00, 03, 04, 06, 07, 10, 13, 15 and 22:
    - live working;
    - done and folded;
    - unfolded;
    - a group open;
    - a command open;
    - a steer, live and done;
    - the workers rail closed and open (chevron right, then down).
  - Measured header, row, gap and bubble sizes match §2 within 1 px (the script prints them).

### Phase 4: plans as documents (daemon and UI)
- §6: `Plan.body`, `propose_plan`, the plan-mode instructions, the plan card and side-panel tab, "Implement this
  plan?", and the step pill.
- **Done when:**
  - Rust tests:
    - `propose_plan` records a Proposed plan with its body;
    - "Yes" approves it and turns plan mode off;
    - free text supersedes it on the next `propose_plan`.
  - **Live, dev build:** plan mode, "add a --version flag to the CLI", gives a plan card (title, summary, Changes,
    Checks, Assumptions, ≤ 15 lines) and the Implement card. Yes starts the work, and the pill shows `Step 1 / 3`.
  - Screenshots next to target 23–26.

The phases run in order. Phase 3 touches only the desktop, so it can start once phase 1's `blocks.ts` change has
landed.

## 8. Open points for the user
1. **Chevrons on hover.** The target hides a row's chevron until hover. THREAD-UX-PLAN §3.7 made them always visible
   (2026-10-08). Recommendation: follow the target (hover only).
2. **The column narrows** from 52rem to 48rem (768 px), as the target. Recommendation: yes.
3. **One short opening line.** The target writes one short line of commentary as it starts work ("I'll check the
   helper and the tests, then propose a plan."). Brigadier's voice rules forbid any text before or between tool calls
   (THREAD-PLAN Q1). Recommendation: allow one short opening line per request, when the work will take more than a
   moment. The rest of the Q1 rules and `[quiet]` stay.
4. **Overnight runs** keep their "Waiting on you" list, since nobody is there to ask (§5 Q6). Recommendation: yes.
