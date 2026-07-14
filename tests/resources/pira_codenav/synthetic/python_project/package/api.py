"""Public fixture API."""

import json as jsonlib
from pathlib import Path

from .models import User, normalize_name

DEFAULT_LIMIT = 10


class Client:
    """Loads users from JSON payloads."""

    def __init__(self, root: Path, *, limit: int = DEFAULT_LIMIT) -> None:
        self.root = root
        self.limit = limit

    def fetch(self, relative: str) -> list[User]:
        """Load at most ``limit`` users from a relative path."""
        payload = (self.root / relative).read_text(encoding="utf-8")
        return parse_payload(payload)[: self.limit]


def parse_payload(payload: str) -> list[User]:
    records = jsonlib.loads(payload)
    return [User(normalize_name(item["name"])) for item in records]


def résumé(items: list[User]) -> tuple[int, int]:
    """Exercise Unicode identifiers without executing the fixture."""
    return len(items), sum(item.active for item in items)
