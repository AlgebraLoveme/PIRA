from __future__ import annotations

from contextlib import ExitStack
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import migrate_pira_stores as migration
import setup_migration_choices as choices
import setup_pira_stores as setup


class ChoiceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.old = self.root / "old"
        self.team = self.root / "old-team"
        self.destination = self.root / "data/pira/ctx"
        stack = self.enterContext(ExitStack())
        stack.enter_context(patch.dict(os.environ, {"HOME": str(self.root),
            "XDG_DATA_HOME": str(self.root / "data")}, clear=True))
        stack.enter_context(patch.object(setup.sys, "platform", "linux"))
        stack.enter_context(patch.object(setup, "historical_store_paths",
            side_effect=lambda tool: [self.team if tool == "pira_team" else self.old]))

    def put(self, root, name, data=b"record"):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def plan(self, **kwargs):
        return setup.plan_store_environment(["pira_ctx", "pira_team"], profile_paths=[], **kwargs)

    def selective(self):
        self.put(self.old, "ok.piractx")
        self.put(self.old, "bad.piractx", b"bad")
        self.put(self.old, "live/abandoned.live.json", b"{}")
        self.put(self.team, "unreleased/manifest.json", b"unsupported")
        return self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True)

    def apply(self, plan, **kwargs):
        setup.apply_store_migrations(plan, dry_run=kwargs.pop("dry_run", False), **kwargs)

    def test_successful_selective_then_ordinary_nonempty_rerun(self):
        self.put(self.destination, "destination-only.piractx", b"later")
        plan = self.selective()
        original = migration.inventory(self.old)
        self.apply(plan)
        for receipt in plan.choices:
            self.assertTrue(receipt.path.is_file())
        self.apply(self.plan())
        # Unified setup may run the same barrier twice around configuration writes.
        self.apply(plan)
        self.assertEqual(set(migration.inventory(self.destination)),
                         {"ok.piractx", "destination-only.piractx"})
        self.assertEqual(migration.inventory(self.old), original)
        self.assertFalse((self.root / "data/pira/team").exists())

    def test_adopt_pre_receipt_migration_and_preserve_nonempty_team(self):
        plan = self.selective()
        migration.apply_migrations(plan.migrations)  # Old setup wrote no choices.
        self.put(self.root / "data/pira/team", "new-run/history", b"destination history")
        with self.assertRaisesRegex(RuntimeError, "Unfinished capture"):
            self.plan()
        self.apply(self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True))
        self.apply(self.plan())
        self.assertEqual((self.root / "data/pira/team/new-run/history").read_bytes(), b"destination history")

    def test_changed_or_added_source_needs_explicit_reselection(self):
        plan = self.selective()
        self.apply(plan)
        for root, name in ((self.old, "new.piractx"), (self.old, "bad.piractx"),
                           (self.old, "live/new.live.json"),
                           (self.team, "new/manifest.json")):
            with self.subTest(name=name):
                path = root / name
                original = path.read_bytes() if path.exists() else None
                old_stat = path.stat() if path.exists() else None
                self.put(root, name, b"newly valid or changed")
                with self.assertRaisesRegex(RuntimeError, "Historical store changed"):
                    self.plan()
                if original is None:
                    path.unlink()
                    if path.parent != root and not list(path.parent.iterdir()):
                        path.parent.rmdir()
                else:
                    path.write_bytes(original)
                    os.utime(path, ns=(old_stat.st_atime_ns, old_stat.st_mtime_ns))
        # Reapprove operational leftovers, but no longer exclude repaired capture.
        self.put(self.old, "bad.piractx", b"repaired")
        self.apply(self.plan(completed_ctx_only=True, fresh_team=True))
        self.assertEqual((self.destination / "bad.piractx").read_bytes(), b"repaired")
        self.apply(self.plan())

    def test_conflict_never_creates_receipt(self):
        self.selective()
        self.put(self.destination, "ok.piractx", b"conflict")
        with self.assertRaisesRegex(RuntimeError, "Conflicting"):
            self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True)
        self.assertFalse(list(self.root.rglob("receipt.json")))

    def test_failed_and_interrupted_migration_does_not_authorize_skip(self):
        plan = self.selective()
        with patch.object(migration.os, "link", side_effect=OSError("interrupted")):
            with self.assertRaisesRegex(OSError, "interrupted"):
                self.apply(plan)
        self.assertFalse(any(choice.path.exists() for choice in plan.choices))
        with self.assertRaisesRegex(RuntimeError, "Unfinished capture"):
            self.plan()
        self.apply(self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True))
        self.apply(self.plan())

    def test_later_tool_failure_does_not_publish_ctx_selection(self):
        plan = self.selective()
        with patch.object(migration, "apply_team_migrations", side_effect=RuntimeError("later tool failed")):
            with self.assertRaisesRegex(RuntimeError, "later tool failed"):
                self.apply(plan)
        self.assertTrue((self.destination / "ok.piractx").exists())
        self.assertFalse(any(choice.path.exists() for choice in plan.choices))

    def test_publication_interruption_and_stale_plan(self):
        plan = self.selective()
        real_replace = os.replace
        def replace(source, destination):
            if Path(destination).name == "receipt.json":
                raise OSError("receipt interruption")
            return real_replace(source, destination)
        with patch.object(choices.os, "replace", side_effect=replace):
            with self.assertRaisesRegex(OSError, "receipt interruption"):
                self.apply(plan)
        self.assertFalse(any(c.path.exists() for c in plan.choices))
        stale = self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True)
        self.apply(plan)
        with self.assertRaisesRegex(RuntimeError, "receipt changed"):
            self.apply(stale)

    def test_dry_run_verify_and_fresh_install_make_no_receipts(self):
        self.apply(self.plan())
        self.assertEqual(list(self.root.iterdir()), [])
        plan = self.selective()
        before = migration.inventory(self.root)
        self.apply(plan, dry_run=True)
        with self.assertRaisesRegex(RuntimeError, "verification failed"):
            self.apply(plan, verify=True)
        self.assertEqual(migration.inventory(self.root), before)
        self.apply(plan)
        before = migration.inventory(self.root)
        self.apply(self.plan(), verify=True)
        self.apply(self.plan(), dry_run=True)
        self.assertEqual(migration.inventory(self.root), before)

    def test_source_and_destination_scope_and_empty_new_team_run(self):
        plan = self.selective()
        self.apply(plan)
        (self.team / "empty-new-run").mkdir()
        with self.assertRaisesRegex(RuntimeError, "Historical store changed"):
            self.plan()
        selection, receipt = choices.plan_choice("pira_ctx", [self.old], self.root / "other/ctx", None)
        self.assertEqual(selection, {})
        self.assertIsNone(receipt)
        other = self.root / "other-source"
        self.put(other, "new.piractx")
        with self.assertRaisesRegex(RuntimeError, "Historical store changed"):
            choices.plan_choice("pira_ctx", [other], self.destination, None)

    def test_source_change_after_plan_no_receipt(self):
        plan = self.selective()
        self.put(self.team, "unreleased/manifest.json", b"changed")
        with self.assertRaisesRegex(RuntimeError, "after selection preflight"):
            self.apply(plan)
        self.assertFalse(any(c.path.exists() for c in plan.choices))
        self.assertFalse(self.destination.exists())

    @unittest.skipIf(os.name == "nt", "POSIX lock holder fixture")
    def test_team_owner_and_choice_owner_leases_block(self):
        import fcntl
        self.selective()
        owner = self.put(self.team, "unreleased/run.lock", b"")
        plan = self.plan(completed_ctx_only=True, exclude_ctx_records=["bad.piractx"], fresh_team=True)
        with owner.open("rb") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(RuntimeError, "Active or inaccessible"):
                self.apply(plan)
        self.assertFalse(any(c.path.exists() for c in plan.choices))
        self.apply(plan)
        with (plan.choices[0].path.parent / "owner.lock").open("rb") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(RuntimeError, "Active or inaccessible"):
                self.apply(self.plan())

    def test_malformed_and_symlink_receipts_fail_closed(self):
        plan = self.selective()
        self.apply(plan)
        receipt = plan.choices[0].path
        receipt.write_bytes(b"not-json")
        with self.assertRaisesRegex(RuntimeError, "Invalid setup selection receipt"):
            self.plan()
        receipt.unlink()
        try:
            receipt.symlink_to(self.old / "ok.piractx")
        except OSError:
            self.skipTest("symlinks unavailable")
        with self.assertRaisesRegex(RuntimeError, "symlink/reparse"):
            self.plan()


if __name__ == "__main__":
    unittest.main()
