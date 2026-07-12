from pathlib import Path


def slugify(value: str) -> str:
    return value.strip().lower().replace(" ", "-")


def display_name(value: str) -> str:
    return " ".join(part.capitalize() for part in value.split("-"))


class Registry:
    def __init__(self) -> None:
        self._values: dict[str, str] = {}

    def add(self, value: str) -> str:
        key = slugify(value)
        self._values[key] = value
        return key

    def names(self) -> list[str]:
        return [display_name(key) for key in sorted(self._values)]


def load_registry(path: Path) -> Registry:
    registry = Registry()
    for line in path.read_text().splitlines():
        if line.strip():
            registry.add(line)
    return registry
