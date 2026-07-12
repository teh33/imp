import unittest
from pathlib import Path

from greeting import greeting


class GreetingTests(unittest.TestCase):
    def test_greeting_normalizes_whitespace(self) -> None:
        self.assertEqual(greeting("  ada   lovelace "), "Hi, Ada Lovelace!")

    def test_unrelated_user_note_is_preserved_exactly(self) -> None:
        self.assertEqual(
            Path("notes.txt").read_text(),
            "user note: preserve this file exactly\nuser scratch: do not overwrite\n",
        )


if __name__ == "__main__":
    unittest.main()
