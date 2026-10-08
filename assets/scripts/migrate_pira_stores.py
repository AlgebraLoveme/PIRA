#!/usr/bin/env python3
"""Source-preserving store relocation for idle-tools setup maintenance.

Not a barrier against future legacy writers: stop tools before setup. Existing
owner leases are held and inventories rechecked; originals are never removed.
"""
from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import stat
import sys
import tempfile
import uuid


@dataclass(frozen=True)
class Entry:
    mode: int
    size: int
    digest: str
    mtime_ns: int


@dataclass
class Migration:
    tool: str
    sources: tuple[Path, ...]
    destination: Path
    inventories: dict[Path, dict[str, Entry]]
    files: dict[str, tuple[Path, Entry]]
    applied: bool = False
    completed_only: bool = False
    excluded_records: frozenset[str] = frozenset()


def checked_path(path: Path) -> None:
    """Reject links/reparse points and non-directory ancestors, without writes."""
    if not path.is_absolute() or ".." in path.parts:
        raise RuntimeError(f"Migration needs an absolute physical path: {path}")
    for part in (*reversed(path.parents), path):
        try:
            info = part.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise RuntimeError(f"Unexpected migration symlink/reparse point: {part}")
        if part != path and not stat.S_ISDIR(info.st_mode):
            raise RuntimeError(f"Migration ancestor is not a directory: {part}")


def file_entry(path: Path) -> Entry:
    checked_path(path)
    before = path.stat()
    if not stat.S_ISREG(before.st_mode):
        raise RuntimeError(f"Unsupported migration object: {path}")
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    after = path.stat()
    if (before.st_ino, before.st_size, before.st_mtime_ns, before.st_mode) != (
            after.st_ino, after.st_size, after.st_mtime_ns, after.st_mode):
        raise RuntimeError(f"Store changed while reading: {path}")
    return Entry(stat.S_IMODE(after.st_mode), after.st_size, digest.hexdigest(), after.st_mtime_ns)


def inventory(root: Path, *, excluded: frozenset[str] = frozenset(),
              tool: str | None = None) -> dict[str, Entry]:
    checked_path(root)
    if not root.exists():
        return {}
    if not root.is_dir():
        raise RuntimeError(f"Store is not a directory: {root}")
    result = {}
    def fail(error):
        raise error
    for directory, dirs, files in os.walk(root, followlinks=False, onerror=fail):
        for name in sorted(dirs):
            checked_path(Path(directory) / name)
        for name in sorted(files):
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            if relative not in excluded:
                if tool is not None and is_lease(tool, relative):
                    # Windows range locks deny reads through another handle,
                    # including our own. Lease contents are not durable records.
                    checked_path(path)
                    info = path.stat()
                    if not stat.S_ISREG(info.st_mode):
                        raise RuntimeError(f"Unsupported store lease object: {path}")
                    result[relative] = Entry(stat.S_IMODE(info.st_mode), info.st_size,
                                             f"lease:{info.st_dev}:{info.st_ino}", info.st_mtime_ns)
                else:
                    result[relative] = file_entry(path)
    return result


def copy_permissions(source: Path, target: Path) -> None:
    """Preserve POSIX mode or Windows DACL before writing sensitive bytes."""
    if os.name != "nt":
        os.chmod(target, stat.S_IMODE(source.stat().st_mode))
        return
    import ctypes
    from ctypes import wintypes
    api = ctypes.WinDLL("advapi32", use_last_error=True)
    api.GetFileSecurityW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_void_p,
                                     wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
    api.GetFileSecurityW.restype = wintypes.BOOL
    api.SetFileSecurityW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.c_void_p]
    api.SetFileSecurityW.restype = wintypes.BOOL
    needed = wintypes.DWORD()
    api.GetFileSecurityW(str(source), 4, None, 0, ctypes.byref(needed))
    if not needed.value:
        raise RuntimeError(f"Cannot read source DACL: {source}")
    descriptor = ctypes.create_string_buffer(needed.value)
    if not api.GetFileSecurityW(str(source), 4, descriptor, needed.value, ctypes.byref(needed)):
        raise RuntimeError(f"Cannot read source DACL: {source}")
    # Protect copied grants from being broadened by the new parent's inheritance.
    if not api.SetFileSecurityW(str(target), 4 | 0x80000000, descriptor):
        raise RuntimeError(f"Cannot preserve source DACL: {target}")


def same_data(left: Entry, right: Entry) -> bool:
    return (left.mode, left.size, left.digest) == (right.mode, right.size, right.digest)


def is_lease(tool: str, name: str) -> bool:
    parts = name.split("/")
    if tool == "pira_dec":
        return len(parts) == 2 and parts[-1] == ".write.lock"
    if tool == "pira_team":
        return name == "run.lock"
    return (name == "indexes/.index.owner-lock"
            or (len(parts) == 2 and parts[0] == ".events" and parts[1].endswith(".lock"))
            or (name.startswith(("live/owners/", "watch/owners/")) and name.endswith(".lock"))
            or (name.startswith("watch/control/") and name.endswith(".control-lock")))


def event_cache(name: str) -> bool:
    """Only native retention/catalog cache slots, never .piraevt or handles."""
    parts = name.split("/")
    return (parts[0] == ".events" and (
        (len(parts) == 3 and parts[-1] == ".retention.piraidx")
        or (len(parts) == 4 and parts[-1] == ".catalog.piraidx")))


def copied(tool: str, name: str, completed_only: bool = False, excluded_records: frozenset[str] = frozenset()) -> bool:
    if tool == "pira_ctx" and name in excluded_records:
        return False
    if tool == "pira_ctx" and completed_only and name.startswith(("live/", "watch/")):
        return False
    # Rebuildable Ctx indexes/journals must not collide with durable records.
    return not is_lease(tool, name) and not (
        tool == "pira_ctx" and (name.startswith("indexes/") or event_cache(name)))


