"""Exercise the smoke's answer verdict with synthetic views; no daemon or CLI runs."""
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import smoke


class AnswerTests(unittest.TestCase):
    def verdict(self, replies):
        view = {
            "requests": [{"id": "request", "state": {"type": "done"}, "preview": "Return CODE-542"}],
            "messages": {"messages": [
                {"role": role, "requestId": request, "text": text, "seq": seq}
                for seq, (role, request, text) in enumerate(replies)
            ]},
        }
        run = smoke.Smoke.__new__(smoke.Smoke)
        run.convs = {"Codex": "conversation"}
        run.expected = {"conversation": {"request": ["CODE-542"]}}
        run.ipc = SimpleNamespace(view=lambda _: view)
        with patch.object(smoke, "check") as check:
            run.scenario_answers()
        check.assert_called_once()
        return check.call_args.args[2]

    def test_a_later_ledger_acknowledgement_keeps_the_requested_answer(self):
        self.assertTrue(self.verdict([
            ("assistant", "request", "Result: code-542"),
            ("assistant", "request", "Added entry L6."),
        ]))

    def test_a_code_in_another_request_or_the_users_words_is_no_answer(self):
        self.assertFalse(self.verdict([
            ("assistant", "other", "CODE-542"),
            ("user", "request", "CODE-542"),
            ("assistant", "request", "Added entry L6."),
        ]))

    def test_an_empty_or_quiet_reply_is_no_answer(self):
        for text in ["", "[quiet] CODE-542"]:
            with self.subTest(text=text):
                self.assertFalse(self.verdict([("assistant", "request", text)]))


class WorkerEventWordTests(unittest.TestCase):
    def test_only_user_messages_with_the_word_count_in_either_content_shape(self):
        import json, os, tempfile
        rows = [
            {"type": "user", "message": {"role": "user", "content": "[worker events wn-1] finished · CODE-R"}},
            {"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "[worker events wn-2] x"}]}},
            {"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": "ok"}]}},
            {"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "[worker events wn-3]"}]}},
        ]
        with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
            f.write("\n".join(json.dumps(r) for r in rows) + "\nnot json\n")
        self.addCleanup(os.unlink, f.name)
        self.assertEqual(smoke.worker_event_words(f.name),
                         ["[worker events wn-1] finished · CODE-R", "[worker events wn-2] x"])


if __name__ == "__main__":
    unittest.main()
