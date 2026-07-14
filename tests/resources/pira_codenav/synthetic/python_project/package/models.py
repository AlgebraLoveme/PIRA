from dataclasses import dataclass


@dataclass(frozen=True)
class User:
    """Minimal user record."""

    name: str
    active: bool = True


def normalize_name(name: str) -> str:
    return name.strip().casefold()
