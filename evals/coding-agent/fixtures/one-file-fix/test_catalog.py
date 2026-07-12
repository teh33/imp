import unittest

from catalog import Registry, display_name, slugify


class CatalogTests(unittest.TestCase):
    def test_slugify_collapses_whitespace(self) -> None:
        self.assertEqual(slugify("  Hello   World  "), "hello-world")

    def test_display_name_formats_slug(self) -> None:
        self.assertEqual(display_name("hello-world"), "Hello World")

    def test_registry_names_are_sorted(self) -> None:
        registry = Registry()
        registry.add("zebra item")
        registry.add("apple item")
        self.assertEqual(registry.names(), ["Apple Item", "Zebra Item"])


if __name__ == "__main__":
    unittest.main()