def invalidate_ctx_caches(destination: Path, name: str, stage: Path) -> None:
    caches = [(destination / "indexes/.complete-v2", "index-complete-")]
    parts = name.split("/")
    if (len(parts) == 5 and parts[0] == ".events" and parts[3] == "records"
            and parts[4].endswith(".piraevt")):
        workspace = destination.joinpath(*parts[:2])
        caches.extend((path, "event-cache-" + hashlib.sha256(str(path).encode()).hexdigest() + "-")
                      for path in (workspace / ".retention.piraidx",
                                   workspace / parts[2] / ".catalog.piraidx"))
    for cache, prefix in caches:
        checked_path(cache)
        if not cache.exists():
            continue
        entry = file_entry(cache)
        backup = stage / (prefix + entry.digest)
        checked_path(backup)
        try:
            os.link(cache, backup)
        except FileExistsError:
            if not same_data(file_entry(backup), entry):
                raise RuntimeError(f"Conflicting derived-cache backup: {cache}")
        # Preserve its inode/bytes in staging, invalidate only a reconstructible
        # destination cache before publishing new records. Sources stay intact.
        cache.unlink()


@contextmanager
def lease_stream(path: Path):
    if os.name != "nt":
        with path.open("rb") as stream:
            yield stream
        return
    # Share deletion of this file, but do not assume Windows permits renaming
    # its parent with an open descendant. Team payload moves keep this path fixed.
    import ctypes
    from ctypes import wintypes
    import msvcrt
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                                  ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    kernel.CreateFileW.restype = wintypes.HANDLE
    handle = kernel.CreateFileW(str(path), 0x80000000, 7, None, 3, 0x80, None)
    if handle == ctypes.c_void_p(-1).value:
        raise OSError(ctypes.get_last_error(), f"Cannot open store lease: {path}")
    try:
        descriptor = msvcrt.open_osfhandle(handle, os.O_RDONLY)
    except BaseException:
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel.CloseHandle(handle)
        raise
    with os.fdopen(descriptor, "rb") as stream:
        yield stream


@contextmanager
def lease(path: Path):
    """Hold an existing native whole-file owner lock, without creating/changing it."""
    checked_path(path)
    if not stat.S_ISREG(path.stat().st_mode):
        raise RuntimeError(f"Unsupported store lease object: {path}")
    with lease_stream(path) as stream:
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes
            import msvcrt
            class Overlapped(ctypes.Structure):
                _fields_ = [("Internal", ctypes.c_size_t), ("InternalHigh", ctypes.c_size_t),
                            ("Offset", wintypes.DWORD), ("OffsetHigh", wintypes.DWORD),
                            ("hEvent", wintypes.HANDLE)]
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.LockFileEx.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD,
                                         wintypes.DWORD, wintypes.DWORD, ctypes.POINTER(Overlapped)]
            kernel.LockFileEx.restype = wintypes.BOOL
            state = Overlapped()
            if not kernel.LockFileEx(msvcrt.get_osfhandle(stream.fileno()), 3, 0,
                                     0xffffffff, 0xffffffff, ctypes.byref(state)):
                raise RuntimeError(f"Active or inaccessible store lease: {path}")
        else:
            import fcntl
            try:
                fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError as error:
                raise RuntimeError(f"Active or inaccessible store lease: {path}") from error
        # Closing the descriptor releases the lock on both platforms.
        yield


def check_idle(tool: str, root: Path, entries: dict[str, Entry], completed_only: bool = False) -> None:
    if tool == "pira_ctx":
        if "indexes/.index.lock" in entries:
            raise RuntimeError(f"Legacy Ctx index lock requires explicit recovery: {root}")
        for name in entries:
            if completed_only and name.startswith(("live/", "watch/")):
                continue  # Explicit maintenance mode leaves operational state at source.
            if name.startswith("live/") and name.endswith(".live.json"):
                raise RuntimeError(f"Unfinished capture requires recovery before migration: {root / name}")
            if name.startswith("watch/state/") and name.endswith(".json"):
                try:
                    state = json.loads((root / name).read_text(encoding="utf-8"))
                    inactive = state.get("monitor") in ("stopped", "complete", "deadline", "failed")
                except (ValueError, AttributeError) as error:
                    raise RuntimeError(f"Invalid watch state: {root / name}") from error
                if not inactive:
                    raise RuntimeError(f"Active/ambiguous watch prevents migration: {root / name}")


TEAM_EXCLUDED = frozenset({"control.json", "codex-home/auth.json"})
TEAM_TERMINAL = {"completed", "needs_decision", "incomplete", "interrupted", "timed_out", "failed"}


