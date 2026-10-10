"""Successful setup-only selections, kept outside runtime store payloads."""
from __future__ import annotations

from contextlib import ExitStack, contextmanager
from dataclasses import asdict, dataclass
import hashlib
import json
import os
from pathlib import Path
import tempfile

import migrate_pira_stores as migration


@dataclass
class Choice:
    path: Path
    state: dict
    previous: bytes | None


def snapshot(tool: str, sources: list[Path]) -> dict:
    result = {}
    for source in sorted(sources):
        entries = migration.inventory(source, tool=tool)
        # Empty directories also count as new historical data (notably Team runs).
        directories = sorted(path.relative_to(source).as_posix()
                             for path in source.rglob("*") if path.is_dir())
        result[str(source)] = dict(files={name: asdict(entry) for name, entry in entries.items()},
                                   directories=directories)
    return result


def read(path: Path) -> bytes | None:
    migration.checked_path(path)
    return path.read_bytes() if path.exists() else None


def plan_choice(tool: str, sources: list[Path], destination: Path,
                explicit: dict | None) -> tuple[dict, Choice | None]:
    """Reuse only exact successful source inventories, never blanket future opt-outs."""
    migration.checked_path(destination)
    for source in sources:
        if source == destination or source in destination.parents or destination in source.parents:
            raise RuntimeError("Overlapping selection roots are unsafe")
        if any(source in other.parents for other in sources):
            raise RuntimeError("Overlapping selection sources are ambiguous")
    identity = dict(tool=tool, destination=str(destination))
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    path = destination.parent / (".pira-setup-choice-" + key) / "receipt.json"
    previous = read(path)
    if previous is None and explicit is None:
        return {}, None
    current = snapshot(tool, sources)
    if previous is not None:
        try:
            state = json.loads(previous)
            if (state["schema"] != 1 or state["identity"] != identity
                    or not isinstance(state["sources"], dict)
                    or not isinstance(state["selection"], dict)):
                raise ValueError("invalid receipt fields")
            selection = state["selection"]
            if tool == "pira_ctx":
                if (set(selection) != {"completed_only", "excluded_records"}
                        or type(selection["completed_only"]) is not bool
                        or not isinstance(selection["excluded_records"], list)
                        or any(not isinstance(name, str) for name in selection["excluded_records"])):
                    raise ValueError("invalid Ctx selection")
            elif selection != {"fresh_team": True}:
                raise ValueError("invalid Team selection")
        except (ValueError, KeyError, TypeError) as error:
            raise RuntimeError(f"Invalid setup selection receipt: {path}") from error
        if explicit is None:
            # PIRA: conservative whole-source ceiling; incremental exemptions would
            # need per-record validation/dependency tracking. Never hide new data.
            if state["sources"] != current:
                raise RuntimeError("Historical store changed since successful selection; inspect it and rerun with explicit migration choices")
            explicit = selection
    if explicit is None or not sources:
        return {}, None
    state = dict(schema=1, identity=identity, sources=current, selection=explicit)
    return explicit, Choice(path, state, previous)


def verify_source(choice: Choice) -> None:
    identity = choice.state["identity"]
    if snapshot(identity["tool"], [Path(s) for s in choice.state["sources"]]) != choice.state["sources"]:
        raise RuntimeError("Historical store changed after selection preflight; no receipt published")


def source_leases(stack: ExitStack, choice: Choice) -> None:
    tool = choice.state["identity"]["tool"]
    for root, source in choice.state["sources"].items():
        for name in sorted(source["files"]):
            if migration.is_lease(tool, name):
                stack.enter_context(migration.lease(Path(root) / name))


def publish(choice: Choice) -> None:
    path = choice.path
    migration.checked_path(path)
    fd, temporary = tempfile.mkstemp(prefix="receipt-", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(choice.state, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        if os.name != "nt":
            fd = os.open(path.parent, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
    finally:
        Path(temporary).unlink(missing_ok=True)


@contextmanager
def successful_choices(choices: list[Choice], *, readonly: bool):
    """Publish receipts only after the entire migration barrier succeeds."""
    with ExitStack() as stack:
        for choice in sorted(choices, key=lambda c: str(c.path)):
            if not readonly:
                template = Path(next(iter(choice.state["sources"])))
                migration.private_parents(choice.path.parent, template)
                owner = choice.path.parent / "owner.lock"
                migration.checked_path(owner)
                try:
                    fd = os.open(owner, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    os.close(fd)
                except FileExistsError:
                    pass
                stack.enter_context(migration.lease(owner))
            if read(choice.path) != choice.previous:
                raise RuntimeError("Setup selection receipt changed after preflight; rerun")
            # Fresh Team has no native migration to acquire its run owner leases.
            if choice.state["identity"]["tool"] == "pira_team":
                source_leases(stack, choice)
            verify_source(choice)
        yield
        for choice in choices:
            if choice.state["identity"]["tool"] == "pira_ctx":
                source_leases(stack, choice)
            verify_source(choice)
        if not readonly:
            for choice in choices:
                publish(choice)
                choice.previous = read(choice.path)
