import unittest


class Counter:
    def __init__(self, value: int = 0) -> None:
        self.value = value

    def increment(self, amount: int = 1) -> int:
        self.value += amount
        return self.value


class CounterTests(unittest.TestCase):
    def test_increment_uses_default_amount(self) -> None:
        counter = Counter(2)
        self.assertEqual(counter.increment(), 3)

    def test_increment_accepts_explicit_amount(self) -> None:
        counter = Counter(2)
        self.assertEqual(counter.increment(4), 6)


if __name__ == "__main__":
    unittest.main()