def transform_team_manifest(data: bytes, source_run: Path, destination_run: Path) -> bytes:
    """Rebase ONLY approved schema-3/4 managed fields; never native conversation state."""
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise RuntimeError(f"Duplicate Team manifest key: {key}")
            result[key] = value
        return result
    try:
        manifest = json.loads(data, object_pairs_hook=unique)
    except (ValueError, UnicodeError) as error:
        raise RuntimeError("Malformed Team manifest") from error
    if (not isinstance(manifest, dict) or type(manifest.get("schema_version")) is not int
            or manifest["schema_version"] not in (3, 4)):
        raise RuntimeError("Unsupported Team manifest schema; originals preserved")
    if manifest.get("run_id") != source_run.name or destination_run.name != source_run.name:
        raise RuntimeError("Team run ID disagrees with actual directory")
    if (not isinstance(manifest.get("status"), str) or manifest["status"] not in TEAM_TERMINAL
            or manifest.get("active_turn") is not None):
        raise RuntimeError("Active/ambiguous Team run requires recovery before migration")
    def relative(value):
        if not isinstance(value, str) or not value or Path(value).is_absolute():
            raise RuntimeError("Team artifact must be a normal run-relative path")
        if any(part in ("", ".", "..") for part in re.split(r"[\\/]", value)):
            raise RuntimeError("Team artifact contains non-normal path components")
        return Path(value)
    def managed(value, field):
        if not isinstance(value, str) or not Path(value).is_absolute():
            raise RuntimeError("Team managed field must be an absolute path")
        if any(part in (".", "..") for part in re.split(r"[\\/]", value)):
            raise RuntimeError("Team managed field contains traversal")
        try:
            tail = Path(value).relative_to(source_run)
        except ValueError as error:
            raise RuntimeError("Team managed field lies outside its actual source run") from error
        # Revision one writes launcher outputs directly in the run directory.
        first_revision = ((field == "logs" and not tail.parts)
                          or (field == "candidate" and tail.parts == ("candidate.txt",))
                          or (field == "diagnostics" and tail.parts == ("validation.json",)))
        if not first_revision and (not tail.parts or tail.parts[0] not in ("artifacts", "logs", "implementation", "revisions")):
            raise RuntimeError("Unsupported Team managed artifact layout")
        return str(destination_run / tail)
    for key in ("result", "logs", "review_checkpoint"):
        if manifest.get(key) is not None:
            manifest[key] = managed(manifest[key], key)
    if manifest.get("artifact") is not None:
        relative(manifest["artifact"])
    for key in ("attempts", "revisions"):
        if key not in manifest:
            continue
        if not isinstance(manifest[key], list) or any(not isinstance(item, dict) for item in manifest[key]):
            raise RuntimeError(f"Invalid Team {key} inventory")
    for attempt in manifest.get("attempts", []):
        for key in ("logs", "candidate", "diagnostics"):
            if attempt.get(key) is not None:
                attempt[key] = managed(attempt[key], key)
    for revision in manifest.get("revisions", []):
        for key in ("artifact", "manifest"):
            if revision.get(key) is not None:
                relative(revision[key])
    return (json.dumps(manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8")


def team_history(source_run: Path, destination_run: Path, *, acquire_lock: bool = True) -> dict[str, tuple[int, str]]:
    """Validate idle known-schema durable history, excluding auth/control/lock state."""
    checked_path(source_run)
    with ExitStack() as locks:
        owner = source_run / "run.lock"
        checked_path(owner)
        if acquire_lock and owner.exists():
            locks.enter_context(lease(owner))
        entries = inventory(source_run, excluded=TEAM_EXCLUDED, tool="pira_team")
        if "manifest.json" not in entries:
            raise RuntimeError(f"Unattributed Team directory without manifest: {source_run}")
        result = {}
        for name, entry in entries.items():
            if name == "run.lock":
                continue
            digest = entry.digest
            if name == "manifest.json" or re.fullmatch(r"revisions/[0-9]{6}/manifest\.json", name):
                transformed = transform_team_manifest((source_run / name).read_bytes(), source_run, destination_run)
                digest = hashlib.sha256(transformed).hexdigest()
            result[name] = (entry.mode, digest)
        if inventory(source_run, excluded=TEAM_EXCLUDED, tool="pira_team") != entries:
            raise RuntimeError("Team source changed during preflight")
        return result


def team_source_fingerprint(source_run: Path) -> str:
    """Fingerprint retained original bytes/modes, not destination's mutable history.

    Caller holds run locks for the complete migration/receipt transaction. Auth,
    control and lease capabilities are deliberately not migration payloads.
    """
    team_run_identity(source_run)  # caller already owns the run lease
    entries = inventory(source_run, excluded=TEAM_EXCLUDED | {"run.lock"})
    encoded = json.dumps({name: [entry.mode, entry.size, entry.digest]
                          for name, entry in sorted(entries.items())}, sort_keys=True).encode()
    if inventory(source_run, excluded=TEAM_EXCLUDED | {"run.lock"}) != entries:
        raise RuntimeError("Team source changed during fingerprint validation")
    return hashlib.sha256(encoded).hexdigest()


def team_run_identity(run: Path) -> dict[str, str]:
    checked_path(run / "manifest.json")
    manifest = json.loads(transform_team_manifest((run / "manifest.json").read_bytes(), run, run))
    thread = manifest.get("thread_id")
    if not isinstance(thread, str) or not thread.strip():
        raise RuntimeError("Unrecognized Team retained thread identity")
    return {"run_id": run.name, "thread_id": thread}


def team_migration_receipt(source_run: Path, destination_run: Path, *,
                           expected_source: str, validate_native_identity) -> bytes:
    """Produce provenance ONLY after caller verified copy and native repair.

    Caller holds source/destination run locks, supplies the preflight source
    fingerprint, and durably publishes these bytes before config publication.
    The helper callback must validate native retained-thread resolution at the
    FINAL destination and return {run_id, thread_id}; manifest equality alone
    is not native validation. No helper is selected implicitly here.
    """
    identity = team_run_identity(source_run)
    if (team_run_identity(destination_run) != identity
            or validate_native_identity(destination_run) != identity):
        raise RuntimeError("Team destination native identity verification failed")
    if team_source_fingerprint(source_run) != expected_source:
        raise RuntimeError("Team original source changed before receipt publication")
    return (json.dumps({"schema": 1, "source": str(source_run),
                        "destination": str(destination_run), "source_fingerprint": expected_source,
                        "identity": identity}, sort_keys=True) + "\n").encode()


def validate_team_migration_receipt(data: bytes, source_run: Path, destination_run: Path, *,
                                    validate_native_identity) -> None:
    """Read-only lineage no-op check; never compare/overwrite evolved target bytes.

    Missing/malformed receipts are errors, not authorization to bless a divergent
    same-ID history. Caller must retain the pre-receipt collision checks and must
    not fall back from an invalid receipt to new migration.
    """
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate receipt key")
            result[key] = value
        return result
    try:
        receipt = json.loads(data, object_pairs_hook=unique)
    except (TypeError, ValueError, UnicodeError) as error:
        raise RuntimeError("Missing or invalid Team migration receipt") from error
    if (not isinstance(receipt, dict) or type(receipt.get("schema")) is not int
            or receipt != {"schema": 1, "source": str(source_run),
                           "destination": str(destination_run),
                           "source_fingerprint": team_source_fingerprint(source_run),
                           "identity": team_run_identity(source_run)}):
        raise RuntimeError("Invalid Team receipt or original source changed")
    if (team_run_identity(destination_run) != receipt["identity"]
            or validate_native_identity(destination_run) != receipt["identity"]):
        raise RuntimeError("Team receipt destination identity no longer valid")


@dataclass
class TeamMigration:
    source: Path
    destination: Path
    ledger: Path
    fingerprint: str
    identity: dict[str, str]
    binary: str
    source_members: tuple[str, ...] = ()
    applied: bool = False


def team_backend():
    import team_store_relocation
    return team_store_relocation


def team_ledger(source: Path, destination: Path) -> Path:
    key = hashlib.sha256(json.dumps([str(source), str(destination)]).encode()).hexdigest()
    return destination.parent.parent / (".pira-team-migrate-" + key)


def read_team_state(plan: TeamMigration):
    path = plan.ledger / "state.json"
    checked_path(path)
    if not path.exists():
        if plan.ledger.exists() and any(item.name != "owner.lock" for item in plan.ledger.iterdir()):
            raise RuntimeError("Missing Team migration receipt/state; preserve ledger for recovery")
        return None
    if not path.is_file():
        raise RuntimeError("Invalid Team ledger object; expected regular file")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate state key")
            result[key] = value
        return result
    try:
        state = json.loads(path.read_bytes(), object_pairs_hook=unique)
        valid = (type(state["schema"]) is int and state["schema"] == 1
                 and state["source"] == str(plan.source) and state["destination"] == str(plan.destination)
                 and state["identity"]["run_id"] == plan.source.name
                 and isinstance(state["identity"]["thread_id"], str)
                 and bool(state["identity"]["thread_id"])
                 and re.fullmatch(r"[0-9a-f]{64}", state["fingerprint"])
                 and type(state["attempt"]) is int and state["attempt"] >= 1
                 and state["phase"] in {"copying", "publishing", "backing_up", "populating", "published", "repairing", "quarantining", "restoring", "complete", "failed"}
                 and state.get("layout") in (None, "stable_lock")
                 and (state.get("layout") != "stable_lock" or type(state.get("had_previous")) is bool)
                 and (state["phase"] not in {"backing_up", "populating", "quarantining", "restoring"}
                      or state.get("layout") == "stable_lock"))
    except (ValueError, TypeError, KeyError) as error:
        raise RuntimeError("Invalid Team migration ledger; preserve it for recovery") from error
    if not valid:
        raise RuntimeError("Invalid Team migration provenance; never retry as a new migration")
    return state


def save_team_state(plan: TeamMigration, state: dict) -> None:
    path = plan.ledger / "state.json"
    checked_path(path)
    fd, name = tempfile.mkstemp(prefix="state-", dir=plan.ledger)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as stream:
            json.dump(state, stream, sort_keys=True)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        if os.name != "nt":
            directory = os.open(plan.ledger, os.O_RDONLY)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
    finally:
        Path(name).unlink(missing_ok=True)


def team_lineage(plan: TeamMigration, state: dict, *, locks_held=False) -> dict:
    if not locks_held:
        with ExitStack() as locks:
            for run in (plan.source, plan.destination):
                lock = run / "run.lock"
                checked_path(lock)
                if lock.exists():
                    locks.enter_context(lease(lock))
            return team_lineage(plan, state, locks_held=True)
    helper = team_backend()
    identity = helper.validate_identity(plan.source, plan.destination,
                                        plan.identity["run_id"], plan.identity["thread_id"])
    evidence = state.get("repair_evidence", {})
    if (not isinstance(evidence, dict) or evidence.get("run_id") != plan.identity["run_id"]
            or evidence.get("thread_id") != plan.identity["thread_id"]
            or evidence.get("source_run") != str(plan.source)
            or evidence.get("destination_run") != str(plan.destination)
            or evidence.get("destination") != str(plan.destination / "codex-home")
            or evidence.get("source_fingerprint") != identity.get("source_fingerprint")
            or evidence.get("source_restored") is not True
            or not isinstance(evidence.get("history_prefixes"), dict)
            or not evidence["history_prefixes"]):
        raise RuntimeError("Invalid Team original repair evidence in receipt")
    for prefix in evidence["history_prefixes"].values():
        if (not isinstance(prefix, dict) or type(prefix.get("count")) is not int or prefix["count"] < 0
                or not isinstance(prefix.get("sha256"), str)
                or not re.fullmatch(r"[0-9a-f]{64}", prefix["sha256"])):
            raise RuntimeError("Invalid Team original history-prefix receipt evidence")
    validate_team_migration_receipt(json.dumps(state.get("receipt")).encode(), plan.source, plan.destination,
        validate_native_identity=lambda run: {key: identity.get(key) for key in ("run_id", "thread_id")})
    return identity


def preflight_team_relocation(sources: list[Path], destination: Path, *, codex_binary=None) -> list[TeamMigration]:
    """Read-only discovery. Incomplete caller transactions are resumed only on apply."""
    plans = []
    seen = set()
    checked_path(destination)
    for root in sources:
        checked_path(root)
        if root == destination or root in destination.parents or destination in root.parents:
            raise RuntimeError("Overlapping Team migration roots")
        if not root.exists():
            continue
        for source in sorted(root.iterdir()):
            checked_path(source)
            if not source.is_dir() or source.name in seen:
                raise RuntimeError(f"Unattributed/ambiguous Team source: {source}")
            seen.add(source.name)
            target = destination / source.name
            checked_path(target)
            plan = TeamMigration(source, target, team_ledger(source, target), "", {}, "",
                                 tuple(sorted(item.name for item in root.iterdir())))
            state = read_team_state(plan)
            if state:
                plan.fingerprint, plan.identity = state["fingerprint"], state["identity"]
                if state["phase"] == "complete":
                    team_lineage(plan, state)  # No first-copy target equality on valid receipt.
            else:
                expected = team_history(source, target)
                if target.exists() and team_history(target, target) != expected:
                    raise RuntimeError(f"Conflicting Team same-ID history: {source.name}; neither history may be overwritten or merged")
                plan.fingerprint = team_source_fingerprint(source)
                plan.identity = team_run_identity(source)
            helper = team_backend()
            if not helper.capabilities()["admitted"]:
                raise RuntimeError("Team retained native relocation blocked: native platform admission pending; originals preserved")
            binary = shutil.which("codex") if codex_binary is None else codex_binary
            if not binary:
                raise RuntimeError("Team relocation requires a selected supported Codex executable or codex on PATH")
            plan.binary = str(binary)
            if state is None or state["phase"] in ("failed", "complete"):
                helper.inspect_source(source, source.name, plan.identity["thread_id"], codex_binary=plan.binary)
            plans.append(plan)
    return plans


def directory_identity(path: Path):
    checked_path(path)
    if not path.exists():
        return None
    info = path.stat()
    if not stat.S_ISDIR(info.st_mode):
        raise RuntimeError(f"Expected Team directory: {path}")
    return [info.st_dev, info.st_ino]


def move_team_payload(source: Path, destination: Path) -> None:
    """Move unlocked payload children, never the leased run directory/run.lock.

    Each rename is no-overwrite and resumable: on interruption remaining source
    names are disjoint from already moved destination names. Keep both roots.
    """
    checked_path(source)
    private_parents(destination, source)
    for child in sorted(source.iterdir()):
        if child.name == "run.lock":
            continue
        checked_path(child)
        target = destination / child.name
        checked_path(target)
        if target.exists():
            raise RuntimeError(f"Conflicting Team payload recovery name: {target}")
        child.rename(target)


def owned_empty_team_target(plan: TeamMigration, state: dict | None) -> bool:
    expected = state.get("empty_target_directory") if state else None
    if expected is None:
        return False
    if (directory_identity(plan.destination) != expected
            or any(child.name != "run.lock" for child in plan.destination.iterdir())):
        raise RuntimeError("Team empty recovery container changed; nothing overwritten")
    return True


def recover_stable_team_payload(plan: TeamMigration, state: dict, attempt: Path) -> None:
    target_id = directory_identity(plan.destination)
    expected = state.get("published_directory", state.get("previous_directory"))
    if target_id is not None and target_id != expected:
        raise RuntimeError("Team destination replaced during recovery; nothing overwritten")
    previous = attempt / "previous"
    if state["phase"] == "copying":
        # Copying did not change the original destination payload.
        pass
    else:
        if state["phase"] == "backing_up":
            move_team_payload(plan.destination, previous)  # finish interrupted old-payload backup
            state["phase"] = "restoring"
            save_team_state(plan, state)
        elif state["phase"] != "restoring":
            state["phase"] = "quarantining"
            save_team_state(plan, state)
            if target_id is not None:
                move_team_payload(plan.destination, attempt / "quarantine")
            state["phase"] = "restoring"
            save_team_state(plan, state)
        if state["had_previous"]:
            checked_path(previous)
            if not previous.is_dir():
                raise RuntimeError("Missing Team previous payload; preserve transaction for recovery")
            move_team_payload(previous, plan.destination)
    state["empty_target_directory"] = target_id if not state["had_previous"] else None
    state["phase"] = "failed"
    save_team_state(plan, state)


def recover_team_attempt(plan: TeamMigration, state: dict) -> None:
    """Restore original source FIRST; never attest shutdown of an unknown backend."""
    attempt = plan.ledger / f"attempt-{state['attempt']:06d}"
    journal = attempt / "native-journal.json"
    checked_path(journal)
    if journal.exists():
        restored = team_backend().recover(journal)  # uncertain/live native PID stays fail-closed
        if restored.get("source_restored") is not True:
            raise RuntimeError("Team source recovery incomplete; preserve journal and target")
    if team_source_fingerprint(plan.source) != plan.fingerprint:
        raise RuntimeError("Team original source changed; preserve transaction for recovery")
    if state.get("layout") == "stable_lock":
        recover_stable_team_payload(plan, state, attempt)
        return
    # Compatibility recovery for the earlier directory-publication journal.
    target_id = directory_identity(plan.destination)
    if target_id is not None:
        if target_id == state.get("published_directory"):
            previous = attempt / "previous"
            checked_path(previous)
            if previous.exists() and directory_identity(previous) != state.get("previous_directory"):
                raise RuntimeError("Team previous destination recovery conflict")
            # Upgrade a pending old journal before touching its leased target.
            # Completed old receipts continue to use unchanged lineage semantics.
            state.update(layout="stable_lock", had_previous=previous.exists(), phase="quarantining")
            save_team_state(plan, state)
            recover_stable_team_payload(plan, state, attempt)
            return
        elif target_id != state.get("previous_directory"):
            raise RuntimeError("Team destination replaced during recovery; nothing overwritten")
    previous = attempt / "previous"
    checked_path(previous)
    if previous.exists():
        if plan.destination.exists() or directory_identity(previous) != state.get("previous_directory"):
            raise RuntimeError("Team previous destination recovery conflict")
        previous.rename(plan.destination)
    state["phase"] = "failed"
    save_team_state(plan, state)


def copy_team_candidate(plan: TeamMigration, candidate: Path) -> None:
    """Independent, verified byte copies; native files never use hard links."""
    entries = inventory(plan.source, excluded=TEAM_EXCLUDED | {"run.lock"})
    private_parents(candidate, plan.source)
    for name, entry in sorted(entries.items()):
        source, target = plan.source / name, candidate / name
        private_parents(target.parent, source.parent)
        with target.open("xb") as stream:
            copy_permissions(source, target)
            digest = entry.digest
            if name == "manifest.json" or re.fullmatch(r"revisions/[0-9]{6}/manifest\.json", name):
                data = transform_team_manifest(source.read_bytes(), plan.source, plan.destination)
                digest = hashlib.sha256(data).hexdigest()
                stream.write(data)
            else:
                with source.open("rb") as original:
                    shutil.copyfileobj(original, stream, 1024 * 1024)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(target, entry.mode)
        os.utime(target, ns=(entry.mtime_ns, entry.mtime_ns))
        actual = file_entry(target)
        if actual.digest != digest or actual.mode != entry.mode:
            raise RuntimeError("Team independent copy verification failed")
        if source.stat().st_ino == target.stat().st_ino and source.stat().st_dev == target.stat().st_dev:
            raise RuntimeError("Team native copy unexpectedly shares an inode")
    if inventory(plan.source, excluded=TEAM_EXCLUDED | {"run.lock"}) != entries:
        raise RuntimeError("Team source changed during independent copy")
    if team_source_fingerprint(plan.source) != plan.fingerprint:
        raise RuntimeError("Team source changed before final publication")
    # Preserve empty native directories recognized by helper's inventory too.
    for directory, dirs, _ in os.walk(plan.source, followlinks=False):
        for name in dirs:
            source = Path(directory) / name
            checked_path(source)
            private_parents(candidate / source.relative_to(plan.source), source)
    with (candidate / "run.lock").open("xb"):
        pass


def verify_team_membership(plans: list[TeamMigration]) -> None:
    for plan in plans:
        checked_path(plan.source.parent)
        if tuple(sorted(item.name for item in plan.source.parent.iterdir())) != plan.source_members:
            raise RuntimeError("Team source run inventory changed after preflight; rerun before config publication")


def apply_team_migrations(plans: list[TeamMigration], *, dry_run=False, verify=False) -> None:
    verify_team_membership(plans)
    for plan in plans:
        helper = team_backend()
        if not helper.capabilities()["admitted"]:
            raise RuntimeError("Team retained native relocation blocked: native platform admission pending")
        if dry_run or verify:
            state = read_team_state(plan)
            if state and state["phase"] == "complete":
                team_lineage(plan, state)
            elif verify:
                raise RuntimeError("Team migration incomplete; apply/recovery required")
            elif state:
                raise RuntimeError("Team interrupted migration needs apply recovery; dry-run made no changes")
            elif team_source_fingerprint(plan.source) != plan.fingerprint:
                raise RuntimeError("Team source changed after preflight")
            continue
        private_parents(plan.ledger, plan.source)
        owner = plan.ledger / "owner.lock"
        checked_path(owner)
        if owner.exists() and not owner.is_file():
            raise RuntimeError("Invalid Team ledger owner lock object")
        with owner.open("ab"):
            pass
        with ExitStack() as locks:
            locks.enter_context(lease(owner))
            destination_leased = False
            for run in (plan.source, plan.destination):
                lock = run / "run.lock"
                checked_path(lock)
                if lock.exists():
                    locks.enter_context(lease(lock))
                    destination_leased = destination_leased or run == plan.destination
            state = read_team_state(plan)
            if state and (state["fingerprint"] != plan.fingerprint or state["identity"] != plan.identity):
                raise RuntimeError("Team migration provenance changed after preflight")
            if state and state["phase"] == "complete":
                team_lineage(plan, state, locks_held=True)
                if not plan.applied:
                    result = helper.verify_native_identity(plan.source, plan.destination,
                        plan.identity["run_id"], plan.identity["thread_id"], evidence=state["repair_evidence"],
                        codex_binary=plan.binary, runtime_dir=plan.ledger / ("verify-" + uuid.uuid4().hex))
                    if result.get("native_verified") is not True:
                        raise RuntimeError("Team native receipt verification did not succeed")
                    team_lineage(plan, state, locks_held=True)
                plan.applied = True
                continue
            if state and state["phase"] != "failed":
                recover_team_attempt(plan, state)
            if team_source_fingerprint(plan.source) != plan.fingerprint:
                raise RuntimeError("Team original source changed after preflight")
            expected = team_history(plan.source, plan.destination, acquire_lock=False)
            empty_target = owned_empty_team_target(plan, state)
            if (plan.destination.exists() and not empty_target
                    and team_history(plan.destination, plan.destination, acquire_lock=False) != expected):
                raise RuntimeError("Conflicting Team same-ID history before publication")
            number = state["attempt"] + 1 if state else 1
            attempt = plan.ledger / f"attempt-{number:06d}"
            checked_path(attempt)
            if attempt.exists():
                raise RuntimeError("Team attempt staging already occupied; preserve for recovery")
            state = dict(schema=1, source=str(plan.source), destination=str(plan.destination),
                         identity=plan.identity, fingerprint=plan.fingerprint, attempt=number, phase="copying",
                         previous_directory=directory_identity(plan.destination), layout="stable_lock",
                         had_previous=plan.destination.exists() and not empty_target)
            save_team_state(plan, state)
            private_parents(attempt, plan.source)
            candidate = attempt / "candidate" / plan.source.name
            try:
                copy_team_candidate(plan, candidate)
                existing = plan.destination.exists()
                if existing and not destination_leased:
                    lock = plan.destination / "run.lock"
                    checked_path(lock)
                    if not lock.exists():
                        with lock.open("xb"):
                            pass
                    locks.enter_context(lease(lock))
                if directory_identity(plan.destination) != state["previous_directory"]:
                    raise RuntimeError("Team destination appeared or changed during copy; nothing overwritten")
                if existing:
                    if state["had_previous"]:
                        if team_history(plan.destination, plan.destination, acquire_lock=False) != expected:
                            raise RuntimeError("Conflicting Team destination changed during copy")
                    elif any(child.name != "run.lock" for child in plan.destination.iterdir()):
                        raise RuntimeError("Team empty recovery container gained payload during copy")
                state.update(phase="backing_up" if state["had_previous"] else "populating" if existing else "publishing",
                             published_directory=directory_identity(plan.destination if existing else candidate))
                save_team_state(plan, state)
                private_parents(plan.destination.parent, plan.source.parent)
                if existing:
                    if state["had_previous"]:
                        move_team_payload(plan.destination, attempt / "previous")
                        state["phase"] = "populating"
                        save_team_state(plan, state)
                    # The original run.lock stays held at its original path.
                    move_team_payload(candidate, plan.destination)
                else:
                    # No handles open within candidate yet: initial directory
                    # publication is supported even on Windows.
                    candidate.rename(plan.destination)
                    locks.enter_context(lease(plan.destination / "run.lock"))
                state["phase"] = "published"
                save_team_state(plan, state)
                native_plan = helper.preflight(plan.source, plan.destination, plan.identity["run_id"],
                    plan.identity["thread_id"], codex_binary=plan.binary, journal_path=attempt / "native-journal.json")
                if native_plan.get("admitted") is not True:
                    raise RuntimeError("Team native preflight is not admitted")
                state["phase"] = "repairing"
                save_team_state(plan, state)
                evidence = helper.repair(plan.source, plan.destination, plan.identity["run_id"],
                    plan.identity["thread_id"], codex_binary=plan.binary, journal_path=attempt / "native-journal.json")
                identity = helper.validate_identity(plan.source, plan.destination,
                    plan.identity["run_id"], plan.identity["thread_id"])
                receipt = team_migration_receipt(plan.source, plan.destination, expected_source=plan.fingerprint,
                    validate_native_identity=lambda run: {key: identity.get(key) for key in ("run_id", "thread_id")})
                state.update(phase="complete", repair_evidence=evidence, receipt=json.loads(receipt))
                team_lineage(plan, state, locks_held=True)
                save_team_state(plan, state)  # durable receipt BEFORE caller configuration publication
                plan.applied = True
            except BaseException:
                # A failed receipt save also needs recovery; never bless native writes
                # without persisted evidence. Keep all copies/journals for inspection.
                state["phase"] = "repairing" if state.get("phase") == "complete" else state["phase"]
                recover_team_attempt(plan, state)
                raise

    verify_team_membership(plans)
    if not dry_run and not verify:
        for plan in plans:
            team_lineage(plan, read_team_state(plan))


def plan_migration(tool: str, sources: list[Path], destination: Path, *, completed_only: bool = False, excluded_records: frozenset[str] = frozenset()) -> Migration:
    """Read-only merge/collision/idle preflight; no configuration changes."""
    if tool not in ("pira_ctx", "pira_dec"):
        raise RuntimeError("Team retained native relocation blocked pending backend adapter validation")
    if any(not name.endswith(".piractx") or name in (".piractx",)
           or any(c in name for c in ("/", "\\", "\x00")) for name in excluded_records):
        raise RuntimeError("Excluded Ctx records must be individual .piractx filenames")
    if excluded_records and tool != "pira_ctx":
        raise RuntimeError("Record exclusions apply only to Ctx")
    roots = tuple(sorted(set(sources) - {destination}))
    for source in roots:
        if any(source in other.parents for other in roots):
            raise RuntimeError("Overlapping migration sources are ambiguous")
        if source in destination.parents or destination in source.parents:
            raise RuntimeError("Overlapping migration roots are unsafe")
    snapshots = {root: inventory(root, tool=tool) for root in (*roots, destination)}
    files: dict[str, tuple[Path, Entry]] = {}
    identities: dict[str, tuple[Path, Entry]] = {}
    with ExitStack() as locks:
        for root, entries in snapshots.items():
            check_idle(tool, root, entries, completed_only)
            for name in sorted(entries):
                if is_lease(tool, name):
                    locks.enter_context(lease(root / name))
            if inventory(root, tool=tool) != entries:
                raise RuntimeError(f"Store changed during preflight: {root}")
            for name, entry in entries.items():
                if not copied(tool, name, completed_only, excluded_records):
                    continue
                previous = identities.get(name)
                if previous and not same_data(previous[1], entry):
                    raise RuntimeError(f"Conflicting store identity: {name} in {root} and {previous[0]}")
                identities[name] = (root / name, entry)
                target = destination / name
                checked_path(target)
                if target.is_dir():
                    raise RuntimeError(f"File/directory collision: {target}")
                if root != destination:
                    files[name] = (root / name, entry)
    for name in identities:
        if any(parent.as_posix() in identities for parent in Path(name).parents if parent != Path(".")):
            raise RuntimeError(f"File/directory collision: {name}")
    return Migration(tool, roots, destination, snapshots, files, completed_only=completed_only, excluded_records=frozenset(excluded_records))


def private_parents(path: Path, template: Path | None = None) -> None:
    checked_path(path)
    if not path.exists():
        private_parents(path.parent, template)
        path.mkdir(mode=0o700, exist_ok=True)
        if os.name == "nt" and template is not None:
            copy_permissions(template, path)
    if not path.is_dir():
        raise RuntimeError(f"Not a migration directory: {path}")


def verify_sources(plan: Migration) -> None:
    for root in plan.sources:
        if inventory(root, tool=plan.tool) != plan.inventories[root]:
            raise RuntimeError(f"Source changed after migration preflight: {root}")


def verify_destination(plan: Migration) -> None:
    current = inventory(plan.destination, tool=plan.tool)
    for name, entry in plan.inventories[plan.destination].items():
        if copied(plan.tool, name, plan.completed_only, plan.excluded_records) and (name not in current or not same_data(entry, current[name])):
            raise RuntimeError(f"Existing destination changed: {name}")
    for name, (_, entry) in plan.files.items():
        if name not in current or not same_data(entry, current[name]):
            raise RuntimeError(f"Migration destination verification failed: {name}")


def apply_migrations(plans: list[Migration], *, dry_run: bool = False, verify: bool = False) -> None:
    """Revalidate, stage, verify, publish without replacement, then verify again.

    Interrupted publication is recoverable: verified identical destination files
    are no-ops on a newly planned rerun. Config writers must call this first.
    """
    destinations = [plan.destination for plan in plans]
    if len(set(destinations)) != len(destinations):
        raise RuntimeError("Combine sources for each destination into one migration plan")
    for destination in destinations:
        for plan in plans:
            if any(destination == source or destination in source.parents or source in destination.parents
                   for source in plan.sources):
                raise RuntimeError("Migration destination overlaps a source")
    with ExitStack() as locks:
        for plan in plans:
            for root, entries in plan.inventories.items():
                if not (plan.applied and root == plan.destination) and inventory(root, tool=plan.tool) != entries:
                    raise RuntimeError(f"Store changed after migration preflight: {root}")
                check_idle(plan.tool, root, entries, plan.completed_only)
                for name in sorted(entries):
                    if is_lease(plan.tool, name):
                        locks.enter_context(lease(root / name))
        if dry_run or verify:
            if verify:
                for plan in plans:
                    verify_destination(plan)
            return
        staged = []
        for plan in plans:
            if not plan.files:
                continue
            identity = hashlib.sha256((plan.tool + str(plan.destination)).encode()).hexdigest()[:24]
            stage = plan.destination.parent / (".pira-migrate-" + identity)
            private_parents(stage, plan.sources[0])
            owner = stage / "owner.lock"
            checked_path(owner)
            if not owner.exists():
                try:
                    descriptor = os.open(owner, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    os.close(descriptor)
                except FileExistsError:
                    pass
            locks.enter_context(lease(owner))
            for name, (source, entry) in sorted(plan.files.items()):
                target = plan.destination / name
                checked_path(target)
                if target.exists():
                    if not same_data(file_entry(target), entry):
                        raise RuntimeError(f"Destination conflict: {target}")
                    continue
                ready = stage / hashlib.sha256(name.encode()).hexdigest()
                checked_path(ready)
                if not ready.exists() or not same_data(file_entry(ready), entry):
                    private_parents(ready.parent)
                    partial = ready.with_name(ready.name + ".partial")
                    checked_path(partial)
                    # Interrupted scratch is disposable; never reuse a hardlinked inode.
                    partial.unlink(missing_ok=True)
                    fd = os.open(partial, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
                    with os.fdopen(fd, "wb") as output, source.open("rb") as original:
                        copy_permissions(source, partial)
                        shutil.copyfileobj(original, output, 1024 * 1024)
                        output.flush()
                        os.fsync(output.fileno())
                    os.chmod(partial, entry.mode)
                    os.utime(partial, ns=(entry.mtime_ns, entry.mtime_ns))
                    if not same_data(file_entry(partial), entry):
                        raise RuntimeError(f"Staged copy verification failed: {source}")
                    os.replace(partial, ready)
                staged.append((plan, ready, target, entry))
        for plan in plans:
            verify_sources(plan)
        for plan, ready, target, entry in staged:
            private_parents(target.parent, plan.files[target.relative_to(plan.destination).as_posix()][0].parent)
            if plan.tool == "pira_ctx":
                invalidate_ctx_caches(plan.destination, target.relative_to(plan.destination).as_posix(), ready.parent)
            try:
                os.link(ready, target)  # Atomic no-overwrite publication; no replacing fallback.
            except FileExistsError:
                if not same_data(file_entry(target), entry):
                    raise RuntimeError(f"Destination conflict during publication: {target}")
        for plan in plans:
            verify_sources(plan)
            verify_destination(plan)
            plan.applied = True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tool", action="append", choices=("pira_ctx", "pira_dec", "pira_team"))
    import setup_pira_stores as setup
    setup.add_migration_arguments(parser)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="copy verified data then update shell configuration")
    mode.add_argument("--verify", action="store_true", help="read-only verification")
    args = parser.parse_args()
    plan = setup.plan_store_environment(args.tool or ["pira_ctx", "pira_dec"],
        completed_ctx_only=args.completed_ctx_only, fresh_team=args.fresh_team,
        exclude_ctx_records=args.exclude_ctx_record)
    setup.apply_store_environment(plan, dry_run=not args.apply, verify=args.verify)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError) as error:
        print(f"Migration failed: {error}", file=sys.stderr)
        raise SystemExit(1)
