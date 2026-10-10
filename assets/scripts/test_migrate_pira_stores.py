from __future__ import annotations

import json
import os
import shutil
from pathlib import Path
import tempfile
import tomllib
import subprocess
import sys
import unittest
from unittest.mock import patch, MagicMock

import migrate_pira_stores as migration
import setup_pira_stores as setup


def native_ctx_environment(root):
    # OS loader/command-discovery inputs only; no auth, agent session IDs or
    # user configuration. All home/data/temp roots are disposable fixtures.
    env = {key: os.environ[key] for key in ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "SystemDrive")
           if key in os.environ}
    env.update({key: str(root) for key in ("HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "TMPDIR", "TMP", "TEMP")})
    env.update(PYTHONDONTWRITEBYTECODE="1", PYTHONNOUSERSITE="1")
    return env


class MigrationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.source = self.root / "old"
        self.destination = self.root / "new"

    def put(self, root, name, data=b"record\x00\xff", mode=0o600):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(mode)
        return path

    def plan(self, tool="pira_dec"):
        return migration.plan_migration(tool, [self.source], self.destination)

    def test_nonempty_identical_disjoint_and_idempotent_preserve_originals(self):
        source = self.put(self.source, "workspace/records/one.piradec", mode=0o400)
        self.put(self.source, "workspace/records/two.piradec", b"two")
        existing = self.put(self.destination, "workspace/records/one.piradec", mode=0o400)
        self.put(self.destination, "other/records/three.piradec", b"three")
        original = migration.inventory(self.source)
        inode = source.stat().st_ino
        destination_inode = existing.stat().st_ino
        plan = self.plan()
        before = sorted(self.root.rglob("*"))
        migration.apply_migrations([plan], dry_run=True)
        self.assertEqual(sorted(self.root.rglob("*")), before)
        with self.assertRaisesRegex(RuntimeError, "verification"):
            migration.apply_migrations([plan], verify=True)
        migration.apply_migrations([plan])
        migration.apply_migrations([plan])  # unified setup then standalone application
        migration.apply_migrations([self.plan()], verify=True)
        self.assertEqual(migration.inventory(self.source), original)
        self.assertEqual(source.stat().st_ino, inode)
        self.assertEqual(existing.stat().st_ino, destination_inode)
        self.assertEqual((self.destination / "workspace/records/two.piradec").read_bytes(), b"two")
        self.assertEqual(len(migration.inventory(self.destination)), 3)

    def test_conflicting_content_or_modes_and_tree_collision_fail_before_writes(self):
        self.put(self.source, "record", b"source")
        for content, mode in ((b"different", 0o600), (b"source", 0o400)):
            self.put(self.destination, "record", content, mode)
            before = migration.inventory(self.destination)
            with self.assertRaisesRegex(RuntimeError, "Conflicting"):
                self.plan()
            self.assertEqual(migration.inventory(self.destination), before)
        (self.destination / "record").chmod(0o600)  # clear Windows read-only fixture flag
        (self.destination / "record").unlink()
        self.put(self.destination, "record/child")
        with self.assertRaisesRegex(RuntimeError, "collision"):
            self.plan()

    def test_two_sources_disjoint_merge_and_same_identity_conflict(self):
        other = self.root / "other"
        self.put(self.source, "a")
        self.put(other, "b")
        migration.apply_migrations([migration.plan_migration("pira_dec", [self.source, other], self.destination)])
        self.assertEqual(set(migration.inventory(self.destination)), {"a", "b"})
        self.put(other, "a", b"conflict")
        with self.assertRaisesRegex(RuntimeError, "Conflicting"):
            migration.plan_migration("pira_dec", [self.source, other], self.destination)

    def test_interrupted_staging_and_publication_recover_without_replacing_records(self):
        self.put(self.source, "a", b"a")
        self.put(self.source, "b", b"b")
        plan = self.plan()
        real_link = migration.os.link
        calls = []
        def link(source, target):
            calls.append(target)
            if len(calls) == 2:
                raise OSError("injected interruption")
            real_link(source, target)
        with patch.object(migration.os, "link", side_effect=link):
            with self.assertRaisesRegex(OSError, "injected"):
                migration.apply_migrations([plan])
        first_inode = (self.destination / "a").stat().st_ino
        migration.apply_migrations([self.plan()])
        self.assertEqual((self.destination / "a").stat().st_ino, first_inode)
        self.assertEqual((self.destination / "b").read_bytes(), b"b")
        self.assertEqual((self.source / "a").read_bytes(), b"a")

    def test_source_change_and_corrupt_stage_fail_before_publication(self):
        source = self.put(self.source, "record", b"before")
        plan = self.plan()
        source.write_bytes(b"after")
        with self.assertRaisesRegex(RuntimeError, "changed"):
            migration.apply_migrations([plan])
        self.assertFalse(self.destination.exists())
        plan = self.plan()
        def corrupt(original, output, length):
            output.write(b"corrupt")
        with patch.object(migration.shutil, "copyfileobj", side_effect=corrupt):
            with self.assertRaisesRegex(RuntimeError, "verification"):
                migration.apply_migrations([plan])
        self.assertFalse(self.destination.exists())
        migration.apply_migrations([self.plan()])
        self.assertEqual((self.destination / "record").read_bytes(), b"after")

    def test_source_change_during_copy_blocks_configuration(self):
        source = self.put(self.source, "record", b"before")
        plan = self.plan()
        original_copy = migration.shutil.copyfileobj
        def change(original, output, length):
            original_copy(original, output, length)
            source.write_bytes(b"changed while copying")
        with patch.object(migration.shutil, "copyfileobj", side_effect=change):
            with self.assertRaisesRegex(RuntimeError, "Source changed"):
                migration.apply_migrations([plan])
        self.assertFalse(self.destination.exists())

    def test_symlinks_special_files_and_overlapping_roots_rejected(self):
        self.source.mkdir()
        (self.source / "link").symlink_to(self.root / "missing")
        with self.assertRaisesRegex(RuntimeError, "symlink"):
            self.plan()
        (self.source / "link").unlink()
        self.put(self.source, "record")
        with self.assertRaisesRegex(RuntimeError, "Overlapping"):
            migration.plan_migration("pira_dec", [self.source], self.source / "nested")
        self.destination.symlink_to(self.source, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, "symlink"):
            self.plan()
        if hasattr(os, "mkfifo"):
            self.destination.unlink()
            os.mkfifo(self.source / "pipe")
            with self.assertRaisesRegex(RuntimeError, "Unsupported"):
                self.plan()

    @unittest.skipIf(os.name == "nt", "Unix flock interaction; Windows native helper needs Windows runner")
    def test_real_existing_writer_leases_block_preflight_and_apply(self):
        import fcntl
        for tool, name in (("pira_dec", "workspace/.write.lock"),
                           ("pira_ctx", "live/owners/capture.lock"),
                           ("pira_ctx", "watch/owners/watch.lock"),
                           ("pira_ctx", "indexes/.index.owner-lock"),
                           ("pira_ctx", ".events/workspace.lock")):
            with self.subTest(tool=tool, name=name):
                lock = self.put(self.source, name, b"")
                plan = self.plan(tool)
                with lock.open("rb") as stream:
                    fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                    with self.assertRaisesRegex(RuntimeError, "Active"):
                        self.plan(tool)
                    with self.assertRaisesRegex(RuntimeError, "Active"):
                        migration.apply_migrations([plan])
                self.assertFalse(self.destination.exists())
                lock.unlink()

    def test_completed_ctx_only_preserves_unfinished_state_and_reruns(self):
        self.put(self.source, "live/pending.live.json", b"{}")
        self.put(self.source, "watch/state/paused.json", b'{"monitor":"paused"}')
        self.put(self.source, "record.piractx")
        self.put(self.source, "short-ids/workspace/session/reservation", b"record")
        original = migration.inventory(self.source)
        plan = migration.plan_migration("pira_ctx", [self.source], self.destination, completed_only=True)
        migration.apply_migrations([plan])
        self.assertEqual(set(migration.inventory(self.destination)),
                         {"record.piractx", "short-ids/workspace/session/reservation"})
        self.assertEqual(migration.inventory(self.source), original)
        self.put(self.destination, "later.piractx", b"destination-only")
        rerun = migration.plan_migration("pira_ctx", [self.source], self.destination, completed_only=True)
        migration.apply_migrations([rerun])
        self.assertEqual((self.destination / "later.piractx").read_bytes(), b"destination-only")
        if os.name != "nt":
            import fcntl
            lock = self.put(self.source, "live/owners/pending.lock", b"")
            with lock.open("rb") as stream:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                with self.assertRaisesRegex(RuntimeError, "Active"):
                    migration.plan_migration("pira_ctx", [self.source], self.destination, completed_only=True)

    def test_rejected_capture_exclusion_is_explicit_preserved_and_path_scoped(self):
        self.put(self.source, "valid.piractx")
        bad = self.put(self.source, "damaged.piractx", b"bad")
        plan = migration.plan_migration("pira_ctx", [self.source], self.destination,
                                        excluded_records=frozenset({bad.name}))
        migration.apply_migrations([plan])
        self.assertEqual(bad.read_bytes(), b"bad")
        self.assertFalse((self.destination / bad.name).exists())
        self.assertTrue((self.destination / "valid.piractx").exists())
        for name in ("../bad.piractx", "nested/bad.piractx", "nested\\bad.piractx", "records", ".piractx"):
            with self.subTest(name=name), self.assertRaises(RuntimeError):
                migration.plan_migration("pira_ctx", [self.source], self.destination,
                                         excluded_records=frozenset({name}))

    def test_explicit_setup_modes_route_without_inspecting_old_team(self):
        import setup_pira
        import setup_pira_tools
        for parser in (setup_pira.build_parser(), setup_pira_tools.build_parser()):
            args = parser.parse_args(["--completed-ctx-only", "--fresh-team"])
            self.assertTrue(args.completed_ctx_only and args.fresh_team)
        self.put(self.source, "live/pending.live.json", b"{}")
        for platform in dict.fromkeys((sys.platform, "win32")):
            registry = MagicMock()
            registry.OpenKey.side_effect = FileNotFoundError
            with self.subTest(platform=platform), \
                 patch.dict(os.environ, {"HOME": str(self.root), "LOCALAPPDATA": str(self.root)}, clear=True), \
                 patch.dict(sys.modules, {"winreg": registry}), \
                 patch.object(setup.sys, "platform", platform), \
                 patch.object(setup, "historical_store_paths", return_value=[self.source]), \
                 patch.object(migration, "preflight_team_relocation", side_effect=AssertionError("must not inspect")):
                plan = setup.plan_store_environment(["pira_ctx", "pira_team"], profile_paths=[],
                                                   completed_ctx_only=True, fresh_team=True)
                self.assertTrue(plan.migrations[0].completed_only)
                self.assertEqual(plan.team_migrations, [])
                if platform == "win32":
                    registry.OpenKey.assert_called_once()
                registry.SetValueEx.assert_not_called()

    def test_ctx_states_and_derived_indexes_do_not_hide_merged_records(self):
        state = self.put(self.source, "watch/state/id.json", b'{"monitor":"active"}')
        with self.assertRaisesRegex(RuntimeError, "watch"):
            self.plan("pira_ctx")
        state.write_text('{"monitor":"complete"}')
        live = self.put(self.source, "live/id.live.json", b"{}")
        with self.assertRaisesRegex(RuntimeError, "Unfinished"):
            self.plan("pira_ctx")
        live.unlink()
        self.put(self.source, "indexes/workspace.jsonl", b"old path")
        self.put(self.source, "record.piractx")
        index = self.put(self.destination, "indexes/workspace.jsonl", b"new path")
        self.put(self.destination, "indexes/.complete-v2", b"2\n")
        migration.apply_migrations([self.plan("pira_ctx")])
        self.assertEqual(index.read_bytes(), b"new path")
        self.assertFalse((self.destination / "indexes/.complete-v2").exists())
        self.assertFalse((self.destination / "indexes/.dirty-setup-migration").exists())
        backups = list(self.root.glob(".pira-migrate-*/index-complete-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), b"2\n")
        self.assertTrue((self.destination / "record.piractx").is_file())

    def test_event_cache_merge_invalidation_and_post_use_rerun(self):
        scope = ".events/workspace/.unscoped"
        event = scope + "/records/00000000000000000001-0000000001-0000000000000001.piraevt"
        caches = (".events/workspace/.retention.piraidx", scope + "/.catalog.piraidx")
        self.put(self.source, event, b"source event")
        for cache in caches:
            self.put(self.source, cache, b"old derived cache")
            self.put(self.destination, cache, b"new derived cache")
        self.put(self.destination, scope + "/records/other.piraevt", b"destination event")
        before = migration.inventory(self.source)
        migration.apply_migrations([self.plan("pira_ctx")])
        for cache in caches:
            self.assertFalse((self.destination / cache).exists())
            self.put(self.destination, cache, b"rebuilt cache after ordinary use")
        backups = list(self.root.glob(".pira-migrate-*/event-cache-*"))
        self.assertEqual(len(backups), 2)
        self.assertTrue(all(path.read_bytes() == b"new derived cache" for path in backups))
        used = migration.inventory(self.destination)
        migration.apply_migrations([self.plan("pira_ctx")])
        self.assertEqual(migration.inventory(self.destination), used)
        self.assertEqual(migration.inventory(self.source), before)
        # File extension alone never marks an authoritative record/handle derived.
        for name in (event, "short-ids/workspace/scope/suffix", "records/.catalog.piraidx"):
            with self.subTest(name=name):
                self.put(self.source, name, b"different authoritative content")
                self.put(self.destination, name, b"existing authoritative content")
                with self.assertRaisesRegex(RuntimeError, "Conflicting"):
                    self.plan("pira_ctx")
                (self.source / name).unlink()
                (self.destination / name).unlink()

    def test_native_ctx_environment_preserves_only_os_execution_inputs(self):
        fixture = {"PATH": "fixture-path", "SystemRoot": r"C:\Windows", "WINDIR": r"C:\Windows",
                   "COMSPEC": r"C:\Windows\System32\cmd.exe", "PATHEXT": ".COM;.EXE;.BAT;.CMD",
                   "SystemDrive": "C:", "HOME": "not-fixture", "TEMP": "not-fixture",
                   "CODEX_HOME": "do-not-inherit", "OPENAI_API_KEY": "synthetic-do-not-inherit",
                   "PIRA_CTX_THREAD_ID": "do-not-inherit"}
        with patch.dict(os.environ, fixture, clear=True):
            env = native_ctx_environment(self.root)
        for key in ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "SystemDrive"):
            self.assertEqual(env[key], fixture[key])
        for key in ("HOME", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "TMPDIR", "TMP", "TEMP"):
            self.assertEqual(env[key], str(self.root))
        for key in ("CODEX_HOME", "OPENAI_API_KEY", "PIRA_CTX_THREAD_ID"):
            self.assertNotIn(key, env)

    @unittest.skipUnless(os.environ.get("PIRA_TEST_CTX_BINARY"), "Set PIRA_TEST_CTX_BINARY for native-format controls")
    def test_native_ctx_event_merge_and_post_use_rerun(self):
        binary = os.environ["PIRA_TEST_CTX_BINARY"]
        env = native_ctx_environment(self.root)
        def run(store, *arguments):
            result = subprocess.run([binary, *arguments], cwd=self.root,
                                    env={**env, "PIRA_CTX_STORE_DIR": str(store)},
                                    text=True, capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result.stdout
        def event(store, intent):
            run(store, "exact", "--intent", intent, "--", sys.executable, "-c", "print('fixture')")
        event(self.source, "source-native-event")
        event(self.destination, "destination-native-event")
        original = migration.inventory(self.source)
        source_events = set(self.source.rglob("*.piraevt"))
        self.assertTrue(source_events)
        migration.apply_migrations([self.plan("pira_ctx")])
        history = run(self.destination, "history", "--scope", "workspace", "--limit", "20")
        self.assertIn("source-native-event", history)
        self.assertIn("destination-native-event", history)
        event(self.destination, "post-migration-native-event")
        migration.apply_migrations([self.plan("pira_ctx")])
        history = run(self.destination, "history", "--scope", "workspace", "--limit", "20")
        self.assertIn("source-native-event", history)
        self.assertIn("post-migration-native-event", history)
        self.assertEqual(migration.inventory(self.source), original)
        # A genuinely different native payload at the same event ID still conflicts.
        source_event = next(iter(source_events))
        source_event.write_bytes(b"conflicting event payload")
        with self.assertRaisesRegex(RuntimeError, "Conflicting"):
            self.plan("pira_ctx")

    def test_setup_discovers_legacy_and_copies_before_configuration(self):
        env = {"HOME": str(self.root), "SHELL": "/bin/sh"}
        with patch.dict(os.environ, env, clear=True), patch.object(setup.sys, "platform", "linux"):
            old = self.root / ".cache/pira/ctx"
            new = self.root / ".local/share/pira/ctx"
            self.put(old, "one.piractx")
            self.put(new, "two.piractx", b"disjoint")
            profile = self.root / "profile"
            profile.write_text(setup.STORE_BLOCK_START + "\nexport PIRA_CTX_STORE_DIR=" + setup.shlex.quote(str(old)) + "\n" + setup.STORE_BLOCK_END + "\n")
            initial = profile.read_bytes()
            plan = setup.plan_store_environment(["pira_ctx"], profile_paths=[profile])
            self.assertEqual(plan.stores["PIRA_CTX_STORE_DIR"], str(new))
            setup.apply_store_environment(plan, dry_run=True)
            self.assertEqual(profile.read_bytes(), initial)
            with patch.object(migration, "verify_destination", side_effect=RuntimeError("injected verify failure")):
                with self.assertRaisesRegex(RuntimeError, "injected"):
                    setup.apply_store_environment(plan, dry_run=False)
            self.assertEqual(profile.read_bytes(), initial)
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"], profile_paths=[profile]), dry_run=False)
            self.assertIn(str(new), profile.read_text())
            self.assertEqual((new / "one.piractx").read_bytes(), (old / "one.piractx").read_bytes())
            setup.apply_store_environment(setup.plan_store_environment(["pira_ctx"], profile_paths=[profile]), dry_run=True, verify=True)
            with patch.dict(os.environ, {"PIRA_CTX_STORE_DIR": str(self.root / "custom")}):
                custom = setup.plan_store_environment(["pira_ctx"], profile_paths=[profile])
                self.assertEqual(custom.migrations, [])
                self.assertEqual(custom.stores["PIRA_CTX_STORE_DIR"], str(self.root / "custom"))

    def test_absolute_nonempty_xdg_and_codex_only_migration(self):
        for xdg in ("", "relative", str(self.root / "data")):
            env = {"HOME": str(self.root), "XDG_DATA_HOME": xdg}
            with self.subTest(xdg=xdg), patch.dict(os.environ, env, clear=True), patch.object(setup.sys, "platform", "linux"):
                parent = Path(xdg) if Path(xdg).is_absolute() else self.root / ".local/share"
                selected = setup.selected_store_paths(["pira_ctx", "pira_dec", "pira_team"])
                self.assertEqual(selected["PIRA_CTX_STORE_DIR"], str(parent / "pira/ctx"))
                self.assertEqual(selected["PIRA_DEC_STORE_DIR"], str(parent / "pira/decision"))
        with patch.dict(os.environ, {"HOME": str(self.root), "PIRA_CTX_STORE_DIR": str(self.root / "custom")}, clear=True), patch.object(setup.sys, "platform", "linux"):
            old = self.root / ".cache/pira/ctx"
            self.put(old, "one.piractx")
            text = 'shell_environment_policy.set.PIRA_CTX_STORE_DIR=' + json.dumps(str(old))
            plan = setup.plan_store_environment(["pira_ctx"], profile_paths=[], codex_text=text)
            self.assertEqual(len(plan.migrations), 1)
            setup.apply_store_migrations(plan, dry_run=False)
            changed = setup.codex_store_configuration(text, ["pira_ctx"], plan.stores)
            self.assertEqual(tomllib.loads(changed)["shell_environment_policy"]["set"]["PIRA_CTX_STORE_DIR"],
                             str(self.root / ".local/share/pira/ctx"))
            with self.assertRaisesRegex(RuntimeError, "TOML"):
                setup.plan_store_environment(["pira_ctx"], profile_paths=[], codex_text="[broken")

    def test_explicit_custom_store_requires_no_home_or_default(self):
        with patch.dict(os.environ, {"PIRA_TEAM_DIR": str(self.root / "custom-team")}, clear=True), \
             patch.object(setup.sys, "platform", "linux"):
            plan = setup.plan_store_environment(["pira_team"], profile_paths=[])
            self.assertEqual(plan.stores, {"PIRA_TEAM_DIR": str(self.root / "custom-team")})

    def team_manifest(self, root, **changes):
        manifest = {"schema_version": 4, "run_id": root.name, "status": "completed",
                    "active_turn": None, "thread_id": "native-fixture", "transport": "app-server",
                    "artifact": "artifacts/handoff", "result": str(root / "artifacts/handoff"),
                    "logs": str(root / "logs"),
                    "review_checkpoint": str(root / "artifacts/review.md"),
                    "cwd": str(root / "user-workspace"), "ctx_store": "/custom/ctx",
                    "phase_instructions": "history " + str(root / "artifacts/handoff"),
                    "attempts": [{"logs": str(root / "logs/attempt"), "candidate": None}],
                    "revisions": [{"manifest": "revisions/000001/manifest.json", "artifact": None}]}
        manifest.update(changes)
        return json.dumps(manifest).encode()

    def test_team_known_fields_transform_without_rewriting_history(self):
        old = self.source / "run-id"
        new = self.destination / "run-id"
        for schema in (3, 4):
            original = self.team_manifest(old, schema_version=schema)
            transformed = migration.transform_team_manifest(original, old, new)
            before = json.loads(original)
            after = json.loads(transformed)
            self.assertEqual(after["result"], str(new / "artifacts/handoff"))
            self.assertEqual(after["attempts"][0]["logs"], str(new / "logs/attempt"))
            for key in ("cwd", "ctx_store", "phase_instructions", "artifact", "revisions", "thread_id"):
                self.assertEqual(after[key], before[key])
            self.assertEqual(migration.transform_team_manifest(transformed, new, new), transformed)
        for changes in ({"schema_version": 3.0}, {"status": {}}, {"schema_version": 99}, {"run_id": "wrong"}, {"status": "running"},
                        {"active_turn": "active"}, {"result": str(old) + "-lookalike/artifacts/handoff"},
                        {"result": str(old / "artifacts/../handoff")}, {"artifact": "../outside"},
                        {"attempts": [{"logs": 3}]}, {"result": str(old / "codex-home/history")}):
            with self.subTest(changes=changes), self.assertRaises(RuntimeError):
                migration.transform_team_manifest(self.team_manifest(old, **changes), old, new)

    def test_team_first_revision_root_outputs_are_field_scoped(self):
        old = self.source / "run-id"
        new = self.destination / "run-id"
        for directory in (Path(), Path("revisions/000002")):
            with self.subTest(directory=directory):
                raw = self.team_manifest(old, logs=str(old / directory), attempts=[{
                    "logs": str(old / directory),
                    "candidate": str(old / directory / "candidate.txt"),
                    "diagnostics": str(old / directory / "validation.json"),
                }])
                result = json.loads(migration.transform_team_manifest(raw, old, new))
                self.assertEqual(result["logs"], str(new / directory))
                self.assertEqual(result["attempts"][0], {
                    "logs": str(new / directory),
                    "candidate": str(new / directory / "candidate.txt"),
                    "diagnostics": str(new / directory / "validation.json"),
                })
        for field, path in (("candidate", old), ("candidate", old / "auth.json"),
                            ("logs", old / "candidate.txt"),
                            ("diagnostics", old / "candidate.txt")):
            with self.subTest(field=field, path=path), self.assertRaises(RuntimeError):
                migration.transform_team_manifest(
                    self.team_manifest(old, attempts=[{field: str(path)}]), old, new)

    @patch('team_store_relocation._VALIDATED_PLATFORMS', frozenset())
    def test_team_same_id_compares_whole_transformed_history_and_omits_capabilities(self):
        old = self.source / "run-id"
        new = self.destination / "run-id"
        original = self.team_manifest(old)
        self.put(old, "manifest.json", original)
        self.put(old, "artifacts/handoff", b"unchanged history")
        self.put(old, "revisions/000001/manifest.json", original)
        (old / "codex-home").mkdir()
        (old / "codex-home/auth.json").symlink_to(old / "missing-secret")
        self.put(old, "control.json", b"not a portable capability")
        transformed = migration.transform_team_manifest(original, old, new)
        self.put(new, "manifest.json", transformed)
        self.put(new, "artifacts/handoff", b"unchanged history")
        self.put(new, "revisions/000001/manifest.json", transformed)
        self.assertEqual(migration.team_history(old, new), migration.team_history(new, new))
        with self.assertRaisesRegex(RuntimeError, "native relocation blocked"):
            migration.preflight_team_relocation([self.source], self.destination)
        self.put(new, "artifacts/handoff", b"divergent history")
        with self.assertRaisesRegex(RuntimeError, "Conflicting Team same-ID"):
            migration.preflight_team_relocation([self.source], self.destination)
        self.assertEqual((old / "manifest.json").read_bytes(), original)
        self.assertTrue((old / "codex-home/auth.json").is_symlink())

    def test_team_receipt_allows_only_verified_destination_lineage(self):
        old = self.source / "run-id"
        new = self.destination / "run-id"
        original = self.team_manifest(old)
        self.put(old, "manifest.json", original)
        self.put(old, "history", b"original retained bytes")
        self.put(new, "manifest.json", migration.transform_team_manifest(original, old, new))
        history = self.put(new, "history", b"original retained bytes")
        lock = self.put(old, "run.lock", b"")
        source_snapshot = migration.inventory(old)
        with migration.lease(lock):
            fingerprint = migration.team_source_fingerprint(old)
        # A synthetic helper double, NOT evidence of actual native relocation.
        identity = {"run_id": old.name, "thread_id": "native-fixture"}
        def native(run):
            self.assertEqual(run, new)
            if not history.is_file():
                raise RuntimeError("missing native retained history")
            return identity
        receipt = migration.team_migration_receipt(
            old, new, expected_source=fingerprint, validate_native_identity=native)
        history.write_bytes(b"legitimate destination-only continuation")
        self.put(new, "revisions/000002/handoff", b"new revision")
        before = migration.inventory(new)
        migration.validate_team_migration_receipt(receipt, old, new, validate_native_identity=native)
        self.assertEqual(migration.inventory(new), before)
        self.assertEqual(migration.inventory(old), source_snapshot)
        for invalid in (None, b"{", b"{}", receipt.replace(b'"schema": 1', b'"schema": true'),
                        receipt.replace(b'"schema": 1', b'"schema": 1, "schema": 1')):
            with self.subTest(invalid=invalid), self.assertRaises(RuntimeError):
                migration.validate_team_migration_receipt(invalid, old, new, validate_native_identity=native)
        with self.assertRaisesRegex(RuntimeError, "identity"):
            migration.validate_team_migration_receipt(receipt, old, new,
                validate_native_identity=lambda run: {**identity, "thread_id": "wrong-thread"})
        history.unlink()
        with self.assertRaisesRegex(RuntimeError, "missing native"):
            migration.validate_team_migration_receipt(receipt, old, new, validate_native_identity=native)
        history.write_bytes(b"restored synthetic continuation")
        manifest = (new / "manifest.json").read_bytes()
        self.put(new, "manifest.json", self.team_manifest(new, thread_id="replacement-thread"))
        with self.assertRaisesRegex(RuntimeError, "identity"):
            migration.validate_team_migration_receipt(receipt, old, new, validate_native_identity=native)
        (new / "manifest.json").unlink()
        with self.assertRaises((RuntimeError, OSError)):
            migration.validate_team_migration_receipt(receipt, old, new, validate_native_identity=native)
        self.put(new, "manifest.json", manifest)
        self.put(old, "history", b"changed retained SOURCE")
        with self.assertRaisesRegex(RuntimeError, "source changed"):
            migration.validate_team_migration_receipt(receipt, old, new, validate_native_identity=native)
        with self.assertRaisesRegex(RuntimeError, "source changed"):
            migration.team_migration_receipt(old, new, expected_source=fingerprint,
                                             validate_native_identity=native)

    def metadata_plan(self):
        with patch.dict(os.environ, {"HOME": str(self.root)}, clear=True), \
             patch.object(setup.sys, "platform", "linux"), \
             patch.object(setup, "historical_store_paths", return_value=[self.source]):
            self.destination = self.root / ".local/share/pira/team"
            return setup.plan_store_environment(["pira_team"], profile_paths=[], codex_binary="/inert/fixture-codex")

    def test_team_metadata_only_merge_and_post_use_rerun(self):
        self.put(self.source, "bookmarks.json", b'[1, [["same", "run"], ["source", "old-run"]]]')
        self.put(self.source, "bookmarks.lock", b"")
        self.put(self.source, "worker_defaults.json", b'{"main-old": {"model": "worker", "effort": "high"}}')
        self.put(self.source, "worker_defaults.lock", b"")
        self.destination = self.root / ".local/share/pira/team"
        self.put(self.destination, "bookmarks.json", b'[1, [["same", "run"], ["target", "new-run"]]]')
        original = migration.inventory(self.source, tool="pira_team")
        plan = self.metadata_plan()
        before = sorted(self.root.rglob("*"))
        setup.apply_store_migrations(plan, dry_run=True)
        self.assertEqual(sorted(self.root.rglob("*")), before)
        with self.assertRaisesRegex(RuntimeError, "metadata"):
            setup.apply_store_migrations(plan, dry_run=True, verify=True)
        setup.apply_store_migrations(plan, dry_run=False)
        bookmarks = self.destination / "bookmarks.json"
        self.assertEqual(dict(json.loads(bookmarks.read_bytes())[1]),
                         {"same": "run", "source": "old-run", "target": "new-run"})
        defaults = self.destination / "worker_defaults.json"
        self.assertEqual(json.loads(defaults.read_bytes()), {"main-old": {"model": "worker", "effort": "high"}})
        self.assertFalse(os.path.samefile(defaults, self.source / "worker_defaults.json"))
        # New destination-only keys and formatting survive ordinary reruns.
        bookmarks.write_bytes(b'[1, [["same", "run"], ["source", "old-run"], ["target", "new-run"], ["later", "later-run"]]]')
        used = migration.inventory(self.destination, tool="pira_team")
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        setup.apply_store_migrations(self.metadata_plan(), dry_run=True, verify=True)
        self.assertEqual(migration.inventory(self.destination, tool="pira_team"), used)
        self.assertEqual(migration.inventory(self.source, tool="pira_team"), original)
        # Remapped imported keys, removed keys and added defaults are local use,
        # not permission to replay the retained original maps.
        bookmarks.write_bytes(b'[1, [["same", "remapped"], ["later", "later-run"]]]')
        defaults.write_bytes(b'{"main-old": {"model": "custom", "effort": "low"}, "new-main": {"model": "new", "effort": "high"}}')
        used = migration.inventory(self.destination, tool="pira_team")
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        setup.apply_store_migrations(self.metadata_plan(), dry_run=True, verify=True)
        self.assertEqual(migration.inventory(self.destination, tool="pira_team"), used)
        bookmarks.unlink()
        defaults.unlink()
        used = migration.inventory(self.destination, tool="pira_team")
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        setup.apply_store_migrations(self.metadata_plan(), dry_run=True, verify=True)
        self.assertEqual(migration.inventory(self.destination, tool="pira_team"), used)
        self.assertEqual(migration.inventory(self.source, tool="pira_team"), original)

    def test_team_metadata_conflicts_and_malformed_maps_preserve_stores(self):
        self.destination = self.root / ".local/share/pira/team"
        for name, source, target in (
                ("bookmarks.json", b'[1, [["label", "old"]]]', b'[1, [["label", "new"]]]'),
                ("worker_defaults.json", b'{"main": {"model": "old", "effort": "high"}}', b'{"main": {"model": "new", "effort": "high"}}')):
            with self.subTest(name=name):
                self.put(self.source, name, source)
                self.put(self.destination, name, target)
                before = migration.inventory(self.destination)
                with self.assertRaisesRegex(RuntimeError, "Conflicting Team metadata"):
                    self.metadata_plan()
                self.assertEqual(migration.inventory(self.destination), before)
                (self.source / name).unlink()
                (self.destination / name).unlink()
        malformed = [("bookmarks.json", b'[1, [["same", "a"], ["same", "b"]]]'),
                     ("bookmarks.json", b'[true, []]'),
                     ("bookmarks.json", b'[1, [["label", "../outside"]]]'),
                     ("worker_defaults.json", b'{"main": {"model": "m", "effort": "unknown"}}'),
                     ("worker_defaults.json", b'{"main": {}, "main": {}}')]
        for name, data in malformed:
            with self.subTest(data=data):
                self.put(self.source, name, data)
                with self.assertRaisesRegex(RuntimeError, "metadata"):
                    self.metadata_plan()
                self.assertFalse((self.destination / name).exists())
                (self.source / name).unlink()

    def test_team_metadata_with_retained_run_and_interrupted_publish(self):
        self.put_team_run(self.source / "run-id")
        self.put(self.source, "bookmarks.json", b'[1, [["retained", "run-id"]]]')
        helper = self.fake_team_backend()
        with patch.object(migration, "team_backend", return_value=helper):
            plan = self.metadata_plan()
            self.assertEqual(len(plan.team_migrations), 1)
            with patch.object(migration.os, "replace", side_effect=OSError("interrupted")):
                with self.assertRaises(OSError):
                    setup.apply_store_migrations(plan, dry_run=False)
            # Retry from a new preflight: original source and target histories survive.
            setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
            self.assertEqual(json.loads((self.destination / "bookmarks.json").read_bytes()), [1, [["retained", "run-id"]]])
            self.assertTrue((self.destination / "run-id/artifacts/handoff").exists())
            self.assertTrue((self.source / "run-id/artifacts/handoff").exists())

    def test_team_metadata_interrupted_merge_backup_and_retry(self):
        self.destination = self.root / ".local/share/pira/team"
        self.put(self.source, "bookmarks.json", b'[1, [["source", "old"]]]')
        target = self.put(self.destination, "bookmarks.json", b'[1, [["target", "new"]]]')
        original = target.read_bytes()
        plan = self.metadata_plan()
        replace = os.replace
        def interrupt(source, destination):
            if Path(destination) == target:
                raise OSError("interrupted metadata publication")
            return replace(source, destination)
        with patch.object(migration.os, "replace", side_effect=interrupt):
            with self.assertRaises(OSError):
                setup.apply_store_migrations(plan, dry_run=False)
        self.assertEqual(target.read_bytes(), original)
        self.assertEqual(target.stat().st_nlink, 1)
        backups = list(self.destination.parent.glob(".pira-team-metadata-*/previous-*"))
        self.assertEqual([p.read_bytes() for p in backups], [original])
        self.assertFalse(os.path.samefile(backups[0], target))
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        self.assertEqual(dict(json.loads(target.read_bytes())[1]), {"source": "old", "target": "new"})
        self.assertEqual((self.source / "bookmarks.json").read_bytes(), b'[1, [["source", "old"]]]')

    def test_team_metadata_receipt_rejects_drift_invalid_and_replaced_roots(self):
        source = self.put(self.source, "bookmarks.json", b'[1, [["source", "old"]]]')
        original = source.read_bytes()
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        plan = self.metadata_plan().team_metadata_migrations[0]
        receipt = plan.ledger / "state.json"
        valid = receipt.read_bytes()
        destination_before = migration.inventory(self.destination, tool="pira_team")
        source.write_bytes(b'[1, [["source", "changed"]]]')
        with self.assertRaisesRegex(RuntimeError, "receipt|changed"):
            self.metadata_plan()
        source.write_bytes(original)
        for data in (b'{', valid.replace(b'"schema": 1', b'"schema": true'),
                     valid.replace(b'"complete"', b'"unknown"'), b'{"schema":1,"schema":1}'):
            with self.subTest(data=data):
                receipt.write_bytes(data)
                with self.assertRaisesRegex(RuntimeError, "receipt"):
                    self.metadata_plan()
                receipt.write_bytes(valid)
        receipt.unlink()
        with self.assertRaisesRegex(RuntimeError, "Missing.*receipt"):
            self.metadata_plan()
        receipt.write_bytes(valid)
        for root in (self.source, self.destination):
            with self.subTest(root=root):
                saved = root.with_name(root.name + "-original")
                root.rename(saved)
                shutil.copytree(saved, root)
                with self.assertRaisesRegex(RuntimeError, "identity changed"):
                    self.metadata_plan()
                shutil.rmtree(root)
                saved.rename(root)
        self.assertEqual(migration.inventory(self.destination, tool="pira_team"), destination_before)

    def test_team_metadata_partial_publication_cannot_mark_complete(self):
        self.put(self.source, "bookmarks.json", b'[1, [["source", "old"]]]')
        self.put(self.source, "worker_defaults.json", b'{"main": {"model": "worker", "effort": "high"}}')
        original = migration.inventory(self.source, tool="pira_team")
        plan = self.metadata_plan()
        replace = os.replace
        def interrupt(source, destination):
            if Path(destination).name == "worker_defaults.json":
                raise OSError("second map interrupted")
            return replace(source, destination)
        with patch.object(migration.os, "replace", side_effect=interrupt):
            with self.assertRaises(OSError):
                setup.apply_store_migrations(plan, dry_run=False)
        receipt = plan.team_metadata_migrations[0].ledger / "state.json"
        self.assertEqual(json.loads(receipt.read_bytes())["phase"], "publishing")
        self.assertTrue((self.destination / "bookmarks.json").exists())
        self.assertFalse((self.destination / "worker_defaults.json").exists())
        pending = receipt.read_bytes()
        receipt.write_bytes(pending.replace(b'"publishing"', b'"complete"'))
        with self.assertRaisesRegex(RuntimeError, "receipt"):
            self.metadata_plan()
        receipt.write_bytes(pending)
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            setup.apply_store_migrations(self.metadata_plan(), dry_run=True, verify=True)
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        self.assertEqual(json.loads(receipt.read_bytes())["phase"], "complete")
        self.assertEqual(json.loads((self.destination / "worker_defaults.json").read_bytes()),
                         {"main": {"model": "worker", "effort": "high"}})
        self.assertEqual(migration.inventory(self.source, tool="pira_team"), original)

    def test_team_metadata_completion_write_failure_requires_verified_retry(self):
        self.put(self.source, "bookmarks.json", b'[1, [["source", "old"]]]')
        plan = self.metadata_plan()
        replace = os.replace
        def interrupt(source, destination):
            if Path(destination).name == "state.json" and json.loads(Path(source).read_bytes())["phase"] == "complete":
                raise OSError("completion receipt interrupted")
            return replace(source, destination)
        with patch.object(migration.os, "replace", side_effect=interrupt):
            with self.assertRaises(OSError):
                setup.apply_store_migrations(plan, dry_run=False)
        receipt = plan.team_metadata_migrations[0].ledger / "state.json"
        self.assertEqual(json.loads(receipt.read_bytes())["phase"], "publishing")
        target = self.destination / "bookmarks.json"
        published = target.read_bytes()
        target.write_bytes(b'[1, [["local", "new"]]]')
        with self.assertRaisesRegex(RuntimeError, "Interrupted.*changed"):
            self.metadata_plan()
        target.write_bytes(published)
        retry = self.metadata_plan()
        self.assertEqual(retry.team_metadata_migrations[0].files, {})
        with self.assertRaisesRegex(RuntimeError, "incomplete"):
            setup.apply_store_migrations(retry, dry_run=True, verify=True)
        setup.apply_store_migrations(retry, dry_run=False)
        self.assertEqual(json.loads(receipt.read_bytes())["phase"], "complete")
        target.write_bytes(b'[1, []]')
        setup.apply_store_migrations(self.metadata_plan(), dry_run=False)
        self.assertEqual(target.read_bytes(), b'[1, []]')

    def test_team_metadata_intent_failure_preserves_maps_without_completion(self):
        self.put(self.source, "bookmarks.json", b'[1, [["source", "old"]]]')
        plan = self.metadata_plan()
        with patch.object(migration.os, "replace", side_effect=OSError("intent publication failed")):
            with self.assertRaises(OSError):
                setup.apply_store_migrations(plan, dry_run=False)
        receipt = plan.team_metadata_migrations[0].ledger / "state.json"
        self.assertFalse(receipt.exists())
        self.assertFalse((self.destination / "bookmarks.json").exists())
        self.assertEqual((self.source / "bookmarks.json").read_bytes(), b'[1, [["source", "old"]]]')
        with self.assertRaisesRegex(RuntimeError, "Missing.*receipt"):
            self.metadata_plan()

    @unittest.skipIf(os.name == "nt", "POSIX lock interaction")
    def test_team_metadata_active_lock_and_stale_plan_block_switch(self):
        import fcntl
        self.put(self.source, "bookmarks.json", b'[1, []]')
        lock = self.put(self.source, "bookmarks.lock", b"")
        with lock.open("rb") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(RuntimeError, "lease"):
                self.metadata_plan()
        plan = self.metadata_plan()
        (self.source / "bookmarks.json").write_bytes(b'[1, [["late", "run"]]]')
        with self.assertRaisesRegex(RuntimeError, "changed"):
            setup.apply_store_migrations(plan, dry_run=False)
        self.assertFalse(self.destination.exists())

    def fake_team_backend(self):
        """EXPLICIT native/admission double: caller-order evidence, not native proof."""
        helper = MagicMock()
        helper.capabilities.return_value = {"admitted": True}
        helper.inspect_source.return_value = {"admitted": True}
        helper.recover.return_value = {"source_restored": True}
        def validate(source, target, run_id, thread_id):
            self.assertEqual(migration.team_run_identity(target), {"run_id": run_id, "thread_id": thread_id})
            if not (target / "codex-home/native").read_bytes().startswith(b"native-original"):
                raise RuntimeError("lost original native lineage")
            return dict(run_id=run_id, thread_id=thread_id, source_fingerprint="native-source-fingerprint")
        helper.validate_identity.side_effect = validate
        def preflight(source, target, run_id, thread_id, **kwargs):
            self.assertEqual((source / "codex-home/native").read_bytes(), (target / "codex-home/native").read_bytes())
            self.assertFalse(os.path.samefile(source / "codex-home/native", target / "codex-home/native"))
            self.assertEqual(target.name, run_id)
            self.assertNotIn(target, kwargs["journal_path"].parents)
            self.assertNotIn(source, kwargs["journal_path"].parents)
            self.assertEqual(json.loads((target / "manifest.json").read_bytes())["result"], str(target / "artifacts/handoff"))
            return {"admitted": True}
        helper.preflight.side_effect = preflight
        def repair(source, target, run_id, thread_id, **kwargs):
            kwargs["journal_path"].write_text("synthetic helper journal")
            with (target / "codex-home/native").open("ab") as stream:
                stream.write(b" repaired-at-final-path")
            return dict(run_id=run_id, thread_id=thread_id, source_run=str(source), destination_run=str(target),
                        destination=str(target / "codex-home"), source_fingerprint="native-source-fingerprint",
                        source_restored=True, history_prefixes={"turns": {"count": 2, "sha256": "a" * 64}})
        helper.repair.side_effect = repair
        helper.verify_native_identity.return_value = {"native_verified": True}
        return helper

    def put_team_run(self, root):
        self.put(root, "manifest.json", self.team_manifest(root))
        self.put(root, "codex-home/native", b"native-original")
        self.put(root, "artifacts/handoff", b"original handoff")
        self.put(root, "run.lock", b"")

    def test_team_public_setup_receipt_barrier_and_destination_evolution(self):
        helper = self.fake_team_backend()
        with patch.object(migration, "team_backend", return_value=helper), \
             patch.object(migration.shutil, "which", return_value="synthetic-codex"), \
             patch.dict(os.environ, {"HOME": str(self.root)}, clear=True), \
             patch.object(setup.sys, "platform", "linux"), \
             patch.object(setup.tempfile, "gettempdir", return_value=str(self.root / "temporary")):
            source = setup.historical_store_paths("pira_team")[0] / "run-id"
            destination = self.root / ".local/share/pira/team"
            target = destination / source.name
            self.put_team_run(source)
            self.put_team_run(destination / "unrelated-run")
            unrelated = migration.inventory(destination / "unrelated-run")
            original = migration.inventory(source)
            profile = self.root / "profile"
            profile.write_bytes(b"# unchanged until verified\n")
            plan = setup.plan_store_environment(["pira_team"], profile_paths=[profile])
            self.assertEqual(len(plan.team_migrations), 1)
            setup.apply_store_environment(plan, dry_run=True)
            self.assertFalse(plan.team_migrations[0].ledger.exists())
            with self.assertRaisesRegex(RuntimeError, "incomplete"):
                setup.apply_store_migrations(plan, dry_run=True, verify=True)
            real_write = setup.write_profile
            def barrier(path, text, dry_run):
                state = migration.read_team_state(plan.team_migrations[0])
                self.assertEqual(state["phase"], "complete")
                self.assertEqual(helper.repair.call_count, 1)
                return real_write(path, text, dry_run)
            with patch.object(setup, "write_profile", side_effect=barrier):
                setup.apply_store_environment(plan, dry_run=False)
            self.assertEqual(migration.inventory(source), original)
            self.assertEqual(migration.inventory(destination / "unrelated-run"), unrelated)
            helper.verify_native_identity.assert_not_called()
            with (target / "codex-home/native").open("ab") as stream:
                stream.write(b" legitimate destination-only continuation")
            self.put(target, "revisions/000002/handoff", b"new handoff")
            before = migration.inventory(target)
            again = setup.plan_store_environment(["pira_team"], profile_paths=[profile])
            setup.apply_store_environment(again, dry_run=True, verify=True)
            self.assertEqual(migration.inventory(target), before)
            helper.verify_native_identity.assert_not_called()
            setup.apply_store_migrations(again, dry_run=False)
            setup.apply_store_environment(again, dry_run=False)
            self.assertEqual(helper.repair.call_count, 1)
            self.assertEqual(helper.preflight.call_count, 1)
            self.assertEqual(helper.verify_native_identity.call_count, 1)
            self.assertEqual(helper.verify_native_identity.call_args.kwargs["evidence"]["history_prefixes"],
                             {"turns": {"count": 2, "sha256": "a" * 64}})
            self.assertEqual(migration.inventory(target), before)
            self.put(source, "artifacts/handoff", b"changed original")
            with self.assertRaisesRegex(RuntimeError, "source changed"):
                setup.plan_store_environment(["pira_team"], profile_paths=[profile])

    def test_team_selected_backend_outside_path_reaches_repair_and_receipt_verification(self):
        helper = self.fake_team_backend()  # admission/native double, no executable launched
        selected = self.root / "managed backend" / "codex.exe"
        with patch.object(migration, "team_backend", return_value=helper), \
             patch.object(migration.shutil, "which", side_effect=AssertionError("explicit selection must not search PATH")), \
             patch.dict(os.environ, {"HOME": str(self.root), "PATH": "synthetic-no-backend"}, clear=True), \
             patch.object(setup.sys, "platform", "linux"), \
             patch.object(setup.tempfile, "gettempdir", return_value=str(self.root / "temporary")):
            initial_env = dict(os.environ)
            # No retained Team data: no backend validation or discovery needed.
            fresh = setup.plan_store_environment(["pira_team"], profile_paths=[], codex_binary=selected)
            self.assertEqual(fresh.team_migrations, [])
            helper.inspect_source.assert_not_called()
            absent = setup.plan_store_environment([], profile_paths=[], codex_binary=selected)
            self.assertEqual(absent.team_migrations, [])
            source = setup.historical_store_paths("pira_team")[0] / "run-id"
            self.put_team_run(source)
            with self.assertRaisesRegex(RuntimeError, "selected supported Codex"):
                setup.plan_store_environment(["pira_team"], profile_paths=[], codex_binary="")
            plan = setup.plan_store_environment(["pira_team"], profile_paths=[], codex_binary=selected)
            self.assertEqual(plan.team_migrations[0].binary, str(selected))
            setup.apply_store_migrations(plan, dry_run=False)
            again = setup.plan_store_environment(["pira_team"], profile_paths=[], codex_binary=selected)
            setup.apply_store_migrations(again, dry_run=False)
            for operation in (helper.inspect_source, helper.preflight, helper.repair, helper.verify_native_identity):
                self.assertTrue(operation.called)
                for call in operation.call_args_list:
                    self.assertEqual(call.kwargs["codex_binary"], str(selected))
            self.assertEqual(helper.repair.call_count, 1)  # continued receipt path, not first repair again
            self.assertEqual(helper.verify_native_identity.call_count, 1)
            self.assertEqual(dict(os.environ), initial_env)

    def test_team_pre_receipt_conflicts_equal_target_and_invalid_receipt(self):
        helper = self.fake_team_backend()
        source, target = self.source / "run-id", self.destination / "run-id"
        self.put_team_run(source)
        self.put_team_run(target)
        # Preserve nonmanaged history strings exactly, rebasing only known fields.
        self.put(target, "manifest.json", migration.transform_team_manifest((source / "manifest.json").read_bytes(), source, target))
        with patch.object(migration, "team_backend", return_value=helper):
            self.put(target, "artifacts/handoff", b"divergent")
            before = migration.inventory(self.destination)
            with self.assertRaisesRegex(RuntimeError, "Conflicting Team"):
                migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")
            self.assertEqual(migration.inventory(self.destination), before)
            self.put(target, "artifacts/handoff", b"original handoff")
            (target / "run.lock").unlink()  # preexisting idle run may not have a lease file yet
            plans = migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")
            migration.apply_team_migrations(plans)
            self.assertEqual((plans[0].ledger / "attempt-000001/previous/artifacts/handoff").read_bytes(), b"original handoff")
            state = plans[0].ledger / "state.json"
            state.write_bytes(b"invalid receipt")
            with self.assertRaisesRegex(RuntimeError, "Invalid Team"):
                migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")
            state.unlink()
            with self.assertRaisesRegex(RuntimeError, "Missing Team"):
                migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")

    def test_team_failed_and_interrupted_repair_recovers_then_retries_independent_copy(self):
        for interrupted in (False, True):
            with self.subTest(interrupted=interrupted):
                source_root = self.root / f"source-{interrupted}"
                target_root = self.root / f"target-{interrupted}"
                source, target = source_root / "run-id", target_root / "run-id"
                self.put_team_run(source)
                original = migration.inventory(source)
                helper = self.fake_team_backend()
                repair = helper.repair.side_effect
                def fail(*args, **kwargs):
                    repair(*args, **kwargs)
                    if interrupted:
                        (source / "codex-home").rename(source / "helper-backup")
                        (source / "codex-home").symlink_to(target / "codex-home", target_is_directory=True)
                    raise RuntimeError("injected native failure")
                helper.repair.side_effect = fail
                if interrupted:
                    helper.recover.side_effect = RuntimeError("native shutdown uncertain")
                with patch.object(migration, "team_backend", return_value=helper):
                    plans = migration.preflight_team_relocation([source_root], target_root, codex_binary="fake")
                    profile = self.root / f"profile-{interrupted}"
                    profile.write_bytes(b"original config")
                    public_plan = setup.StorePlan(profiles={profile: ("original config", "new config")}, team_migrations=plans)
                    with self.assertRaisesRegex(RuntimeError, "uncertain" if interrupted else "native failure"):
                        setup.apply_store_environment(public_plan, dry_run=False)
                    self.assertEqual(profile.read_bytes(), b"original config")
                    if interrupted:
                        self.assertTrue((source / "codex-home").is_symlink())
                        self.assertEqual((source / "helper-backup/native").read_bytes(), b"native-original")
                    else:
                        self.assertEqual(migration.inventory(source), original)
                    self.assertTrue(target.exists())
                    if not interrupted:
                        self.assertEqual([child.name for child in target.iterdir()], ["run.lock"])
                    helper.repair.side_effect = repair
                    def recover(journal):
                        if (source / "codex-home").is_symlink():
                            (source / "codex-home").unlink()
                            (source / "helper-backup").rename(source / "codex-home")
                        return {"source_restored": True}
                    helper.recover.side_effect = recover
                    rerun = migration.preflight_team_relocation([source_root], target_root, codex_binary="fake")
                    if interrupted:
                        with self.assertRaisesRegex(RuntimeError, "recovery"):
                            migration.apply_team_migrations(rerun, dry_run=True)
                    migration.apply_team_migrations(rerun)
                    self.assertEqual(migration.inventory(source), original)
                    self.assertTrue((rerun[0].ledger / "attempt-000001/quarantine/codex-home/native").is_file())
                    self.assertEqual(migration.read_team_state(rerun[0])["phase"], "complete")
                    self.assertEqual(helper.repair.call_count, 2)
                    self.assertEqual(helper.preflight.call_count, 2)  # each received pristine independent bytes

    def test_team_membership_writer_and_capability_checks_prevent_publication(self):
        helper = self.fake_team_backend()
        source = self.source / "run-id"
        self.put_team_run(source)
        with patch.object(migration, "team_backend", return_value=helper):
            with migration.lease(source / "run.lock"):
                with self.assertRaisesRegex(RuntimeError, "lease"):
                    migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")
            plans = migration.preflight_team_relocation([self.source], self.destination, codex_binary="fake")
            helper.capabilities.return_value = {"admitted": False}
            with self.assertRaisesRegex(RuntimeError, "admission"):
                migration.apply_team_migrations(plans)
            self.assertFalse(self.destination.exists())
            self.assertFalse(plans[0].ledger.exists())
            helper.capabilities.return_value = {"admitted": True}
            self.put_team_run(self.source / "newly-arrived-run")
            with self.assertRaisesRegex(RuntimeError, "inventory changed"):
                migration.apply_team_migrations(plans)
            self.assertFalse(self.destination.exists())

    def test_team_payload_move_interruptions_recover_with_leased_roots_fixed(self):
        for interrupted_phase in ("backing_up", "quarantining", "restoring"):
            with self.subTest(phase=interrupted_phase):
                source_root = self.root / interrupted_phase / "old"
                target_root = self.root / interrupted_phase / "new"
                source, target = source_root / "run-id", target_root / "run-id"
                self.put_team_run(source)
                self.put_team_run(target)
                self.put(target, "manifest.json", migration.transform_team_manifest(
                    (source / "manifest.json").read_bytes(), source, target))
                original = migration.inventory(source)
                target_identity = migration.directory_identity(target)
                helper = self.fake_team_backend()
                repair = helper.repair.side_effect
                if interrupted_phase != "backing_up":
                    def failed_repair(*args, **kwargs):
                        repair(*args, **kwargs)
                        raise RuntimeError("native failure before payload recovery")
                    helper.repair.side_effect = failed_repair
                rename = Path.rename
                interrupt = True
                def guarded_rename(path, destination):
                    destination = Path(destination)
                    # Model the observed Windows denial on every host. No
                    # production move may rename either leased run root.
                    if path in (source, target):
                        raise PermissionError("leased directory rename denied")
                    result = rename(path, destination)
                    chosen = ((interrupted_phase == "backing_up" and destination.parent.name == "previous")
                              or (interrupted_phase == "quarantining" and destination.parent.name == "quarantine")
                              or (interrupted_phase == "restoring" and path.parent.name == "previous"))
                    if interrupt and chosen:
                        raise OSError("injected interruption after child rename")
                    return result
                with patch.object(migration, "team_backend", return_value=helper), \
                     patch.object(Path, "rename", guarded_rename):
                    plans = migration.preflight_team_relocation([source_root], target_root, codex_binary="fake")
                    with self.assertRaisesRegex(OSError, "injected interruption"):
                        migration.apply_team_migrations(plans)
                    self.assertEqual(migration.inventory(source), original)
                    self.assertEqual(migration.directory_identity(target), target_identity)
                    interrupt = False
                    helper.repair.side_effect = repair
                    again = migration.preflight_team_relocation([source_root], target_root, codex_binary="fake")
                    migration.apply_team_migrations(again)
                    self.assertEqual(migration.inventory(source), original)
                    self.assertEqual(migration.directory_identity(target), target_identity)
                    self.assertEqual(migration.read_team_state(again[0])["phase"], "complete")

    def test_locked_lease_inventory_uses_metadata_not_content(self):
        original_entry = migration.file_entry
        for tool, name in (("pira_ctx", ".events/workspace.lock"),
                           ("pira_ctx", "indexes/.index.owner-lock"),
                           ("pira_dec", "workspace/.write.lock"),
                           ("pira_team", "run.lock")):
            with self.subTest(tool=tool, name=name):
                root = self.root / tool / name.replace("/", "-")
                self.put_team_run(root) if tool == "pira_team" else self.put(root, "records/authoritative.lock")
                lock = self.put(root, name, b"lease bytes not record data")
                def guarded(path):
                    if path == lock:
                        raise PermissionError("simulated mandatory lock content denial")
                    return original_entry(path)
                with migration.lease(lock), patch.object(migration, "file_entry", side_effect=guarded):
                    snapshot = migration.inventory(root, tool=tool)
                    self.assertTrue(snapshot[name].digest.startswith("lease:"))
                    with self.assertRaisesRegex(RuntimeError, "lease"):
                        with migration.lease(lock):
                            self.fail("exclusive native lease lost")
                with patch.object(migration, "file_entry", side_effect=guarded):
                    if tool == "pira_team":
                        migration.team_history(root, root)
                    else:
                        target = self.root / (tool + "-" + name.replace("/", "-") + "-target")
                        plan = migration.plan_migration(tool, [root], target)
                        migration.apply_migrations([plan])
                        self.assertEqual((target / "records/authoritative.lock").read_bytes(), b"record\x00\xff")

    def test_replaced_lease_identity_is_not_hidden_by_metadata_inventory(self):
        lock = self.put(self.source, "workspace/.write.lock", b"same bytes")
        self.put(self.source, "workspace/record", b"authoritative")
        plan = self.plan()
        info = lock.stat()
        lock.rename(self.root / "retained-old-lock")
        replacement = self.put(self.source, "workspace/.write.lock", b"same bytes")
        os.utime(replacement, ns=(info.st_atime_ns, info.st_mtime_ns))
        with self.assertRaisesRegex(RuntimeError, "changed after migration preflight"):
            migration.apply_migrations([plan])
        self.assertFalse(self.destination.exists())

    def test_run_lease_remains_exclusive_during_payload_backup_and_restore(self):
        lock = self.put(self.source, "run.lock", b"")
        self.put(self.source, "codex-home/native", b"payload")
        with migration.lease(lock):
            migration.move_team_payload(self.source, self.destination)
            self.assertTrue(lock.exists())
            self.assertEqual((self.destination / "codex-home/native").read_bytes(), b"payload")
            self.assertFalse((self.destination / "run.lock").exists())
            with self.assertRaisesRegex(RuntimeError, "lease"):
                with migration.lease(lock):
                    self.fail("overlapping native lease accepted")
            migration.move_team_payload(self.destination, self.source)
            self.assertEqual((self.source / "codex-home/native").read_bytes(), b"payload")
        with migration.lease(lock):
            pass

    @patch('team_store_relocation._VALIDATED_PLATFORMS', frozenset())
    def test_fresh_team_default_allowed_retained_native_blocked_and_custom_retained(self):
        with patch.dict(os.environ, {"HOME": str(self.root)}, clear=True), \
             patch.object(setup.sys, "platform", "linux"), \
             patch.object(setup.tempfile, "gettempdir", return_value=str(self.root / "temporary")):
            fresh = setup.plan_store_environment(["pira_team"], profile_paths=[])
            self.assertEqual(fresh.migrations, [])
            legacy = setup.historical_store_paths("pira_team")[0]
            run = legacy / "run-id"
            self.put(run, "manifest.json", self.team_manifest(run))
            with self.assertRaisesRegex(RuntimeError, "native relocation blocked"):
                setup.plan_store_environment(["pira_team"], profile_paths=[])
            with patch.dict(os.environ, {"PIRA_TEAM_DIR": str(self.root / "custom-team")}):
                plan = setup.plan_store_environment(["pira_team"], profile_paths=[])
                self.assertEqual(plan.stores, {"PIRA_TEAM_DIR": str(self.root / "custom-team")})


if __name__ == "__main__":
    unittest.main()
