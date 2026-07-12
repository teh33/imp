def normalize_name(value: str) -> str:
    return " ".join(value.strip().split()).title()


def greeting(value: str) -> str:
    return f"Hello, {normalize_name(value)}!"
