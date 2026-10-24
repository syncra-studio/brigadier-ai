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


if __name__ == "__main__":
    unittest.main()
