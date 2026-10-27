import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Received = { id: string | null; caption: string | null; text: string | null; right: boolean };
type Row = { text: string | null; state: string | null; id: string | null; link: string | null; message: string | null; cut: boolean; shimmer: boolean; opens: boolean };
type Seen = {
  failed?: string;
  renderError: string | null;
  receiver: { received: Received[]; headers: boolean; answer: string | null; automaticNotice: boolean };
  jump: { opened: string | null; rows: Row[] };
  tooltip: string | null;
  sending: Row | null;
  sentLive: Row | null;
  receivedLive: { received: Received[]; headers: boolean };
};

const SENDER = "0192aaaa-bbbb-7ccc-8ddd-0000a11ce5e5";
const ASK = "Can you check the orders schema before I cut the release? The migration in 0042 renames customer_id, and the export job still reads the old column on every nightly run.";
const CAPTION = "Sent by Release prep from another session";

// S30: a session's message to another shows in both threads, read back and live. The receiver
// shows it where the user's own message stands, captioned with its sender; the sender's work has
// one row, "Messaged API work: ‹message›", the message cut to the line with the whole in a
// tooltip, the name opening the receiver.
test("sessions messaging each other: the receiver's captioned message and the sender's row, read back and live", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "session-messages.html", "session-messages-result", 30000)) as Seen;
  assert.equal(seen.failed, undefined, String(seen.failed));
  assert.equal(seen.renderError, null, `a turn failed to render: ${seen.renderError}`);

  // The receiver, read back: the message, never Brigadier's header, captioned, right-aligned; its reply below.
  assert.deepEqual(seen.receiver.received, [{ id: "sm-0123456789ab", caption: CAPTION, text: ASK, right: true }]);
  assert.equal(seen.receiver.headers, false, "Brigadier's header never shows");
  assert.equal(seen.receiver.automaticNotice, false, "automatic engine notices never become bubbles beside authored peer content");
  assert.equal(seen.receiver.answer, "The export job reads customer_id; I'll point it at the new column.");

  // Its caption opens the sender at the row of that message, its folded work opened.
  assert.equal(seen.jump.opened, SENDER);
  assert.deepEqual(seen.jump.rows, [{
    text: `Messaged API work: ${ASK}`, state: "sent", id: "sm-0123456789ab", link: "API work", message: ASK, cut: true, shimmer: false, opens: false,
  }]);
  assert.equal(seen.tooltip, ASK, "the whole message in the tooltip");

  // Live in the sender: "Messaging API work" while the call runs, then the row of what it sent.
  assert.equal(seen.sending?.text, "Messaging API work");
  assert.equal(seen.sending?.shimmer, true);
  assert.equal(seen.sending?.link, "API work");
  assert.deepEqual(seen.sentLive, {
    text: "Messaged API work: Also the docs.", state: "sent", id: "sm-ba9876543210", link: "API work", message: "Also the docs.", cut: false, shimmer: false, opens: false,
  });

  // Live in the receiver: the message steered into its running turn shows there, captioned the same.
  assert.deepEqual(seen.receivedLive.received.at(-1), { id: "sm-ba9876543210", caption: CAPTION, text: "Also the docs.", right: true });
  assert.equal(seen.receivedLive.headers, false);
});
